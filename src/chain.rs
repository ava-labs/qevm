use std::collections::hash_map::HashMap;
use std::sync::Arc;

use crate::common::{Hash, U256};
#[cfg(feature = "actor")] pub use actor::*;

pub trait Block: Send {
    fn number(&self) -> U256;
    fn parent(&self) -> &Hash;
    fn hash(&self) -> &Hash;
}

pub struct BlockRef<B: Block>(Arc<B>);

impl<B: Block> Clone for BlockRef<B> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<B: Block> std::ops::Deref for BlockRef<B> {
    type Target = B;
    fn deref(&self) -> &B {
        &*self.0
    }
}

pub struct Chain<B: Block> {
    base: BlockRef<B>,
    /// Blocks that are currently transitively attached to the base.
    attached: HashMap<Hash, BlockRef<B>>,
    /// Blocks that are currently free floating from the base.
    orphans: HashMap<Hash, BlockRef<B>>,
    orphan_on: HashMap<Hash, Vec<Hash>>,
}

impl<B: Block> Chain<B> {
    pub fn new(base: B) -> Self {
        let base = BlockRef(Arc::new(base));
        let mut attached = HashMap::new();
        attached.insert(base.hash().clone(), base.clone());
        Chain {
            base,
            attached,
            orphans: HashMap::new(),
            orphan_on: HashMap::new(),
        }
    }

    /// Add an arbitrary block to the chain, orphan blocks will be kept until `remove_orphans` is
    /// invoked. Returns the shared reference for the added block. The blocks will be deduplicated.
    pub fn add_block(&mut self, block: B) -> BlockRef<B> {
        let block = BlockRef(Arc::new(block));
        let hash = block.hash();
        if let Some(b) = self.attached.get(hash) {
            return b.clone()
        }
        if let Some(b) = self.orphans.get(hash) {
            return b.clone()
        }
        match self.attached.get(block.parent()) {
            None => {
                // add to the waiting list
                self.orphan_on
                    .entry(block.parent().clone())
                    .or_insert_with(|| Vec::new())
                    .push(hash.clone());
                // orphan block
                self.orphans.insert(hash.clone(), block.clone());
            }
            Some(_) => {
                self.attached.insert(hash.clone(), block.clone());
                let mut q = std::collections::VecDeque::new();
                q.push_back(hash.clone());
                // recursively clear the waiting list to remove orphans
                while let Some(u) = q.pop_front() {
                    if let Some(orphans) = self.orphan_on.remove(&u) {
                        for orphan in orphans.into_iter() {
                            let o = self.orphans.remove(&orphan).unwrap();
                            self.attached.insert(orphan.clone(), o);
                            q.push_back(orphan);
                        }
                    }
                }
            }
        }
        block
    }

    /// Returns if block is connected to the base block (i.e. not dangling from the base).
    pub fn is_attached(&self, block: &Hash) -> bool {
        self.attached.contains_key(block)
    }

    pub fn get_block(&self, block: &Hash) -> Option<BlockRef<B>> {
        self.attached
            .get(block)
            .or_else(|| self.orphans.get(block))
            .map(|b| b.clone())
    }

    pub fn remove_orphans(&mut self) {
        self.orphans.clear();
        self.orphan_on.clear();
    }

    pub fn get_base(&self) -> &BlockRef<B> {
        &self.base
    }

    pub fn get_nattached(&self) -> usize {
        self.attached.len()
    }

    pub fn get_norphans(&self) -> usize {
        self.orphans.len()
    }
}

#[cfg(feature = "actor")]
mod actor {
    use std::fmt;

    use super::*;
    use actix::prelude::*;
    use hex::ToHex;
    use serde::{
        de::{self, Deserialize, Deserializer, Visitor},
        Serialize, Serializer,
    };

    use crate::common::{
        Addr, Bytes, Bytes32, FixedBytes, NullableAddr, Wei, WrappedGas,
        WrappedU256, WrappedU64, WrappedWei,
    };

    pub trait ChainStateService:
        Actor<Context = Context<Self>>
        + Handler<GetBalance>
        + Handler<GetNonce>
        + Handler<GetState>
        + Handler<GetCode>
        + Handler<GetTailBlock>
        + Handler<GetTxReceipt>
        + Handler<GetNetworkId>
        + Handler<GetBlockByHash>
        + Handler<GetBlockByNumber>
        + Handler<GetTxByHash>
        + Handler<GetChainId>
        + Handler<Call>
        + Handler<EstimateGas>
        + Handler<AddBlocks<Self::Block>>
    {
        type Block: Block;
    }

    #[derive(Message, Debug)]
    #[rtype(result = "Option<WrappedWei>")]
    pub struct GetBalance(pub Addr, pub BlockNumber);

    #[derive(Message, Debug)]
    #[rtype(result = "Option<WrappedU64>")]
    pub struct GetNonce(pub Addr, pub BlockNumber);

    #[derive(Message, Debug)]
    #[rtype(result = "Option<WrappedU256>")]
    pub struct GetState(pub Addr, pub WrappedU256, pub BlockNumber);

    #[derive(Message, Debug)]
    #[rtype(result = "Option<Bytes>")]
    pub struct GetCode(pub Addr, pub BlockNumber);

    #[derive(MessageResponse)]
    pub struct TailBlockResp(pub WrappedU256, pub Hash);

    #[derive(Message, Debug)]
    #[rtype(result = "TailBlockResp")]
    pub struct GetTailBlock;

    #[derive(Message, Debug)]
    #[rtype(result = "Option<TxReceipt>")]
    pub struct GetTxReceipt(pub Hash);

    #[derive(Message, Debug)]
    #[rtype(result = "String")]
    pub struct GetNetworkId;

    #[derive(Message, Debug)]
    #[rtype(result = "WrappedU256")]
    pub struct GetChainId;

    #[derive(Message, Debug)]
    #[rtype(result = "Option<BlockInfo>")]
    pub struct GetBlockByHash(pub Hash, pub bool);

    #[derive(Message, Debug)]
    #[rtype(result = "Option<BlockInfo>")]
    pub struct GetBlockByNumber(pub BlockNumber, pub bool);

    #[derive(Message, Debug)]
    #[rtype(result = "Option<TxResponse>")]
    pub struct GetTxByHash(pub Hash);

    #[derive(Message)]
    #[rtype(result = "()")]
    pub struct AddBlocks<B: Block>(pub Vec<B>);

    #[derive(MessageResponse)]
    pub struct WrappedTxExecResult(pub crate::core::TxExecResult);

    #[derive(Message, Debug)]
    #[rtype(result = "Option<WrappedTxExecResult>")]
    pub struct Call(pub TxArgs, pub BlockNumber);

    #[derive(Message, Debug)]
    #[rtype(result = "Option<WrappedGas>")]
    pub struct EstimateGas(pub TxArgs, pub BlockNumber);

    #[derive(Debug, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct TxArgs {
        pub from: Option<Addr>,
        pub to: Option<Addr>,
        pub gas: Option<WrappedGas>,
        pub gas_price: Option<Wei>,
        pub max_fee_per_gas: Option<Wei>,
        pub max_priority_fee_per_gas: Option<Wei>,
        pub value: Option<Wei>,
        pub data: Option<Bytes>,
    }

    #[derive(MessageResponse, Serialize, Debug)]
    #[serde(rename_all = "camelCase")]
    pub struct TxResponse {
        pub block_hash: Option<Hash>,
        pub block_number: Option<WrappedU256>,
        pub from: Addr,
        pub gas: WrappedGas,
        pub gas_price: Wei,
        pub hash: Hash,
        pub input: Bytes,
        pub nonce: WrappedU64,
        pub to: NullableAddr,
        #[serde(rename = "transactionIndex")]
        pub tx_index: WrappedU64,
        pub value: Wei,
        pub v: Bytes32,
        pub r: Bytes32,
        pub s: Bytes32,
    }

    #[derive(MessageResponse, Serialize, Debug)]
    #[serde(untagged, rename_all = "camelCase")]
    pub enum TxResponseInfo {
        Hashes(Vec<Hash>),
        Objects(Vec<TxResponse>),
    }

    impl Default for TxResponseInfo {
        fn default() -> Self {
            TxResponseInfo::Hashes(Vec::new())
        }
    }

    #[derive(MessageResponse, Serialize, Default, Debug)]
    #[serde(rename_all = "camelCase")]
    pub struct BlockInfo {
        pub number: Option<WrappedU256>,
        pub hash: Option<Hash>,
        pub parent_hash: Hash,
        pub nonce: Option<FixedBytes<8>>,
        #[serde(rename = "sha3Uncles")]
        pub sha3_uncles: Hash,
        pub logs_bloom: Option<Bloom>,
        #[serde(rename = "transactionsRoot")]
        pub tx_root: Hash,
        pub state_root: Hash,
        pub receipts_root: Hash,
        pub miner: Addr,
        pub diff: WrappedU64,
        #[serde(rename = "totalDifficulty")]
        pub total_diff: WrappedU64,
        pub extra_data: Bytes,
        pub size: WrappedU64,
        pub gas_limit: WrappedGas,
        pub gas_used: WrappedGas,
        pub timestamp: WrappedU64,
        #[serde(rename = "transactions")]
        pub txs: TxResponseInfo,
        pub uncles: Vec<Hash>,
    }

    #[derive(MessageResponse, Serialize, Default, Debug)]
    #[serde(rename_all = "camelCase")]
    pub struct TxReceipt {
        #[serde(rename = "transactionHash")]
        pub tx_hash: Hash,
        #[serde(rename = "transactionIndex")]
        pub tx_index: WrappedU64,
        pub block_hash: Hash,
        pub block_number: WrappedU256,
        pub from: Addr,
        pub to: Option<Addr>,
        #[serde(rename = "cumulativeGasUsed")]
        pub cu_gas_used: WrappedGas,
        pub gas_used: WrappedGas,
        #[serde(rename = "contractAddress")]
        pub contract_addr: Option<Addr>,
        pub logs: Vec<LogItem>,
        pub logs_bloom: Bloom,
        pub status: WrappedU64,
    }

    #[derive(Debug)]
    pub enum BlockNumber {
        Number(U256),
        Latest,
        Earliest,
        Pending,
    }

    impl<'de> Deserialize<'de> for BlockNumber {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            struct BNVisitor;
            impl<'de> Visitor<'de> for BNVisitor {
                type Value = BlockNumber;

                fn expecting(
                    &self, formatter: &mut fmt::Formatter,
                ) -> fmt::Result {
                    formatter.write_str(
                        "starting with `0x` and has even number of hex digits",
                    )
                }

                fn visit_str<E>(self, value: &str) -> Result<BlockNumber, E>
                where
                    E: de::Error,
                {
                    Ok(match value {
                        "latest" => BlockNumber::Latest,
                        "earliest" => BlockNumber::Earliest,
                        s @ _ => BlockNumber::Number(
                            crate::common::U256Visitor.visit_str(s)?,
                        ),
                    })
                }
            }
            deserializer.deserialize_identifier(BNVisitor)
        }
    }

    // TODO: LogItem is currently a dummy struct
    #[derive(Serialize, Debug)]
    pub struct LogItem;

    #[derive(Debug)]
    pub struct Bloom([u64; 32]);

    impl Default for Bloom {
        fn default() -> Self {
            Bloom([0; 32])
        }
    }

    impl Serialize for Bloom {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serializer.serialize_str(&format!(
                "0x{}",
                self.0
                    .iter()
                    .map(|x| x.to_le_bytes().encode_hex::<String>())
                    .collect::<Vec<_>>()
                    .join("")
            ))
        }
    }
}
