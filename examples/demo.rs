use actix::prelude::*;
use async_recursion::async_recursion;
use once_cell::sync::OnceCell;
use parking_lot::RwLock;
use sha3::Digest;

use std::cell::{RefCell, UnsafeCell};
use std::collections::{hash_map, HashMap, HashSet, VecDeque};
use std::fmt;
use std::io::{Read, Write};
use std::rc::Rc;
use std::str::FromStr;
use std::sync::{mpsc, Arc};

use qevm::chain::{Block, BlockNumber, BlockRef, ChainStateService};
use qevm::common::{
    Addr, Bytes, Bytes32, Gas, Hash, Wei, WrappedGas, WrappedU256, WrappedU64,
    U256,
};
use qevm::core::{TxExecEnv, TxExecResult, TxExecState, WorldStateW};
use qevm::mempool::MempoolService;
use qevm::processor::run_single_tx;
use qevm::state::{EmptyState, MemStateAuto, MemStateShared, WorldStateStore};

use qevm_tools::merkle::MerkleDB;

const MAX_BLOCK_TXS: usize = 2;
const RPC_GAS_CAP: Gas = 50000000;
const DB_PREFIX_MERKLE: &'static [u8] = b"merkle"; // this space keeps all merkle tries (world states)
const DB_PREFIX_OUTCOME: &'static [u8] = b"outcome"; // this space keeps block execution results
const DB_PREFIX_BLOCK: &'static [u8] = b"block"; // this space keeps block (and tx) data
const DB_PREFIX_INDEX: &'static [u8] = b"index"; // this space keeps block hashes on the canonical chain

struct DemoChainState(Rc<DemoChainStateInner>);

struct BlockOutcome {
    world_state: MemStateShared<'static>,
    gas_used: Vec<(Gas, Gas)>,
    contract_addrs: Vec<Option<Addr>>,
}

struct DemoChainStateInner {
    mut_state: UnsafeCell<DemoChainStateMutable>,
    chain_id: U256,
    network_id: String,
}

struct DemoChainStateMutable {
    core_in: mpsc::Sender<TxExecState>,
    core_out: mpsc::Receiver<TxExecState>,
    _core: qevm::core::Core,
    chain: qevm::chain::Chain<DemoBlock>,
    tx_cache: HashMap<Hash, (BlockRef<DemoBlock>, usize)>,
    indexed: Arc<RwLock<Vec<Option<BlockRef<DemoBlock>>>>>,
    mdb: MerkleDB,
    chain_id: U256,
}

impl Actor for DemoChainState {
    type Context = Context<Self>;
}

impl DemoChainState {
    async fn new(
        chain_id: u64, network_id: String, genesis_account: (&Addr, &Wei),
        db: rocksdb::DB,
    ) -> Self {
        let chain_id = chain_id.into();
        Self(Rc::new(DemoChainStateInner {
            mut_state: UnsafeCell::new(
                DemoChainStateMutable::new(genesis_account, db, &chain_id)
                    .await,
            ),
            chain_id,
            network_id,
        }))
    }
}

impl DemoChainStateInner {
    fn mut_state(&self) -> &mut DemoChainStateMutable {
        unsafe { &mut *self.mut_state.get() }
    }

    async fn get_block_info(
        &self, block: BlockRef<DemoBlock>, verbose: bool,
    ) -> Option<qevm::chain::BlockInfo> {
        let bs = self.get_block_state(&block).await?;
        let mut info = qevm::chain::BlockInfo::default();
        info.number = Some(WrappedU256(block.number()));
        info.hash = Some(block.hash().clone());
        info.parent_hash = block.parent().clone();
        info.timestamp = WrappedU64(block.timestamp);
        info.gas_used =
            WrappedGas(bs.gas_used.last().map(|e| e.0).unwrap_or(0));
        info.txs = if verbose {
            let block_hash = block.hash();
            let block_number = block.number();
            qevm::chain::TxResponseInfo::Objects(
                block
                    .txs
                    .iter()
                    .enumerate()
                    .map(|(i, tx)| {
                        let v: Bytes32 = tx.v().into();
                        let r: Bytes32 = tx.r().into();
                        let s: Bytes32 = tx.s().into();
                        qevm::chain::TxResponse {
                            block_hash: Some(block_hash.clone()),
                            block_number: Some(WrappedU256(
                                block_number.clone(),
                            )),
                            from: tx.from().clone(),
                            gas: WrappedGas(tx.gas()),
                            gas_price: tx.gas_price().clone(),
                            hash: tx.hash().clone(),
                            input: tx.data().clone(),
                            nonce: WrappedU64(tx.nonce()),
                            to: qevm::common::NullableAddr(
                                tx.to().map(|e| e.clone()),
                            ),
                            tx_index: WrappedU64(i as u64),
                            value: tx.value().clone(),
                            v: v.into(),
                            r: r.into(),
                            s: s.into(),
                        }
                    })
                    .collect(),
            )
        } else {
            qevm::chain::TxResponseInfo::Hashes(
                block.txs.iter().map(|tx| tx.hash().clone()).collect(),
            )
        };
        Some(info)
    }

    fn get_exec_env(&self, block: &DemoBlock) -> TxExecEnv {
        let indexed = self.mut_state().indexed.clone();
        TxExecEnv {
            chain_id: self.chain_id.clone(),
            precompiled_contracts: HashMap::new(),
            block: qevm::core::BlockInfo {
                coinbase: Addr::zero().clone(),
                timestamp: block.timestamp.into(),
                number: block.number.into(),
                difficulty: U256::zero(),
                gas_limit: block.gas_limit,
                base_fee: block.base_fee,
                fork: qevm::core::Fork::London,
            },
            block_hash_getter: Box::new(move |number: u64| {
                let indexed = indexed.clone();
                Box::pin(async move {
                    let indexed = indexed.read();
                    indexed[number as usize].as_ref().unwrap().hash().clone()
                })
            }),
        }
    }

    async fn process_state_transition(
        &self, block: &DemoBlock,
    ) -> Option<(BlockOutcome, Hash)> {
        use qevm::core::Transferable;
        let s = self.mut_state();
        let prev_block = s.get_block_by_hash(block.parent())?;
        let prev_state = &self.get_block_state(&prev_block).await?.world_state;
        let prev_root = prev_block.root_hash.clone();
        let cur_state: Arc<RwLock<MemStateAuto>> =
            Arc::new(RwLock::new(prev_state.clone().into()));
        let env = Arc::new(self.get_exec_env(block));
        let mut gas_cumu = 0;
        let mut gas_used = Vec::new();
        let mut contract_addrs = Vec::new();
        for tx in &block.txs {
            let gas = tx.gas();
            // buy gas by burning the fee
            let paid = tx.gas_price().checked_mul(&Wei::from(gas))?;
            if let None = cur_state
                .write()
                .transfer_balance(tx.from(), Addr::zero(), &paid)
                .await
            {
                println!("insufficient balance, skipped");
                continue
            }
            match run_single_tx(
                TxExecState::new(
                    tx.from().clone(),
                    tx.to().map(|e| e.clone()),
                    tx.value().clone(),
                    gas,
                    tx.gas_price().clone(),
                    tx.data().as_ref().into(),
                    None,
                    cur_state.clone(),
                    prev_state.clone().into(),
                    env.clone(),
                ),
                &s.core_in,
                &s.core_out,
            )
            .await
            {
                TxExecResult::Succeeded(data, unused_gas, contract_addr) => {
                    let used_gas = tx.gas() - unused_gas;
                    gas_cumu += used_gas;
                    gas_used.push((gas_cumu, used_gas));
                    contract_addrs.push(contract_addr);
                    println!(
                        "tx {} returned (data={}) used {} gas",
                        tx.hash(),
                        hex::encode(data.as_ref()),
                        used_gas,
                    )
                }
                TxExecResult::Reverted(data, _, err) => println!(
                    "tx {} reverted (data={} err={:?})",
                    tx.hash(),
                    hex::encode(data.as_ref()),
                    err
                ),
            }
        }
        let mut world_state = cur_state.read().to_shared();
        world_state.consolidate(None);
        Some((
            BlockOutcome {
                world_state,
                gas_used,
                contract_addrs,
            },
            prev_root,
        ))
    }

    #[async_recursion(?Send)]
    async fn get_block_state(
        &self, block: &BlockRef<DemoBlock>,
    ) -> Option<BlockOutcome> {
        let ms = self.mut_state();
        let reader = ms.mdb.read(if &block.root_hash == Hash::zero() {
            None
        } else {
            Some(&block.root_hash)
        });
        BlockOutcome::decode(
            &ms.mdb.db().get(&gen_outcome_key(block.hash())).ok()??,
            MemStateShared::new(reader),
        )
    }

    async fn do_call(
        &self, tx_args: &qevm::chain::TxArgs, number: &BlockNumber,
        global_gas_cap: Gas,
    ) -> Option<TxExecResult> {
        let ms = self.mut_state();
        let block = ms.get_block_by_number(&number)?;
        let prev_state = &self.get_block_state(&block).await?.world_state;
        let cur_state: Arc<RwLock<MemStateAuto>> =
            Arc::new(RwLock::new(prev_state.clone().into()));
        let env = Arc::new(self.get_exec_env(&block));
        Some(
            run_single_tx(
                TxExecState::new(
                    tx_args
                        .from
                        .as_ref()
                        .unwrap_or_else(|| Addr::zero())
                        .clone(),
                    tx_args.to.clone(),
                    tx_args
                        .value
                        .as_ref()
                        .unwrap_or_else(|| Wei::zero())
                        .clone(),
                    tx_args
                        .gas
                        .as_ref()
                        .map(|e| e.clone())
                        .unwrap_or(WrappedGas(global_gas_cap))
                        .0,
                    tx_args
                        .gas_price
                        .as_ref()
                        .unwrap_or_else(|| Wei::zero())
                        .clone(),
                    tx_args
                        .data
                        .as_ref()
                        .map(|e| e.clone())
                        .unwrap_or_else(|| Bytes::default())
                        .into_inner()
                        .into(),
                    None,
                    cur_state,
                    prev_state.clone().into(),
                    env,
                ),
                &ms.core_in,
                &ms.core_out,
            )
            .await,
        )
    }

    async fn do_estimate_gas(
        &self, mut tx_args: qevm::chain::TxArgs, number: BlockNumber,
        gas_cap: Gas,
    ) -> Option<Gas> {
        use qevm::core::params::GAS_TX;
        let mut low = GAS_TX - 1;
        let mut high = 0;
        let mut flag = true;
        let ms = self.mut_state();
        let from = tx_args.from.as_ref().unwrap_or_else(|| Addr::zero());
        if let Some(WrappedGas(gas)) = &tx_args.gas {
            if gas >= &GAS_TX {
                high = *gas;
                flag = false;
            }
        }
        if flag {
            let block = ms.get_block_by_number(&number)?;
            high = block.gas_limit
        }

        let fee_cap: U256 = if tx_args.gas_price.is_some() &&
            (tx_args.max_fee_per_gas.is_some() ||
                tx_args.max_priority_fee_per_gas.is_some())
        {
            // both gasPrice and (maxFeePerGas or maxPriorityFeePerGas) specified
            return None
        } else if let Some(p) = &tx_args.gas_price {
            p.clone().into()
        } else if let Some(p) = &tx_args.max_fee_per_gas {
            p.clone().into()
        } else {
            U256::zero().clone()
        };

        if fee_cap.bits() != 0 {
            let block = ms.get_block_by_number(&number)?;
            let state = &self.get_block_state(&block).await?.world_state;
            let balance = state.get_balance(from).await;
            let avail = balance.clone();
            if let Some(val) = &tx_args.value {
                if val >= &avail {
                    // insufficient funds for transfer
                    return None
                }
            }
            let allow = U256::from(avail) / fee_cap;
            if let Some(a) = qevm::common::checked_as_u64(&allow) {
                if high > a {
                    // warn: gas estimation capped by limited funds
                    high = a
                }
            }
        }
        if gas_cap != 0 && high > gas_cap {
            // warn: caller gas above allowance, capping
            high = gas_cap
        }
        let cap = high;

        // use binary search to find the estimated gas
        while low + 1 < high {
            let mid = (low + high) / 2;
            tx_args.gas = Some(WrappedGas(mid));
            match self.do_call(&tx_args, &number, gas_cap).await? {
                TxExecResult::Succeeded(..) => high = mid,
                TxExecResult::Reverted(..) => low = mid,
            }
        }

        if cap == high {
            tx_args.gas = Some(WrappedGas(high));
            if let TxExecResult::Reverted(..) =
                self.do_call(&tx_args, &number, gas_cap).await?
            {
                return None
            }
        }
        Some(high)
    }
}

impl DemoChainStateMutable {
    async fn new(
        (genesis_addr, genesis_balance): (&Addr, &Wei), db: rocksdb::DB,
        chain_id: &U256,
    ) -> Self {
        let (_core, core_in, core_out) = qevm::core::Core::new();
        let mut cs = Self {
            _core,
            core_in,
            core_out,
            chain: qevm::chain::Chain::new(DemoBlock::new(
                0,
                Hash::zero().clone(),
                Hash::zero().clone(),
                None,
                Vec::new(),
            )),
            tx_cache: HashMap::new(),
            indexed: Arc::new(RwLock::new(Vec::new())),
            mdb: MerkleDB::new(db, DB_PREFIX_MERKLE),
            chain_id: chain_id.clone(),
        };
        let mut gstate = MemStateAuto::new(Arc::new(EmptyState));
        gstate.create_account(&genesis_addr).await;
        gstate.set_balance(&genesis_addr, &genesis_balance);
        // genesis block is stored at 0x0 (and always available in memory)
        let g_ret = cs.mdb.db().get(gen_block_key(Hash::zero())).unwrap();
        let g = if let Some(g_raw) = g_ret {
            DemoBlock::decode(&g_raw, chain_id).unwrap()
        } else {
            println!("genesis block does not exist, initializing the DB");
            let mut world_state = gstate.to_shared();
            world_state.consolidate(None);
            let delta = world_state.consolidated_delta().unwrap().clone();
            let outcome = BlockOutcome {
                world_state,
                gas_used: Vec::new(),
                contract_addrs: Vec::new(),
            };
            let mut g = DemoBlock::new(
                0,
                Hash::zero().clone(),
                Hash::zero().clone(),
                Some(1655254842),
                Vec::new(),
            );
            g.root_hash = cs
                .mdb
                .commit(None, &delta, &cs.mdb.get_reader(None))
                .await
                .unwrap();
            {
                let mut wb = cs.mdb.current_writebatch();
                wb.put(gen_block_key(Hash::zero()), g.encode());
                wb.put(gen_outcome_key(g.hash()), outcome.encode());
                wb.put(gen_index_key(0), 1u64.to_le_bytes());
                wb.put(gen_index_key(1), g.hash().as_bytes());
            }
            cs.mdb.commit_writebatch();
            g
        };
        cs.chain = qevm::chain::Chain::new(g);

        let val = cs.mdb.db().get(gen_index_key(0)).unwrap();

        // load canonical chain
        if let Some(raw_len) = val {
            let index_len = u64::from_le_bytes(raw_len.try_into().unwrap());
            let mut indexed = Vec::new();
            indexed.push(Some(cs.chain.get_base().clone()));
            for i in 1..index_len {
                let val = cs.mdb.db().get(gen_index_key(i + 1)).unwrap();
                let block = val.map(|r| {
                    cs.get_block_by_hash(&Hash::from_slice(&r)).unwrap()
                });
                indexed.push(block);
            }
            *cs.indexed.write() = indexed;
        }
        cs
    }

    fn set_canonical_tail(&mut self, new_tail: &BlockRef<DemoBlock>) {
        let new_tail = new_tail.clone();
        let indexed = self.indexed.clone();
        let mut indexed = indexed.write();
        indexed.resize((new_tail.number + 1) as usize, None);
        let mut next_block = new_tail;
        self.mdb
            .current_writebatch()
            .put(gen_index_key(0), (indexed.len() as u64).to_le_bytes());
        for (i, b) in indexed.iter_mut().enumerate().rev() {
            let nblock = next_block.hash();
            if let Some(b) = b {
                let block = b;
                if block.hash() == nblock {
                    break
                }
            }
            self.mdb
                .current_writebatch()
                .put(gen_index_key((i + 1) as u64), nblock.as_bytes());
            *b = Some(self.get_block_by_hash(nblock).unwrap());
            if next_block.number().is_zero() {
                break
            }
            next_block = self.get_block_by_hash(next_block.parent()).unwrap();
        }
        self.mdb.commit_writebatch();
    }

    fn get_block_by_number(
        &self, number: &BlockNumber,
    ) -> Option<BlockRef<DemoBlock>> {
        use BlockNumber::*;
        let indexed = self.indexed.read();
        let number = match number {
            Earliest => 0,
            Latest | Pending => indexed.len() - 1,
            Number(x) => x.low_u64() as usize,
        };
        if number < indexed.len() {
            return Some(indexed[number as usize].as_ref().unwrap().clone())
        }
        None
    }

    fn get_block_by_hash(
        &mut self, hash: &Hash,
    ) -> Option<BlockRef<DemoBlock>> {
        assert!(hash != Hash::zero());
        if let Some(block) = self.chain.get_block(hash) {
            return Some(block)
        }
        let block = DemoBlock::decode(
            &self.mdb.db().get(&gen_block_key(hash)).ok()??,
            &self.chain_id,
        )?;
        Some(self.chain.add_block(block))
    }

    fn get_cached_tx(
        &mut self, tx_hash: &Hash,
    ) -> Option<&(BlockRef<DemoBlock>, usize)> {
        match self.tx_cache.entry(tx_hash.clone()) {
            hash_map::Entry::Occupied(e) => Some(e.into_mut()),
            hash_map::Entry::Vacant(e) => {
                // lookup on the indexed chain
                for b in self.indexed.read().iter().rev() {
                    let block = b.as_ref().unwrap();
                    for (idx, tx) in block.txs.iter().enumerate() {
                        if tx.hash() == tx_hash {
                            return Some(e.insert((block.clone(), idx)))
                        }
                    }
                }
                None
            }
        }
    }
}

impl Handler<qevm::chain::GetBalance> for DemoChainState {
    type Result = ResponseFuture<Option<qevm::common::WrappedWei>>;
    fn handle(
        &mut self, msg: qevm::chain::GetBalance, _ctx: &mut Self::Context,
    ) -> Self::Result {
        println!("{:?}", msg);
        let s = self.0.clone();
        Box::pin(async move {
            let cs = s.mut_state();
            let ws = &s
                .get_block_state(&cs.get_block_by_number(&msg.1)?)
                .await?
                .world_state;
            Some(qevm::common::WrappedWei(ws.get_balance(&msg.0).await))
        })
    }
}

impl Handler<qevm::chain::GetNonce> for DemoChainState {
    type Result = ResponseFuture<Option<WrappedU64>>;
    fn handle(
        &mut self, msg: qevm::chain::GetNonce, _ctx: &mut Self::Context,
    ) -> Self::Result {
        println!("{:?}", msg);
        let s = self.0.clone();
        Box::pin(async move {
            let cs = s.mut_state();
            let ws = &s
                .get_block_state(&cs.get_block_by_number(&msg.1)?)
                .await?
                .world_state;
            Some(WrappedU64(ws.get_nonce(&msg.0).await))
        })
    }
}

impl Handler<qevm::chain::GetState> for DemoChainState {
    type Result = ResponseFuture<Option<WrappedU256>>;
    fn handle(
        &mut self, msg: qevm::chain::GetState, _ctx: &mut Self::Context,
    ) -> Self::Result {
        println!("{:?}", msg);
        let s = self.0.clone();
        Box::pin(async move {
            let cs = s.mut_state();
            let ws = &s
                .get_block_state(&cs.get_block_by_number(&msg.2)?)
                .await?
                .world_state;
            Some(WrappedU256(ws.get_state(&msg.0, &msg.1 .0.into()).await))
        })
    }
}

impl Handler<qevm::chain::GetCode> for DemoChainState {
    type Result = ResponseFuture<Option<Bytes>>;
    fn handle(
        &mut self, msg: qevm::chain::GetCode, _ctx: &mut Self::Context,
    ) -> Self::Result {
        println!("{:?}", msg);
        let s = self.0.clone();
        Box::pin(async move {
            let cs = s.mut_state();
            let ws = &s
                .get_block_state(&cs.get_block_by_number(&msg.1)?)
                .await?
                .world_state;
            Some(ws.get_code(&msg.0).await.as_bytes().into())
        })
    }
}

impl Handler<qevm::chain::GetTailBlock> for DemoChainState {
    type Result = qevm::chain::TailBlockResp;
    fn handle(
        &mut self, msg: qevm::chain::GetTailBlock, _ctx: &mut Self::Context,
    ) -> Self::Result {
        println!("{:?}", msg);
        let block = self
            .0
            .mut_state()
            .get_block_by_number(&BlockNumber::Latest)
            .unwrap();
        qevm::chain::TailBlockResp(
            WrappedU256(block.number()),
            block.hash().clone(),
        )
    }
}

impl Handler<qevm::chain::Call> for DemoChainState {
    type Result = ResponseFuture<Option<qevm::chain::WrappedTxExecResult>>;
    fn handle(
        &mut self, msg: qevm::chain::Call, _ctx: &mut Self::Context,
    ) -> Self::Result {
        println!("{:?}", msg);
        let s = self.0.clone();
        Box::pin(async move {
            s.do_call(&msg.0, &msg.1, RPC_GAS_CAP)
                .await
                .map(qevm::chain::WrappedTxExecResult)
        })
    }
}

impl Handler<qevm::chain::EstimateGas> for DemoChainState {
    type Result = ResponseFuture<Option<WrappedGas>>;
    fn handle(
        &mut self, msg: qevm::chain::EstimateGas, _ctx: &mut Self::Context,
    ) -> Self::Result {
        println!("{:?}", msg);
        let s = self.0.clone();
        Box::pin(async move {
            s.do_estimate_gas(msg.0, msg.1, RPC_GAS_CAP)
                .await
                .map(qevm::common::WrappedGas)
        })
    }
}

impl Handler<qevm::chain::GetTxReceipt> for DemoChainState {
    type Result = ResponseFuture<Option<qevm::chain::TxReceipt>>;
    fn handle(
        &mut self, msg: qevm::chain::GetTxReceipt, _ctx: &mut Self::Context,
    ) -> Self::Result {
        println!("{:?}", msg);
        let s = self.0.clone();
        Box::pin(async move {
            if let Some((block, idx)) = s.mut_state().get_cached_tx(&msg.0) {
                let mut r = qevm::chain::TxReceipt::default();
                let bs = s.get_block_state(&block).await?;
                let gas_used = &bs.gas_used;
                let contract_addrs = &bs.contract_addrs;
                let idx = *idx;
                let tx = &block.txs[idx];
                r.tx_hash = tx.hash().clone();
                r.tx_index = WrappedU64(idx as u64);
                r.block_hash = block.hash().clone();
                r.block_number = WrappedU256(block.number().clone());
                r.from = tx.from().clone();
                r.to = tx.to().map(|e| e.clone());
                r.cu_gas_used = WrappedGas(gas_used[idx].0);
                r.gas_used = WrappedGas(gas_used[idx].1);
                r.contract_addr =
                    contract_addrs[idx].as_ref().map(|e| e.clone());
                r.status = WrappedU64(1);
                return Some(r)
            }
            None
        })
    }
}

impl Handler<qevm::chain::GetTxByHash> for DemoChainState {
    type Result = ResponseFuture<Option<qevm::chain::TxResponse>>;
    fn handle(
        &mut self, msg: qevm::chain::GetTxByHash, _ctx: &mut Self::Context,
    ) -> Self::Result {
        println!("{:?}", msg);
        let s = self.0.clone();
        Box::pin(async move {
            if let Some((block, idx)) = s.mut_state().get_cached_tx(&msg.0) {
                let tx = &block.txs[*idx];
                let r = qevm::chain::TxResponse {
                    block_hash: Some(block.hash().clone()),
                    block_number: Some(WrappedU256(block.number().clone())),
                    from: tx.from().clone(),
                    gas: WrappedGas(tx.gas()),
                    gas_price: tx.gas_price().clone(),
                    hash: tx.hash().clone(),
                    input: tx.data().clone(),
                    nonce: WrappedU64(tx.nonce()),
                    to: qevm::common::NullableAddr(tx.to().map(|e| e.clone())),
                    tx_index: WrappedU64(*idx as u64),
                    value: tx.value().clone(),
                    v: tx.v().into(),
                    r: tx.r().into(),
                    s: tx.s().into(),
                };
                return Some(r)
            }
            None
        })
    }
}

impl Handler<qevm::chain::GetNetworkId> for DemoChainState {
    type Result = String;
    fn handle(
        &mut self, msg: qevm::chain::GetNetworkId, _ctx: &mut Self::Context,
    ) -> Self::Result {
        println!("{:?}", msg);
        self.0.network_id.clone()
    }
}

impl Handler<qevm::chain::GetChainId> for DemoChainState {
    type Result = WrappedU256;
    fn handle(
        &mut self, msg: qevm::chain::GetChainId, _ctx: &mut Self::Context,
    ) -> Self::Result {
        println!("{:?}", msg);
        WrappedU256(self.0.chain_id)
    }
}

impl Handler<qevm::chain::GetBlockByHash> for DemoChainState {
    type Result = ResponseFuture<Option<qevm::chain::BlockInfo>>;
    fn handle(
        &mut self, msg: qevm::chain::GetBlockByHash, _ctx: &mut Self::Context,
    ) -> Self::Result {
        println!("{:?}", msg);
        let s = self.0.clone();
        Box::pin(async move {
            let block = s.mut_state().get_block_by_hash(&msg.0)?;
            s.get_block_info(block, msg.1).await
        })
    }
}

impl Handler<qevm::chain::GetBlockByNumber> for DemoChainState {
    type Result = ResponseFuture<Option<qevm::chain::BlockInfo>>;
    fn handle(
        &mut self, msg: qevm::chain::GetBlockByNumber, _ctx: &mut Self::Context,
    ) -> Self::Result {
        println!("{:?}", msg);
        let s = self.0.clone();
        Box::pin(async move {
            let block = s.mut_state().get_block_by_number(&msg.0)?;
            s.get_block_info(block, msg.1).await
        })
    }
}

impl Handler<qevm::chain::AddBlocks<DemoBlock>> for DemoChainState {
    type Result = ResponseFuture<()>;
    fn handle(
        &mut self, msg: qevm::chain::AddBlocks<DemoBlock>,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let s = self.0.clone();
        Box::pin(async move {
            let cs = s.mut_state();
            let block = cs.get_block_by_number(&BlockNumber::Latest).unwrap();
            let mut number = block.number().as_u64();
            let mut p_hash = block.hash().clone();

            for mut block in msg.0.into_iter() {
                number += 1;
                block.number = number;
                block.parent_hash = p_hash;
                // complete the content of a block from mempool
                let (outcome, old_root) =
                    s.process_state_transition(&block).await.unwrap();
                let old_root = Some(old_root);
                // create new trie root
                let delta = outcome.world_state.consolidated_delta().unwrap();
                block.root_hash = cs
                    .mdb
                    .commit(
                        old_root.as_ref(),
                        delta,
                        &cs.mdb.get_reader(old_root.as_ref()),
                    )
                    .await
                    .unwrap();
                // no hashing before root_hash field is filled in
                assert!(block.cached_hash.get().is_none());
                let b = cs.chain.add_block(block);
                p_hash = b.hash().clone();
                {
                    let mut wb = cs.mdb.current_writebatch();
                    wb.put(gen_block_key(b.hash()), b.encode());
                    wb.put(gen_outcome_key(b.hash()), outcome.encode());
                }
                cs.mdb.commit_writebatch();
                // NOTE: in the demo, always set the tail to the highest block
                if b.number() >= cs.indexed.read().len().into() {
                    if cs.chain.is_attached(&b.hash()) {
                        cs.set_canonical_tail(&b);
                    }
                }
                println!("added block {}", &*b);
            }
        })
    }
}

impl ChainStateService for DemoChainState {
    type Block = DemoBlock;
}

struct DemoMempoolInner<C: ChainStateService> {
    tx_queue: VecDeque<qevm::tx::Tx>,
    in_queue: HashSet<Hash>,
    chain_state: actix::Addr<C>,
    chain_id: U256,
}

struct DemoMempool<C: ChainStateService>(Rc<RefCell<DemoMempoolInner<C>>>);

impl<C: ChainStateService> DemoMempool<C> {
    fn new(chain_id: u64, chain_state: &actix::Addr<C>) -> Self {
        let chain_id = chain_id.into();
        let chain_state = chain_state.clone();
        Self(Rc::new(RefCell::new(DemoMempoolInner {
            tx_queue: VecDeque::new(),
            in_queue: HashSet::new(),
            chain_state,
            chain_id,
        })))
    }
}

impl<C: ChainStateService> Actor for DemoMempool<C> {
    type Context = Context<Self>;
}

impl<C: ChainStateService> Handler<qevm::mempool::AddTransaction>
    for DemoMempool<C>
{
    type Result = Option<qevm::common::WrappedHash>;
    fn handle(
        &mut self, msg: qevm::mempool::AddTransaction, _ctx: &mut Self::Context,
    ) -> Self::Result {
        println!("{:?}", msg);
        let mut mempool = self.0.borrow_mut();
        let tx = qevm::tx::Tx::decode(&msg.0, &mempool.chain_id)?;
        let tx_hash = tx.hash().clone();
        if mempool.in_queue.insert(tx_hash.clone()) {
            mempool.tx_queue.push_back(tx);
        }
        Some(qevm::common::WrappedHash(tx_hash))
    }
}

impl<C: ChainStateService> Handler<qevm::mempool::GasPrice> for DemoMempool<C> {
    type Result = qevm::common::WrappedWei;
    fn handle(
        &mut self, msg: qevm::mempool::GasPrice, _ctx: &mut Self::Context,
    ) -> Self::Result {
        println!("{:?}", msg);
        // cheap!
        qevm::common::WrappedWei(Wei::from_str("0x174876e800").unwrap())
    }
}

struct DemoBlock {
    number: u64,
    parent_hash: Hash,
    root_hash: Hash,
    // NOTE: this demo does not include receiptHash
    timestamp: u64,
    gas_limit: Gas,
    base_fee: U256,
    txs: Vec<qevm::tx::Tx>,
    cached_hash: OnceCell<Hash>,
}

impl fmt::Display for DemoBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(
            f,
            "[Block {}({})=>{} txs=({})]",
            self.hash(),
            self.number,
            self.parent_hash,
            self.txs
                .iter()
                .map(|tx| format!("{}", tx.hash()))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

impl DemoBlock {
    fn new(
        number: u64, parent_hash: Hash, root_hash: Hash,
        timestamp: Option<u64>, txs: Vec<qevm::tx::Tx>,
    ) -> Self {
        use std::time::SystemTime;
        let timestamp = timestamp.unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        });
        Self {
            number,
            parent_hash,
            root_hash,
            timestamp,
            gas_limit: 8_000_000,
            base_fee: U256::zero().clone(),
            txs,
            cached_hash: OnceCell::new(),
        }
    }
}

impl Block for DemoBlock {
    fn number(&self) -> U256 {
        self.number.into()
    }
    fn parent(&self) -> &Hash {
        &self.parent_hash
    }
    fn hash(&self) -> &Hash {
        self.cached_hash.get_or_init(|| {
            let mut hasher = sha3::Keccak256::new();
            hasher.update(self.number.to_le_bytes());
            hasher.update(self.parent_hash.as_bytes());
            hasher.update(self.root_hash.as_bytes());
            hasher.update(self.timestamp.to_le_bytes());
            hasher.update(self.gas_limit.to_le_bytes());
            hasher.update(&Bytes32::from(&self.base_fee)[..]);
            for tx in &self.txs {
                hasher.update(tx.hash().as_bytes());
            }
            Hash::from_slice(hasher.finalize().as_slice())
        })
    }
}

impl<C: ChainStateService<Block = DemoBlock>> Handler<qevm::mempool::GenBlocks>
    for DemoMempool<C>
{
    type Result = ResponseFuture<()>;
    fn handle(
        &mut self, _: qevm::mempool::GenBlocks, _ctx: &mut Self::Context,
    ) -> Self::Result {
        let s = self.0.clone();
        Box::pin(async move {
            let mut block_txs = Vec::new();
            let mut blocks = Vec::new();
            let mut mempool = s.borrow_mut();
            macro_rules! flush_txs {
                () => {
                    let txs = std::mem::replace(&mut block_txs, Vec::new());
                    // the root_hash is empty and will be filled in later by DemoChainState
                    let block = DemoBlock::new(0, Hash::zero().clone(), Hash::zero().clone(), None, txs);
                    blocks.push(block);
                };
            }
            while let Some(p) = mempool.tx_queue.pop_front() {
                block_txs.push(p);
                if block_txs.len() == MAX_BLOCK_TXS {
                    flush_txs!();
                }
            }
            if block_txs.len() > 0 {
                flush_txs!();
            }
            mempool
                .chain_state
                .send(qevm::chain::AddBlocks(blocks))
                .await
                .unwrap();
        })
    }
}

impl<C> MempoolService for DemoMempool<C> where
    C: ChainStateService<Block = DemoBlock>
{
}

fn main() {
    use tokio::runtime;
    let system = actix::System::with_tokio_rt(|| {
        runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .unwrap()
    });
    let chain_id = 10086;
    let network_id = "10086".to_string();
    let airdrop = (
        &Addr::from_str("0xa91212112ab5a2469429A8B2A948097065F4970b").unwrap(),
        &Wei::from_str("0x1000000000000000000").unwrap(),
    );
    let db = rocksdb::DB::open_default("./demo-db").unwrap();

    let chain_state = system.block_on(async {
        DemoChainState::new(chain_id, network_id, airdrop, db)
            .await
            .start()
    });
    let mempool = system
        .block_on(async { DemoMempool::new(chain_id, &chain_state).start() });
    let server =
        qevm::rpc::RPCServer::new("0.0.0.0:3000", &chain_state, &mempool);
    system.block_on(async {
        actix_rt::spawn(async move {
            let mut interval =
                actix_rt::time::interval(std::time::Duration::from_secs(5));
            loop {
                interval.tick().await;
                println!("polling mempool to generate blocks...");
                mempool.send(qevm::mempool::GenBlocks).await.unwrap();
            }
        })
    });
    system.block_on(async { server.start() });
    system.run().unwrap();
}

fn gen_outcome_key(block: &Hash) -> Vec<u8> {
    let mut key = Vec::new();
    key.write(DB_PREFIX_OUTCOME).unwrap();
    key.write(block.as_bytes()).unwrap();
    key
}

fn gen_block_key(block: &Hash) -> Vec<u8> {
    let mut key = Vec::new();
    key.write(DB_PREFIX_BLOCK).unwrap();
    key.write(block.as_bytes()).unwrap();
    key
}

fn gen_index_key(idx: u64) -> Vec<u8> {
    let mut key = Vec::new();
    key.write(DB_PREFIX_INDEX).unwrap();
    key.write(&idx.to_le_bytes()).unwrap();
    key
}

impl BlockOutcome {
    fn encode(&self) -> Vec<u8> {
        let mut buff = Vec::new();
        buff.write(&(self.gas_used.len() as u64).to_le_bytes())
            .unwrap();
        for (g0, g1) in &self.gas_used {
            buff.write(&g0.to_le_bytes()).unwrap();
            buff.write(&g1.to_le_bytes()).unwrap();
        }
        buff.write(&(self.contract_addrs.len() as u64).to_le_bytes())
            .unwrap();
        for addr in &self.contract_addrs {
            buff.write(addr.as_ref().unwrap_or(Addr::zero()).as_bytes())
                .unwrap();
        }
        buff
    }

    fn decode(
        mut raw: &[u8], world_state: MemStateShared<'static>,
    ) -> Option<Self> {
        let mut buff = [0u8; 20];
        raw.read_exact(&mut buff[..8]).ok()?;
        let gas_used_len = u64::from_le_bytes(buff[..8].try_into().unwrap());
        let mut gas_used = Vec::new();
        for _ in 0..gas_used_len {
            raw.read_exact(&mut buff[..8]).ok()?;
            let g0 = u64::from_le_bytes(buff[..8].try_into().unwrap());
            raw.read_exact(&mut buff[..8]).ok()?;
            let g1 = u64::from_le_bytes(buff[..8].try_into().unwrap());
            gas_used.push((g0, g1));
        }
        raw.read_exact(&mut buff[..8]).ok()?;
        let contract_addrs_len =
            u64::from_le_bytes(buff[..8].try_into().unwrap());
        let mut contract_addrs = Vec::new();
        assert_eq!(gas_used_len, contract_addrs_len);
        for _ in 0..contract_addrs_len {
            raw.read_exact(&mut buff[..20]).ok()?;
            let addr = Addr::from_slice(&buff[..20]);
            contract_addrs.push(if &addr == Addr::zero() {
                None
            } else {
                Some(addr)
            });
        }
        Some(Self {
            world_state,
            gas_used,
            contract_addrs,
        })
    }
}

impl DemoBlock {
    fn encode(&self) -> Vec<u8> {
        let mut buff = Vec::new();
        buff.write(&self.number.to_le_bytes()).unwrap();
        buff.write(self.parent_hash.as_bytes()).unwrap();
        buff.write(self.root_hash.as_bytes()).unwrap();
        buff.write(&self.timestamp.to_le_bytes()).unwrap();
        buff.write(&self.gas_limit.to_le_bytes()).unwrap();
        buff.write(&Bytes32::from(&self.base_fee)[..]).unwrap();
        buff.write(&(self.txs.len() as u64).to_le_bytes()).unwrap();
        for tx in &self.txs {
            let tx_raw = tx.encode();
            buff.write(&(tx_raw.len() as u64).to_le_bytes()).unwrap();
            buff.extend(tx_raw);
        }
        buff
    }

    fn decode(mut raw: &[u8], chain_id: &U256) -> Option<Self> {
        let mut buff = [0u8; 32];
        raw.read_exact(&mut buff[..8]).ok()?;
        let number = u64::from_le_bytes(buff[..8].try_into().unwrap());
        raw.read_exact(&mut buff[..32]).ok()?;
        let parent_hash = Hash::from_slice(&buff[..32]);
        raw.read_exact(&mut buff[..32]).ok()?;
        let root_hash = Hash::from_slice(&buff[..32]);
        raw.read_exact(&mut buff[..8]).ok()?;
        let timestamp = u64::from_le_bytes(buff[..8].try_into().unwrap());
        raw.read_exact(&mut buff[..8]).ok()?;
        let gas_limit = u64::from_le_bytes(buff[..8].try_into().unwrap());
        raw.read_exact(&mut buff[..32]).ok()?;
        let base_fee = U256::from_big_endian(&buff[..32]);
        raw.read_exact(&mut buff[..8]).ok()?;
        let txs_len = u64::from_le_bytes(buff[..8].try_into().unwrap());
        let mut txs = Vec::new();
        for _ in 0..txs_len {
            let mut tx_buff = Vec::new();
            raw.read_exact(&mut buff[..8]).ok()?;
            let tx_len = u64::from_le_bytes(buff[..8].try_into().unwrap());
            tx_buff.resize(tx_len as usize, 0);
            raw.read_exact(&mut tx_buff).ok()?;
            txs.push(qevm::tx::Tx::decode(&tx_buff, chain_id)?);
        }
        Some(Self {
            number,
            parent_hash,
            root_hash,
            timestamp,
            gas_limit,
            base_fee,
            txs,
            cached_hash: OnceCell::new(),
        })
    }
}
