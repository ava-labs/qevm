use std::cell::Cell;
use std::collections::{hash_map::HashMap, hash_set::HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use bitvec::vec::BitVec;
use log::{debug, warn};
use num_traits::FromPrimitive;
use parking_lot::RwLock;

use super::call::CallFrame;
use super::memory::Memory;
use super::opcode;
use super::opcode::Opcode;
use super::params::*;
use super::{gas_checked_mul, get_data, Code, ExecError, WorldState};
use crate::common::{checked_as_u64, Addr, Bytes, Gas, Hash, Wei, U256};

/// Helper trait that adds funds transfer functions to any [WorldState] objects.
#[async_trait]
pub trait Transferable {
    async fn add_balance(&mut self, addr: &Addr, val: &Wei) -> Option<()>;
    async fn sub_balance(&mut self, addr: &Addr, val: &Wei) -> Option<()>;
    async fn transfer_balance(
        &mut self, from: &Addr, to: &Addr, val: &Wei,
    ) -> Option<()>;
}

#[async_trait]
impl<T> Transferable for T
where
    T: WorldState,
{
    async fn add_balance(&mut self, addr: &Addr, val: &Wei) -> Option<()> {
        self.set_balance(addr, &self.get_balance(addr).await.checked_add(val)?);
        Some(())
    }

    async fn sub_balance(&mut self, addr: &Addr, val: &Wei) -> Option<()> {
        self.set_balance(addr, &self.get_balance(addr).await.checked_sub(val)?);
        Some(())
    }

    async fn transfer_balance(
        &mut self, from: &Addr, to: &Addr, val: &Wei,
    ) -> Option<()> {
        self.sub_balance(from, val).await?;
        self.add_balance(to, val).await
    }
}

#[async_trait]
pub trait WorldStateExtra {
    async fn is_empty(&self, contract: &Addr) -> bool;
}

#[async_trait]
impl<T> WorldStateExtra for T
where
    T: WorldState,
{
    async fn is_empty(&self, contract: &Addr) -> bool {
        use futures::join;
        let (nonce, balance, code) = join!(
            self.get_nonce(contract),
            self.get_balance(contract),
            self.get_code(contract)
        );
        nonce == 0 &&
            balance.is_zero() &&
            code.get_hash() == Hash::empty_bytes_hash()
    }
}

/// Simple code object implementation that can be constructed from raw byte code. PlainCode is
/// standalone and caches code hash and valid jumps for the code itself.
pub struct PlainCode {
    code: Box<[u8]>,
    bitmap: BitVec,
    hash: Hash,
}

impl PlainCode {
    pub fn new(code: Box<[u8]>) -> Self {
        let bitmap = crate::common::gen_code_bitmap(&code);
        let hash = Hash::hash(&code);
        Self { code, bitmap, hash }
    }

    fn is_opcode(&self, dest: usize) -> bool {
        self.bitmap.get(dest).map(|b| *b).unwrap_or(false)
    }
}

impl Code for PlainCode {
    fn is_valid_jumpdest(&self, dest: &U256) -> bool {
        let dest = if let Some(dest) = checked_as_u64(dest) {
            dest as usize
        } else {
            return false
        };
        if self.is_opcode(dest) {
            let opcode = if let Some(opcode) = Opcode::from_u8(self.code[dest])
            {
                opcode
            } else {
                return false
            };
            if let Opcode::JumpDest = opcode {
                return true
            }
        }
        false
    }

    fn as_bytes(&self) -> &[u8] {
        &self.code
    }

    fn get_hash(&self) -> &Hash {
        &self.hash
    }
}

/// The decoded instruction.
struct Inst<'a> {
    opcode: opcode::Opcode,
    /// data being pushed by a PUSH* insruction, None if not PUSH*
    data: Option<&'a [u8]>,
    /// number used by DUP* and SWAP* instructions, ignored otherwise
    pos: usize,
}

enum TxAux {
    Sender(Addr),
    Contract(Addr),
}

pub(super) struct TxArgs<S> {
    snapshot: S,
    aux: TxAux,
}

pub(super) struct CallArgs<S> {
    snapshot: Option<S>,
    ret_off: U256,
    ret_len: U256,
}

pub(super) struct CreateArgs<S> {
    snapshot: S,
    contract_addr: Addr,
}

pub(super) enum CallType<S> {
    Tx(TxArgs<S>),
    Call(CallArgs<S>),
    Create(CreateArgs<S>),
}

/// The result of a transaction execution.
pub enum TxExecResult {
    /// The execution finishes with some returned data (and optionally, the address of created
    /// contract if it is a contract creation transaction).
    Succeeded(Bytes, Gas, Option<Addr>),
    /// The execution reverts with some returned data and error.
    Reverted(Bytes, Gas, ExecError),
}

/// A future that resolves when a part of the transaction execution becomes ready.
pub struct TxExecReady {
    ready: std::cell::Cell<bool>,
    waker: Option<std::task::Waker>,
}

impl Future for TxExecReady {
    type Output = ();
    fn poll(
        mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        match self.ready.get() {
            true => std::task::Poll::Ready(()),
            false => {
                if self.waker.is_none() {
                    self.waker = Some(cx.waker().clone())
                }
                std::task::Poll::Pending
            }
        }
    }
}

impl TxExecReady {
    fn new() -> Self {
        Self {
            ready: std::cell::Cell::new(false),
            waker: None,
        }
    }
    fn set_ready(&mut self) {
        self.ready.set(true);
        if let Some(waker) = self.waker.take() {
            waker.wake()
        }
    }

    async fn wrap<T, F: Future<Output = T>>(ready: &ReadyList, fut: F) -> T {
        let ready = ready.get_mut();
        ready.push(Self::new());
        let s = ready.last_mut().unwrap();
        let ret = fut.await;
        s.set_ready();
        ret
    }
}

// NOTE: no public methods exposed and no internal shared use between threads
unsafe impl Sync for TxExecReady {}

pub struct ReadyList(std::cell::UnsafeCell<Vec<TxExecReady>>);

impl ReadyList {
    fn new() -> Self {
        Self(std::cell::UnsafeCell::new(Vec::new()))
    }

    fn get_mut(&self) -> &mut Vec<TxExecReady> {
        unsafe { &mut *self.0.get() }
    }

    fn drain(&self) -> impl IntoIterator<Item = TxExecReady> {
        std::mem::take(self.get_mut()).into_iter()
    }
}

// NOTE: no public methods exposed and no internal shared use between threads
unsafe impl Sync for ReadyList {}

/// The on-going execution progress of a transaction.
pub struct TxExecHandle {
    pub(super) fut: Pin<Box<dyn Future<Output = TxExecResult> + Send>>,
    ready: Arc<ReadyList>,
}

impl TxExecHandle {
    /// Drain all notifiers for asynchronous events that a transaction is waiting for. When all
    /// [TxExecReady] futures resolve, the transaction is guaranteed to make some progress (and
    /// thus should be rescheduled to [Core](struct.Core.html)).
    pub fn drain_ready_list(&self) -> impl IntoIterator<Item = TxExecReady> {
        self.ready.drain()
    }
}

enum TxExecStatus {
    /// The transaction is still being executed.
    Running,
    /// The transaction finished with the result.
    Finished(TxExecResult),
}

pub struct TxAccessTuple {
    addr: Addr,
    storage_keys: Vec<Hash>,
}

type TxAccessList<'a> = &'a [TxAccessTuple];

/// AccessList is used by EIP2929 gas calculation (should be the same behavior as `accessList` in
/// `go-ethereum/core/state/access_list.go`)
struct AccessList {
    addrs: HashMap<Addr, i64>,
    slots: Vec<HashSet<Hash>>,
}

impl AccessList {
    fn new<'a, 'b, P: Iterator<Item = &'a Addr>>(
        from: &Addr, to: Option<&Addr>, precompiled: P,
        access_list: Option<TxAccessList<'b>>,
    ) -> Self {
        let mut ac = Self {
            addrs: HashMap::new(),
            slots: Vec::new(),
        };
        ac.add_addr(from);
        if let Some(to) = to {
            ac.add_addr(to);
        }
        for addr in precompiled {
            ac.add_addr(addr);
        }
        if let Some(access_list) = access_list {
            for at in access_list {
                ac.add_addr(&at.addr);
                for key in at.storage_keys.iter() {
                    ac.add_slot(&at.addr, key);
                }
            }
        }
        ac
    }
    fn contains_addr(&mut self, addr: &Addr) -> bool {
        self.addrs.contains_key(addr)
    }

    fn contains(&mut self, addr: &Addr, slot: &Hash) -> (bool, bool) {
        // returns (if address exists, if slot exists)
        match self.addrs.get(addr) {
            Some(idx) => (
                true,
                if *idx == -1 {
                    false
                } else {
                    self.slots[*idx as usize].contains(slot)
                },
            ),
            None => (false, false),
        }
    }

    fn add_addr(&mut self, addr: &Addr) -> bool {
        // returns true if the slot is changed
        match self.addrs.get_mut(addr) {
            Some(_) => false,
            None => {
                self.addrs.insert(addr.clone(), -1);
                true
            }
        }
    }

    fn add_slot(&mut self, addr: &Addr, slot: &Hash) -> (bool, bool) {
        // returns (if address is changed, if slot is changed)
        let idx = self.addrs.get_mut(addr).and_then(|v| {
            if *v == -1 {
                None
            } else {
                Some(*v)
            }
        });
        match idx {
            Some(idx) => {
                let slot_set = &mut self.slots[idx as usize];
                (false, slot_set.insert(slot.clone()))
            }
            None => {
                let addr_changed = self
                    .addrs
                    .insert(addr.clone(), self.slots.len() as i64)
                    .is_none();
                let mut slot_set = HashSet::new();
                slot_set.insert(slot.clone());
                self.slots.push(slot_set);
                (addr_changed, true)
            }
        }
    }
    // There is no need to implement delete* methods as AccessList is freshly constructed when a
    // TxExecContext is created (no journaling or reversion).
}

pub struct TxExecContext<S: WorldState + 'static> {
    call_stack: Vec<Box<CallFrame<S>>>,
    /// Top of the contract call stack, not included in `call_stack`
    cur_call: Box<CallFrame<S>>,
    state: Arc<RwLock<S>>,
    /// Last committed state (the finishing state of the last block)
    committed_state: S,
    // (From EIP-2929): "The sets are transaction-context-wide, implemented identically to other
    // transaction-scoped constructs such as the self-destruct-list and global refund counter."
    /// Global refund counter
    refund: Cell<u64>,
    /// Access list for warm/cold determination
    access_list: AccessList,
    /// Self-destruct list
    destroyed: HashSet<Addr>,
    /// Accounts have state changes during a transaction
    dirty: HashSet<Addr>,
    status: TxExecStatus,

    // the following fields are immutable throughout the execution
    origin: Addr,
    gas_price: Wei,
    env: Arc<super::TxExecEnv>,
}

macro_rules! return_without_callstack {
    ($self: expr, $args: expr, $ret: expr, $unused_gas: expr, $err: expr, $ready: expr) => {
        return match $args {
            CallType::Tx(args) => {
                $self.tx_end(args, $ret, $unused_gas, $err, $ready).await;
                Ok(())
            }
            CallType::Call(args) => {
                let c = $self.call_end(args, $ret, $unused_gas, $err);
                // When the called code is empty we won't enter a new call frame, we
                // should properly advance PC here.
                $self.advance_pc(0, false);
                c
            }
            _ => unreachable!(),
        }
    };
}

impl<S: WorldState + 'static> TxExecContext<S> {
    #[inline(always)]
    async fn balance(&mut self, ready: &ReadyList) -> Result<(), ExecError> {
        let call = &mut self.cur_call;
        let addr = call.stack.consume1()?.into();
        if call.fork >= Fork::Berlin {
            call.use_gas(GAS_WARM_STORAGE_READ_COST_BERLIN)?;
            if !self.access_list.contains_addr(&addr) {
                self.access_list.add_addr(&addr);
                call.use_gas(
                    GAS_COLD_ACCOUNT_ACCESS_COST_BERLIN -
                        GAS_WARM_STORAGE_READ_COST_BERLIN,
                )?;
            }
        } else if call.fork >= Fork::Istanbul {
            call.use_gas(GAS_BALANCE_ISTANBUL)?;
        } else if call.fork >= Fork::TangerineWhistle {
            call.use_gas(GAS_BALANCE_TANGERINE)?;
        } else {
            call.use_gas(GAS_BALANCE_FRONTIER)?;
        }
        // NOTE: _gas not used by any fork (may be used by some future fork)
        let balance =
            TxExecReady::wrap(ready, self.state.read().get_balance(&addr))
                .await;
        call.stack.push(balance.into())
    }

    #[inline(always)]
    fn origin(&mut self) -> Result<(), ExecError> {
        let call = &mut self.cur_call;
        call.use_gas(GAS_QUICK)?;
        call.stack.push(self.origin.clone().into())
    }

    #[inline(always)]
    async fn ext_code_size(
        &mut self, ready: &ReadyList,
    ) -> Result<(), ExecError> {
        let call = &mut self.cur_call;
        let addr = call.stack.consume1()?.into();
        if call.fork >= Fork::Berlin {
            call.use_gas(GAS_WARM_STORAGE_READ_COST_BERLIN)?;
            if !self.access_list.contains_addr(&addr) {
                self.access_list.add_addr(&addr);
                call.use_gas(
                    GAS_COLD_ACCOUNT_ACCESS_COST_BERLIN -
                        GAS_WARM_STORAGE_READ_COST_BERLIN,
                )?;
            }
        } else if call.fork >= Fork::TangerineWhistle {
            call.use_gas(GAS_EXT_CODE_SIZE_TANGERINE)?;
        } else {
            call.use_gas(GAS_EXT_CODE_SIZE_FRONTIER)?;
        }
        let code =
            TxExecReady::wrap(ready, self.state.read().get_code(&addr)).await;
        call.stack.push(code.as_bytes().len().into())
    }

    #[inline(always)]
    async fn ext_code_copy(
        &mut self, ready: &ReadyList,
    ) -> Result<(), ExecError> {
        let call = &mut self.cur_call;
        let (addr, mem_off, code_off, len) = call.stack.consume4()?;
        let addr = addr.into();
        if call.fork >= Fork::Berlin {
            call.use_gas(GAS_WARM_STORAGE_READ_COST_BERLIN)?;
            if !self.access_list.contains_addr(&addr) {
                self.access_list.add_addr(&addr);
                call.use_gas(
                    GAS_COLD_ACCOUNT_ACCESS_COST_BERLIN -
                        GAS_WARM_STORAGE_READ_COST_BERLIN,
                )?;
            }
        } else if call.fork >= Fork::TangerineWhistle {
            call.use_gas(GAS_EXT_BASE_TANGERINE)?;
        } else {
            call.use_gas(GAS_EXT_BASE_FRONTIER)?;
        }
        let code_off = checked_as_u64(&code_off).unwrap_or(u64::MAX);
        let code =
            TxExecReady::wrap(ready, self.state.read().get_code(&addr)).await;
        let (mem, mem_gas) = call.memory.get_slice_mut(mem_off, len)?;
        let len = len.as_u64();
        mem.copy_from_slice(&get_data(code.as_bytes(), code_off, len));
        call.use_gas(mem_gas)?;
        call.use_gas(gas_checked_mul(Memory::to_word_size(len), GAS_COPY_WORD)?)
    }

    #[inline(always)]
    async fn ext_code_hash(
        &mut self, ready: &ReadyList,
    ) -> Result<(), ExecError> {
        let call = &mut self.cur_call;
        if call.fork < Fork::Constantinople {
            return Err(ExecError::InvalidOpcode)
        }
        let addr = call.stack.consume1()?.into();
        if call.fork >= Fork::Berlin {
            call.use_gas(GAS_WARM_STORAGE_READ_COST_BERLIN)?;
            if !self.access_list.contains_addr(&addr) {
                self.access_list.add_addr(&addr);
                call.use_gas(
                    GAS_COLD_ACCOUNT_ACCESS_COST_BERLIN -
                        GAS_WARM_STORAGE_READ_COST_BERLIN,
                )?;
            }
        } else if call.fork >= Fork::Istanbul {
            call.use_gas(GAS_EXT_CODE_HASH_ISTANBUL)?;
        } else {
            call.use_gas(GAS_EXT_CODE_HASH_CONSTANTINOPLE)?;
        }
        let code =
            TxExecReady::wrap(ready, self.state.read().get_code(&addr)).await;
        call.stack.push(code.get_hash().clone().into())
    }

    #[inline(always)]
    async fn block_hash(&mut self, ready: &ReadyList) -> Result<(), ExecError> {
        let call = &mut self.cur_call;
        call.use_gas(GAS_EXT)?;
        let number = call.stack.consume1()?;
        call.stack.push(if let Some(n) = checked_as_u64(&number) {
            // NOTE: geth just forces into Uint64, so here we also just take the low bits
            let upper = self.env.block.number.low_u64();
            let lower = if upper >= 256 { upper - 256 } else { 0 };
            if n < lower || n >= upper {
                U256::zero()
            } else {
                TxExecReady::wrap(ready, (self.env.block_hash_getter)(n))
                    .await
                    .into()
            }
        } else {
            U256::zero()
        })
    }

    #[inline(always)]
    fn gas_price(&mut self) -> Result<(), ExecError> {
        let call = &mut self.cur_call;
        call.use_gas(GAS_QUICK)?;
        call.stack.push(self.gas_price.clone().into())
    }

    #[inline(always)]
    fn coinbase(&mut self) -> Result<(), ExecError> {
        let call = &mut self.cur_call;
        call.use_gas(GAS_QUICK)?;
        call.stack.push(self.env.block.coinbase.clone().into())
    }

    #[inline(always)]
    fn timestamp(&mut self) -> Result<(), ExecError> {
        let call = &mut self.cur_call;
        call.use_gas(GAS_QUICK)?;
        call.stack.push(self.env.block.timestamp)
    }

    #[inline(always)]
    fn number(&mut self) -> Result<(), ExecError> {
        let call = &mut self.cur_call;
        call.use_gas(GAS_QUICK)?;
        call.stack.push(self.env.block.number)
    }

    #[inline(always)]
    fn difficulty(&mut self) -> Result<(), ExecError> {
        let call = &mut self.cur_call;
        call.use_gas(GAS_QUICK)?;
        call.stack.push(self.env.block.difficulty)
    }

    #[inline(always)]
    fn gas_limit(&mut self) -> Result<(), ExecError> {
        let call = &mut self.cur_call;
        call.use_gas(GAS_QUICK)?;
        call.stack.push(self.env.block.gas_limit.into())
    }

    #[inline(always)]
    fn chain_id(&mut self) -> Result<(), ExecError> {
        let call = &mut self.cur_call;
        if call.fork < Fork::Istanbul {
            return Err(ExecError::InvalidOpcode)
        }
        call.use_gas(GAS_QUICK)?;
        call.stack.push(self.env.chain_id)
    }

    #[inline(always)]
    async fn self_balance(
        &mut self, ready: &ReadyList,
    ) -> Result<(), ExecError> {
        let call = &mut self.cur_call;
        if call.fork < Fork::Istanbul {
            return Err(ExecError::InvalidOpcode)
        }
        call.use_gas(GAS_FAST)?;
        // NOTE: maybe should use a pre-cached self balance instead of fetching it from the store?
        call.stack.push(
            TxExecReady::wrap(
                ready,
                self.state.read().get_balance(&call.callee),
            )
            .await
            .into(),
        )
    }

    #[inline(always)]
    fn base_fee(&mut self) -> Result<(), ExecError> {
        let call = &mut self.cur_call;
        if call.fork < Fork::London {
            return Err(ExecError::InvalidOpcode)
        }
        call.use_gas(GAS_QUICK)?;
        call.stack.push(self.env.block.base_fee)
    }

    #[inline(always)]
    async fn sload(&mut self, ready: &ReadyList) -> Result<(), ExecError> {
        let call = &mut self.cur_call;
        let key = call.stack.consume1()?.into();
        call.use_gas(if call.fork >= Fork::Berlin {
            let (_, slot_exist) = self.access_list.contains(&call.callee, &key);
            if !slot_exist {
                self.access_list.add_slot(&call.callee, &key);
                GAS_COLD_SLOAD_COST_BERLIN
            } else {
                GAS_WARM_STORAGE_READ_COST_BERLIN
            }
        } else if call.fork >= Fork::Istanbul {
            GAS_SLOAD_ISTANBUL
        } else if call.fork >= Fork::TangerineWhistle {
            GAS_SLOAD_TANGERINE
        } else {
            GAS_SLOAD_FRONTIER
        })?;
        let val = TxExecReady::wrap(
            ready,
            self.state.read().get_state(&call.callee, &key),
        )
        .await;
        call.stack.push(val)
    }

    #[inline(always)]
    fn jump(&mut self) -> Result<(), ExecError> {
        let call = &mut self.cur_call;
        call.use_gas(GAS_MID)?;
        let dest = call.stack.consume1()?;
        if !call.code.is_valid_jumpdest(&dest) {
            return Err(ExecError::InvalidJump)
        }
        // pc will be increased every iteration in the core loop
        call.pc = dest.as_u64() - 1;
        Ok(())
    }

    #[inline(always)]
    fn jumpi(&mut self) -> Result<(), ExecError> {
        let call = &mut self.cur_call;
        call.use_gas(GAS_SLOW)?;
        let (dest, cond) = call.stack.consume2()?;
        if !cond.is_zero() {
            if !call.code.is_valid_jumpdest(&dest) {
                return Err(ExecError::InvalidJump)
            }
            call.pc = dest.as_u64() - 1;
        }
        Ok(())
    }

    #[inline(always)]
    async fn sstore_istanbul_gas(
        &mut self, key: &Hash, val: &U256, ready: &ReadyList,
    ) -> Result<(), ExecError> {
        // EIP2200
        if self.cur_call.unused_gas <= GAS_SSTORE_SENTRY_ISTANBUL {
            // not enough gas for re-entrancy sentry
            return Err(ExecError::OutOfGas)
        }
        let cur = TxExecReady::wrap(
            ready,
            self.state.read().get_state(&self.cur_call.callee, key),
        )
        .await;
        if cur == *val {
            // noop (1)
            self.cur_call.use_gas(GAS_SLOAD_ISTANBUL)?;
            return Ok(())
        }
        let orig = TxExecReady::wrap(
            ready,
            self.committed_state.get_state(&self.cur_call.callee, key),
        )
        .await;
        if orig == cur {
            if orig.is_zero() {
                // create slot (2.1.1)
                self.cur_call.use_gas(GAS_SSTORE_SET_ISTANBUL)?;
                return Ok(())
            }
            if val.is_zero() {
                // delete slot (2.1.2b)
                self.add_refund(GAS_SSTORE_REFUND_ISTANBUL);
            }
            self.cur_call.use_gas(GAS_SSTORE_RESET_ISTANBUL)?;
            return Ok(())
        }
        if !orig.is_zero() {
            if cur.is_zero() {
                // recreate slot (2.2.1.1)
                self.sub_refund(GAS_SSTORE_REFUND_ISTANBUL);
            } else if val.is_zero() {
                // delete slot (2.2.1.2)
                self.add_refund(GAS_SSTORE_REFUND_ISTANBUL);
            }
        }
        if orig == *val {
            self.add_refund(if orig.is_zero() {
                // reset to original inexistent slot (2.2.2.1)
                GAS_SSTORE_SET_ISTANBUL - GAS_SLOAD_ISTANBUL
            } else {
                // reset to original existing slot (2.2.2.2)
                GAS_SSTORE_RESET_ISTANBUL - GAS_SLOAD_ISTANBUL
            })
        }
        // dirty update (2.2)
        self.cur_call.use_gas(GAS_SLOAD_ISTANBUL)?;
        Ok(())
    }

    #[inline(always)]
    async fn sstore_berlin_london_gas(
        &mut self, key: &Hash, val: &U256, clearing_refund: u64,
        ready: &ReadyList,
    ) -> Result<(), ExecError> {
        if self.cur_call.unused_gas <= GAS_SSTORE_SENTRY_ISTANBUL {
            // not enough gas for re-entrancy sentry
            return Err(ExecError::OutOfGas)
        }
        let (addr_exist, slot_exist) =
            self.access_list.contains(&self.cur_call.callee, key);
        let mut cost = 0;
        if !slot_exist {
            cost = GAS_COLD_SLOAD_COST_BERLIN;
            self.access_list.add_slot(&self.cur_call.callee, key);
            assert!(addr_exist);
        }
        let cur = TxExecReady::wrap(
            ready,
            self.state.read().get_state(&self.cur_call.callee, key),
        )
        .await;
        if cur == *val {
            // noop (1)
            self.cur_call
                .use_gas(cost + GAS_WARM_STORAGE_READ_COST_BERLIN)?;
            return Ok(())
        }
        let orig = TxExecReady::wrap(
            ready,
            self.committed_state.get_state(&self.cur_call.callee, key),
        )
        .await;
        if orig == cur {
            if orig.is_zero() {
                // create slot (2.1.1)
                self.cur_call.use_gas(cost + GAS_SSTORE_SET_ISTANBUL)?;
                return Ok(())
            }
            if val.is_zero() {
                // delete slot (2.1.2b)
                self.add_refund(clearing_refund)
            }
            self.cur_call.use_gas(
                cost + (GAS_SSTORE_RESET_ISTANBUL - GAS_COLD_SLOAD_COST_BERLIN),
            )?;
            return Ok(())
        }
        if !orig.is_zero() {
            if cur.is_zero() {
                self.sub_refund(clearing_refund);
            } else if val.is_zero() {
                self.add_refund(clearing_refund);
            }
        }
        if orig == *val {
            if orig.is_zero() {
                self.add_refund(
                    GAS_SSTORE_SET_ISTANBUL - GAS_WARM_STORAGE_READ_COST_BERLIN,
                );
            } else {
                self.add_refund(
                    GAS_SSTORE_RESET_ISTANBUL -
                        GAS_COLD_SLOAD_COST_BERLIN -
                        GAS_WARM_STORAGE_READ_COST_BERLIN,
                );
            }
        }
        self.cur_call
            .use_gas(cost + GAS_WARM_STORAGE_READ_COST_BERLIN)?;
        Ok(())
    }

    #[inline(always)]
    async fn sstore(&mut self, ready: &ReadyList) -> Result<(), ExecError> {
        let call = &mut self.cur_call;
        if call.read_only {
            return Err(ExecError::WriteProtection)
        }
        let (key, val) = call.stack.consume2()?;
        let key = key.into();

        let fork = call.fork;
        // gas calculation
        if fork >= Fork::Berlin {
            self.sstore_berlin_london_gas(
                &key,
                &val,
                if fork >= Fork::London {
                    GAS_SSTORE_REFUND_LONDON
                } else {
                    GAS_SSTORE_REFUND_ISTANBUL
                },
                ready,
            )
            .await?;
        } else if call.fork >= Fork::Istanbul {
            self.sstore_istanbul_gas(&key, &val, ready).await?;
        } else if call.fork >= Fork::Constantinople {
            // EIP1283 is removed
            let cur = TxExecReady::wrap(
                ready,
                self.state.read().get_state(&call.callee, &key),
            )
            .await;
            let gas = if cur.is_zero() && !val.is_zero() {
                GAS_SSTORE_SET
            } else if !cur.is_zero() && val.is_zero() {
                self.add_refund(GAS_SSTORE_REFUND);
                GAS_SSTORE_CLEAR
            } else {
                GAS_SSTORE_RESET
            };
            self.cur_call.use_gas(gas)?;
        }
        self.state
            .write()
            .set_state(&self.cur_call.callee, &key, &val);
        self.dirty.insert(self.cur_call.callee.clone());
        Ok(())
    }

    #[inline(always)]
    fn log(&mut self, num: usize) -> Result<(), ExecError> {
        let call = &mut self.cur_call;
        if call.read_only {
            return Err(ExecError::WriteProtection)
        }
        call.use_gas(GAS_LOG)?;
        call.use_gas(gas_checked_mul(num as u64, GAS_LOG_TOPIC)?)?;
        let (off, length) = call.stack.consume2()?;
        call.use_gas(gas_checked_mul(
            checked_as_u64(&length).ok_or(ExecError::OutOfMemory)?,
            GAS_LOG_DATA,
        )?)?;

        let mut topics: Vec<Hash> = Vec::new();
        for _ in 0..num {
            topics.push(call.stack.consume1()?.into());
        }
        let (mem, mem_gas) = call.memory.get_slice(off, length)?;
        self.state.write().add_log(
            &call.callee,
            &topics[..],
            mem,
            &self.env.block.number,
        );
        call.use_gas(mem_gas)
    }

    #[inline(always)]
    async fn create_(
        &mut self, contract_addr: Addr, caller: Addr, code: Box<[u8]>,
        value: Wei, gas: Gas, args: CallType<S>, ready: &ReadyList,
    ) -> Result<(), ExecError> {
        let mut state = self.state.write();
        let callee = contract_addr.clone();
        if TxExecReady::wrap(ready, state.get_balance(&caller)).await < value {
            return Err(ExecError::InsufficientBalance)
        }
        // add to the ready list to mitigate scheduling overhead...
        let (nonce_w, code_w, cnonce_w) = (
            TxExecReady::wrap(ready, state.get_nonce(&caller)),
            TxExecReady::wrap(ready, state.get_code(&contract_addr)),
            TxExecReady::wrap(ready, state.get_nonce(&contract_addr)),
        );
        // ...so that these awaits should just resolve
        let code_ = code_w.await;
        let nonce = nonce_w
            .await
            .checked_add(1)
            .ok_or(ExecError::NonceIntOverflow)?;
        let cnonce = cnonce_w.await;

        state.set_nonce(&caller, nonce);
        if self.cur_call.fork >= Fork::Berlin {
            self.access_list.add_addr(&contract_addr);
        }
        let h = code_.get_hash();
        if cnonce != 0 {
            return Err(ExecError::ContractAddrCollision)
        }
        if h != Hash::zero() && h != Hash::empty_bytes_hash() {
            return Err(ExecError::ContractAddrCollision)
        }
        state.create_account(&contract_addr).await;
        if self.cur_call.fork >= Fork::SpuriousDragon {
            // EIP158
            state.set_nonce(&contract_addr, 1);
        }
        // transfer into new contract account
        TxExecReady::wrap(
            ready,
            state.transfer_balance(&caller, &callee, &value),
        )
        .await
        .unwrap();
        drop(state);

        //record modified address
        self.dirty.insert(contract_addr.clone());
        self.dirty.insert(caller.clone());
        self.dirty.insert(callee.clone());

        let code = Arc::new(PlainCode::new(code)) as Arc<dyn Code>;
        self.call_push(
            code,
            Vec::new().into(),
            value,
            callee,
            caller,
            args,
            gas,
            false,
        );
        Ok(())
    }

    #[inline(always)]
    async fn create_begin(
        &mut self, ready: &ReadyList,
    ) -> Result<(), ExecError> {
        self.cur_call.use_gas(GAS_CREATE)?;
        if self.cur_call.read_only {
            return Err(ExecError::WriteProtection)
        }
        if self.call_depth() == MAX_CALL_DEPTH {
            return Err(ExecError::Depth)
        }

        let call = &mut self.cur_call;
        let (value, off, len) = call.stack.consume3()?;
        let (input, mem_gas) = call.memory.get_slice(off, len)?;
        let code = input.to_vec().into_boxed_slice();
        call.use_gas(mem_gas)?;

        // EIP150
        let mut gas_quota = call.unused_gas;
        if call.fork >= Fork::TangerineWhistle {
            gas_quota -= gas_quota / 64;
        }
        // borrow the gas for the nested call, whose residual will be returned when the nested call
        // is finished
        call.use_gas(gas_quota)?;

        let caller = call.callee.clone();
        let value = value.into();

        let state = self.state.read();
        let nonce = TxExecReady::wrap(ready, state.get_nonce(&caller)).await;
        let snapshot = state.snapshot();
        let contract_addr = crate::common::create_addr(&caller, nonce);
        let args = CallType::Create(CreateArgs {
            snapshot,
            contract_addr: contract_addr.clone(),
        });
        drop(state);
        self.create_(contract_addr, caller, code, value, gas_quota, args, ready)
            .await
    }

    #[inline(always)]
    fn create_post_processing(
        &mut self, contract_addr: &Addr, data: &Bytes,
        ret: Result<(), ExecError>,
    ) -> Result<(), ExecError> {
        let call = &mut self.cur_call;
        let data_len = data.len();
        if let Err(err) = ret {
            if let ExecError::Reverted = err {
            } else {
                call.use_gas(call.unused_gas)?;
            }
        } else {
            if call.fork >= Fork::SpuriousDragon && data_len > MAX_CODE_SIZE {
                // EIP158
                return Err(ExecError::MaxCodeSizeExceeded)
            }
            if call.fork >= Fork::London && data_len >= 1 && data[0] == 0xef {
                // EIP3541
                return Err(ExecError::InvalidCode)
            }
            call.use_gas(gas_checked_mul(data.len() as u64, GAS_CREATE_DATA)?)
                .map_err(|_| ExecError::CodeStoreOutOfGas)?;
            self.state.write().set_code(contract_addr, data);
            self.dirty.insert(contract_addr.clone());
        }
        Ok(())
    }

    #[inline(always)]
    fn create_end(
        &mut self, args: CreateArgs<S>, data: Bytes, unused_gas: Gas,
        ret: Result<(), ExecError>,
    ) -> Result<(), ExecError> {
        if ret.is_err() {
            self.state.write().rollback(args.snapshot);
        }
        let ret = self.create_post_processing(&args.contract_addr, &data, ret);
        let call = &mut self.cur_call;
        call.unused_gas += unused_gas;
        call.stack.push(match ret {
            Err(_) => U256::zero(),
            Ok(_) => args.contract_addr.clone().into(),
        })?;
        call.last_returned = if let Err(ExecError::Reverted) = ret {
            data
        } else {
            Bytes::empty()
        };
        Ok(())
    }

    #[inline(always)]
    fn gas_call_common(&mut self, call_cost: &U256) -> Result<Gas, ExecError> {
        let call = &mut self.cur_call;
        if call.fork >= Fork::TangerineWhistle {
            // EIP150
            let gas = call.unused_gas - call.unused_gas / 64;
            match checked_as_u64(call_cost) {
                Some(cost) => {
                    if gas < cost {
                        return Ok(gas)
                    }
                }
                None => return Ok(gas),
            }
        }
        match checked_as_u64(call_cost) {
            None => Err(ExecError::GasIntOverflow),
            Some(cost) => Ok(cost),
        }
    }

    #[inline(always)]
    fn use_gas_call_common_berlin(
        &mut self, addr: &Addr,
    ) -> Result<(), ExecError> {
        let call = &mut self.cur_call;
        if call.fork >= Fork::Berlin && !self.access_list.contains_addr(addr) {
            self.access_list.add_addr(addr);
            call.use_gas(
                GAS_COLD_ACCOUNT_ACCESS_COST_BERLIN -
                    GAS_WARM_STORAGE_READ_COST_BERLIN,
            )?;
        }
        Ok(())
    }

    #[inline(always)]
    async fn use_gas_call(
        &mut self, val: &U256, addr: &Addr, gas: &U256, mem_gas: Gas,
        ready: &ReadyList,
    ) -> Result<Gas, ExecError> {
        self.use_gas_call_common_berlin(addr)?;
        let call = &mut self.cur_call;
        if !val.is_zero() {
            if call.fork >= Fork::SpuriousDragon {
                // EIP158
                if TxExecReady::wrap(ready, self.state.read().is_empty(addr))
                    .await
                {
                    call.use_gas(GAS_CALL_NEW_ACCOUNT)?;
                }
            }
            call.use_gas(GAS_CALL_VALUE_TRANS)?;
        }
        call.use_gas(mem_gas)?;
        let gas_tmp = self.gas_call_common(gas)?;
        self.cur_call.use_gas(gas_tmp)?;
        Ok(gas_tmp)
    }

    #[inline(always)]
    fn use_gas_delegate_call(
        &mut self, addr: &Addr, gas: &U256, mem_gas: Gas,
    ) -> Result<Gas, ExecError> {
        self.use_gas_call_common_berlin(addr)?;
        self.cur_call.use_gas(mem_gas)?;
        let gas_tmp = self.gas_call_common(gas)?;
        self.cur_call.use_gas(gas_tmp)?;
        Ok(gas_tmp)
    }

    #[inline(always)]
    fn use_gas_call_code(
        &mut self, val: &U256, addr: &Addr, gas: &U256, mem_gas: Gas,
    ) -> Result<Gas, ExecError> {
        self.use_gas_call_common_berlin(addr)?;
        let call = &mut self.cur_call;
        if !val.is_zero() {
            call.use_gas(GAS_CALL_VALUE_TRANS)?;
        }
        call.use_gas(mem_gas)?;
        let gas_tmp = self.gas_call_common(gas)?;
        self.cur_call.use_gas(gas_tmp)?;
        Ok(gas_tmp)
    }

    #[inline(always)]
    fn use_gas_static_call(
        &mut self, addr: &Addr, gas: &U256, mem_gas: Gas,
    ) -> Result<Gas, ExecError> {
        self.use_gas_call_common_berlin(addr)?;
        // same as use_gas_delegate_call, but keep it in case it changes in the future
        let call = &mut self.cur_call;
        call.use_gas(mem_gas)?;
        let gas_tmp = self.gas_call_common(gas)?;
        self.cur_call.use_gas(gas_tmp)?;
        Ok(gas_tmp)
    }

    async fn call_(
        &mut self, callee: Addr, caller: Addr, input: Box<[u8]>, gas: Gas,
        value: Wei, args: CallType<S>, ready: &ReadyList,
    ) -> Result<(), ExecError> {
        let mut state = self.state.write();
        if self.call_depth() == MAX_CALL_DEPTH {
            return Err(ExecError::Depth)
        }

        if TxExecReady::wrap(ready, state.get_balance(&caller)).await < value {
            return Err(ExecError::InsufficientBalance)
        }

        // record modified address
        self.dirty.insert(caller.clone());
        self.dirty.insert(callee.clone());

        let precompiled = self.env.precompiled_contracts.get(&callee);
        if !TxExecReady::wrap(ready, state.exist(&callee)).await {
            // EIP158
            if precompiled.is_none() &&
                self.cur_call.fork >= Fork::SpuriousDragon &&
                value.is_zero()
            {
                drop(state);
                return_without_callstack!(
                    self,
                    args,
                    Bytes::empty(),
                    gas,
                    Ok(()),
                    ready
                );
            }
            state.create_account(&callee).await;
        }
        // transfer into the callee
        TxExecReady::wrap(
            ready,
            state.transfer_balance(&caller, &callee, &value),
        )
        .await
        .unwrap();

        match precompiled {
            None => {
                let code =
                    TxExecReady::wrap(ready, state.get_code(&callee)).await;
                drop(state);
                if !code.as_bytes().is_empty() {
                    self.call_push(
                        code, input, value, callee, caller, args, gas, false,
                    );
                } else {
                    return_without_callstack!(
                        self,
                        args,
                        Bytes::empty(),
                        gas,
                        Ok(()),
                        ready
                    );
                }
            }
            Some(contract) => {
                drop(state);
                let (ret, err) = contract.run(&input);
                return_without_callstack!(self, args, ret, gas, err, ready);
            }
        }
        Ok(())
    }

    #[inline(always)]
    async fn call_begin(&mut self, ready: &ReadyList) -> Result<(), ExecError> {
        let call = &mut self.cur_call;
        call.use_gas(if call.fork >= Fork::Berlin {
            GAS_WARM_STORAGE_READ_COST_BERLIN
        } else if call.fork >= Fork::TangerineWhistle {
            GAS_CALL_TANGERINE
        } else {
            GAS_CALL_FRONTIER
        })?;

        let (gas, addr, val) = call.stack.consume3()?;
        let (in_off, in_len) = call.stack.consume2()?;
        let (ret_off, ret_len) = call.stack.consume2()?;
        let (input, mem_gas) = call.memory.get_slice(in_off, in_len)?;
        let input = input.to_vec().into();
        let addr = addr.into();
        if call.read_only && !val.is_zero() {
            return Err(ExecError::WriteProtection)
        }
        let mut gas_quota =
            self.use_gas_call(&val, &addr, &gas, mem_gas, ready).await?;
        if val.is_zero() {
            gas_quota += GAS_CALL_STIPEND
        }

        let callee = addr;
        let caller = self.cur_call.callee.clone();
        let value = val.into();

        let snapshot = Some(self.state.read().snapshot());
        let args = CallType::Call(CallArgs {
            snapshot,
            ret_off,
            ret_len,
        });
        self.call_(callee, caller, input, gas_quota, value, args, ready)
            .await
    }

    #[inline(always)]
    fn call_end(
        &mut self, args: CallArgs<S>, data: Bytes, mut unused_gas: Gas,
        ret: Result<(), ExecError>,
    ) -> Result<(), ExecError> {
        if ret.is_err() {
            if let Some(snapshot) = args.snapshot {
                self.state.write().rollback(snapshot)
            }
        }
        let call = &mut self.cur_call;
        debug!("Last Error: {:?}", ret);
        match ret {
            Ok(()) | Err(ExecError::Reverted) => {
                call.memory.set(args.ret_off, args.ret_len, &data)?;
            }
            _ => unused_gas = 0,
        }
        call.unused_gas += unused_gas;
        call.stack.push(match ret.is_err() {
            true => U256::zero(),
            false => U256::one(),
        })?;
        call.last_returned = data;
        Ok(())
    }

    #[inline(always)]
    async fn delegate_call_begin(
        &mut self, ready: &ReadyList,
    ) -> Result<(), ExecError> {
        if self.cur_call.fork < Fork::Homestead {
            return Err(ExecError::InvalidOpcode)
        }
        if self.call_depth() == MAX_CALL_DEPTH {
            return Err(ExecError::Depth)
        }
        let call = &mut self.cur_call;
        call.use_gas(if call.fork >= Fork::Berlin {
            GAS_WARM_STORAGE_READ_COST_BERLIN
        } else if call.fork >= Fork::TangerineWhistle {
            GAS_CALL_TANGERINE
        } else {
            GAS_CALL_FRONTIER
        })?;

        let (gas, addr) = call.stack.consume2()?;
        let (in_off, in_len) = call.stack.consume2()?;
        let (ret_off, ret_len) = call.stack.consume2()?;
        let (input, mem_gas) = call.memory.get_slice(in_off, in_len)?;
        let input = input.to_vec().into();
        let addr = addr.into();
        let gas_quota = self.use_gas_delegate_call(&addr, &gas, mem_gas)?;

        let callee = self.cur_call.callee.clone(); // inherit from the caller
        let caller = self.cur_call.caller.clone(); // inherit from the caller
        let value = self.cur_call.value.clone(); // inherit from the caller

        let state = self.state.read();
        let snapshot = Some(state.snapshot());
        let args = CallArgs {
            snapshot,
            ret_off,
            ret_len,
        };
        let precompiled = self.env.precompiled_contracts.get(&callee);
        match precompiled {
            None => {
                let code =
                    TxExecReady::wrap(ready, state.get_code(&addr)).await;
                drop(state);
                self.call_push(
                    code,
                    input,
                    value,
                    callee,
                    caller,
                    CallType::Call(args),
                    gas_quota,
                    false,
                );
            }
            Some(contract) => {
                drop(state);
                let (ret, err) = contract.run(&input);
                return_without_callstack!(
                    self,
                    CallType::Call(args),
                    ret,
                    gas_quota,
                    err,
                    ready
                );
            }
        }
        Ok(())
    }

    #[inline(always)]
    async fn create2_begin(
        &mut self, ready: &ReadyList,
    ) -> Result<(), ExecError> {
        if self.cur_call.fork < Fork::Constantinople {
            return Err(ExecError::InvalidOpcode)
        }
        if self.cur_call.read_only {
            return Err(ExecError::WriteProtection)
        }
        if self.call_depth() == MAX_CALL_DEPTH {
            return Err(ExecError::Depth)
        }

        let call = &mut self.cur_call;
        call.use_gas(GAS_CREATE2)?;

        let (value, off, len, salt) = call.stack.consume4()?;
        let (input, mem_gas) = call.memory.get_slice(off, len)?;
        let code = input.to_vec().into_boxed_slice();
        call.use_gas(mem_gas)?;
        call.use_gas(gas_checked_mul(
            Memory::to_word_size(len.as_u64()),
            GAS_SHA3_WORD,
        )?)?;

        // EIP150
        let mut gas_quota = call.unused_gas;
        gas_quota -= gas_quota / 64;
        call.use_gas(gas_quota)?;

        let caller = call.callee.clone();
        let value = value.into();

        use sha3::Digest;
        let salt: crate::common::Bytes32 = (&salt).into();
        let contract_addr = crate::common::create_addr2(
            &caller,
            &salt[..],
            sha3::Keccak256::digest(&code).as_slice(),
        );
        let args = CallType::Create(CreateArgs {
            snapshot: self.state.read().snapshot(),
            contract_addr: contract_addr.clone(),
        });
        self.create_(contract_addr, caller, code, value, gas_quota, args, ready)
            .await
    }

    #[inline(always)]
    async fn call_code_begin(
        &mut self, ready: &ReadyList,
    ) -> Result<(), ExecError> {
        if self.call_depth() == MAX_CALL_DEPTH {
            return Err(ExecError::Depth)
        }

        let call = &mut self.cur_call;
        call.use_gas(if call.fork >= Fork::Berlin {
            GAS_WARM_STORAGE_READ_COST_BERLIN
        } else if call.fork >= Fork::TangerineWhistle {
            GAS_CALL_TANGERINE
        } else {
            GAS_CALL_FRONTIER
        })?;

        let (gas, addr, val) = call.stack.consume3()?;
        let (in_off, in_len) = call.stack.consume2()?;
        let (ret_off, ret_len) = call.stack.consume2()?;
        let (input, mem_gas) = call.memory.get_slice(in_off, in_len)?;
        let input = input.to_vec().into();
        let addr = addr.into();
        let mut gas_quota =
            self.use_gas_call_code(&val, &addr, &gas, mem_gas)?;
        if val.is_zero() {
            gas_quota += GAS_CALL_STIPEND
        }

        let callee = self.cur_call.callee.clone();
        let caller = callee.clone();
        let value = val.into();

        let state = self.state.read();
        let snapshot = Some(state.snapshot());
        let args = CallArgs {
            snapshot,
            ret_off,
            ret_len,
        };

        if TxExecReady::wrap(ready, state.get_balance(&caller)).await < value {
            return Err(ExecError::InsufficientBalance)
        }

        let precompiled = self.env.precompiled_contracts.get(&callee);
        match precompiled {
            None => {
                let code =
                    TxExecReady::wrap(ready, state.get_code(&addr)).await;
                drop(state);
                self.call_push(
                    code,
                    input,
                    value,
                    callee,
                    caller,
                    CallType::Call(args),
                    gas_quota,
                    false,
                );
            }
            Some(contract) => {
                drop(state);
                let (ret, err) = contract.run(&input);
                return_without_callstack!(
                    self,
                    CallType::Call(args),
                    ret,
                    gas_quota,
                    err,
                    ready
                );
            }
        }
        Ok(())
    }

    #[inline(always)]
    async fn return_(&mut self, ready: &ReadyList) -> Result<(), ExecError> {
        let call = &mut self.cur_call;
        let (off, len) = call.stack.consume2()?;
        let (retval, mem_gas) = call.memory.get_slice(off, len)?;
        let retval = retval.to_vec().into();
        call.use_gas(mem_gas)?;
        self.finish_call(retval, Ok(()), ready).await
    }

    #[inline(always)]
    async fn static_call_begin(
        &mut self, ready: &ReadyList,
    ) -> Result<(), ExecError> {
        if self.cur_call.fork < Fork::Byzantium {
            return Err(ExecError::InvalidOpcode)
        }
        if self.call_depth() == MAX_CALL_DEPTH {
            return Err(ExecError::Depth)
        }

        let call = &mut self.cur_call;
        call.use_gas(if call.fork >= Fork::Berlin {
            GAS_WARM_STORAGE_READ_COST_BERLIN
        } else {
            GAS_CALL_TANGERINE
        })?;

        let (gas, addr) = call.stack.consume2()?;
        let (in_off, in_len) = call.stack.consume2()?;
        let (ret_off, ret_len) = call.stack.consume2()?;
        let (input, mem_gas) = call.memory.get_slice(in_off, in_len)?;
        let input = input.to_vec().into();
        let addr = addr.into();
        let gas_quota = self.use_gas_static_call(&addr, &gas, mem_gas)?;

        let callee = addr;
        let caller = self.cur_call.callee.clone();
        let value = Wei::zero().clone();

        let state = self.state.read();
        let args = CallArgs {
            snapshot: None,
            ret_off,
            ret_len,
        };

        let precompiled = self.env.precompiled_contracts.get(&callee);
        match precompiled {
            None => {
                let code =
                    TxExecReady::wrap(ready, state.get_code(&callee)).await;
                drop(state);
                self.call_push(
                    code,
                    input,
                    value,
                    callee,
                    caller,
                    CallType::Call(args),
                    gas_quota,
                    true, // read only
                );
            }
            Some(contract) => {
                drop(state);
                let (ret, err) = contract.run(&input);
                return_without_callstack!(
                    self,
                    CallType::Call(args),
                    ret,
                    gas_quota,
                    err,
                    ready
                );
            }
        }
        Ok(())
    }

    #[inline(always)]
    async fn revert(&mut self, ready: &ReadyList) -> Result<(), ExecError> {
        let call = &mut self.cur_call;
        if call.fork < Fork::Byzantium {
            return Err(ExecError::InvalidOpcode)
        }
        let (off, len) = call.stack.consume2()?;
        let (retval, mem_gas) = call.memory.get_slice(off, len)?;
        let retval = retval.to_vec().into();
        call.use_gas(mem_gas)?;
        self.finish_call(retval, Err(ExecError::Reverted), ready)
            .await
    }

    #[inline(always)]
    async fn self_destruct(
        &mut self, ready: &ReadyList,
    ) -> Result<(), ExecError> {
        let call = &mut self.cur_call;
        if call.read_only {
            return Err(ExecError::WriteProtection)
        }
        let mut state = self.state.write();
        let beneficiary = call.stack.consume1()?.into();
        let refunds_enabled = call.fork < Fork::London;
        if call.fork >= Fork::Berlin &&
            !self.access_list.contains_addr(&beneficiary)
        {
            self.access_list.add_addr(&beneficiary);
            call.use_gas(GAS_COLD_ACCOUNT_ACCESS_COST_BERLIN)?;
        }
        if call.fork >= Fork::TangerineWhistle {
            // EIP150
            call.use_gas(GAS_SELF_DESTRUCT)?;
            if call.fork >= Fork::SpuriousDragon {
                let (a, b) = (
                    TxExecReady::wrap(ready, state.is_empty(&beneficiary)),
                    TxExecReady::wrap(ready, state.get_balance(&call.callee)),
                );
                let a = a.await;
                let b = b.await.is_zero();
                // EIP158
                if a && !b {
                    call.use_gas(GAS_CREATE_BY_SELF_DESTRUCT)?;
                }
            } else if !TxExecReady::wrap(ready, state.exist(&beneficiary)).await
            {
                call.use_gas(GAS_CREATE_BY_SELF_DESTRUCT)?;
            }
            if refunds_enabled && !self.is_destroyed(&self.cur_call.callee) {
                self.add_refund(GAS_SELF_DESTRUCT_REFUND);
            }
        }
        let balance =
            TxExecReady::wrap(ready, state.get_balance(&self.cur_call.callee))
                .await;
        TxExecReady::wrap(ready, state.add_balance(&beneficiary, &balance))
            .await;
        // suicide
        state.set_balance(&self.cur_call.callee, &0.into());
        drop(state);
        self.destroyed.insert(self.cur_call.callee.clone());

        // record modified accounts
        self.dirty.insert(self.cur_call.callee.clone());
        self.dirty.insert(beneficiary.clone());

        self.finish_call(Bytes::empty(), Ok(()), ready).await
    }

    // end of instruction impl

    #[inline(always)]
    fn is_destroyed(&self, addr: &Addr) -> bool {
        self.destroyed.contains(addr)
    }

    #[inline(always)]
    fn call_depth(&self) -> usize {
        self.call_stack.len()
    }

    fn add_refund(&self, gas: u64) {
        // TODO: will self.refund overflow?
        self.refund.set(self.refund.get() + gas)
    }

    fn sub_refund(&self, gas: u64) {
        let refund = self.refund.get();
        if gas > refund {
            panic!("refund counter below zero ({} > {})", gas, refund)
        }
        self.refund.set(refund - gas)
    }

    #[inline(always)]
    fn call_push(
        &mut self, code: Arc<dyn Code>, input: Box<[u8]>, value: Wei,
        callee: Addr, caller: Addr, call_type: CallType<S>, gas: Gas,
        read_only: bool,
    ) {
        let mut old_call = Box::new(CallFrame::new(
            code,
            input,
            value,
            callee,
            caller,
            call_type,
            gas,
            // all child calls become read-only if the parent is read-only
            self.cur_call.read_only || read_only,
            self.env.block.fork,
        ));
        std::mem::swap(&mut self.cur_call, &mut old_call);
        self.call_stack.push(old_call);
    }

    #[inline(always)]
    fn call_pop(&mut self) -> Box<CallFrame<S>> {
        let mut frame = self.call_stack.pop().unwrap();
        std::mem::swap(&mut self.cur_call, &mut frame);
        frame
    }

    #[inline(always)]
    async fn tx_end(
        &mut self, args: TxArgs<S>, data: Bytes, unused_gas: Gas,
        err: Result<(), ExecError>, ready: &ReadyList,
    ) {
        assert!(self.call_stack.is_empty());
        let (ret, addr) = match args.aux {
            TxAux::Contract(addr) => {
                (self.create_post_processing(&addr, &data, err), Some(addr))
            }
            TxAux::Sender(addr) => {
                self.dirty.insert(addr);
                (err, None)
            }
        };
        if let TxExecStatus::Running = self.status {
            // Only delete empty objects if EIP158/161 (a.k.a Spurious Dragon)
            // is in effect.
            if self.cur_call.fork >= Fork::SpuriousDragon {
                for acc in &self.dirty {
                    if TxExecReady::wrap(ready, self.state.read().is_empty(acc))
                        .await
                    {
                        self.state.write().delete_account(acc)
                    }
                }
            }
            self.dirty.clear();

            // Delete the destroyed accounts (by 'SelfDestruct') when transaction
            // is done but before the rollback can happen.
            for acc in &self.destroyed {
                self.state.write().delete_account(acc);
            }
            self.destroyed.clear();

            self.status = TxExecStatus::Finished(match ret {
                Err(err) => {
                    // execution is reverted
                    // FIXME: see `evm.go:481`
                    self.state.write().rollback(args.snapshot);
                    TxExecResult::Reverted(data, unused_gas, err)
                }
                Ok(()) => TxExecResult::Succeeded(data, unused_gas, addr),
            })
        } else {
            unreachable!()
        }
    }

    #[inline(always)]
    async fn finish_call(
        &mut self, data: Bytes, err: Result<(), ExecError>, ready: &ReadyList,
    ) -> Result<(), ExecError> {
        let frame = self.call_pop();
        match frame.call_type {
            CallType::Tx(args) => {
                self.tx_end(args, data, frame.unused_gas, err, ready).await
            }
            CallType::Create(args) => {
                self.create_end(args, data, frame.unused_gas, err)?;
            }
            CallType::Call(args) => {
                self.call_end(args, data, frame.unused_gas, err)?;
            }
        }
        Ok(())
    }

    #[inline(always)]
    fn advance_pc(&mut self, skip: u64, enter_call: bool) {
        if !enter_call {
            self.cur_call.pc += skip + 1
        }
    }

    async fn exec(mut self, ready: &ReadyList) -> TxExecResult {
        use Opcode::*;
        if let TxExecStatus::Finished(res) = self.status {
            warn!(
                "got a transaction execution context that has already finished"
            );
            return res
        }
        while let TxExecStatus::Running = self.status {
            let call = &mut self.cur_call;
            let _code = call.code.clone();
            let code = _code.as_bytes();
            let pc = call.pc as usize;
            let raw_opcode = code.get(pc).copied().unwrap_or(Stop as u8);
            let inst = match raw_opcode {
                // Push* case (32)
                c @ (0x60..=0x7f) => {
                    let base = pc + 1;
                    Inst {
                        opcode: Push,
                        data: Some(&code[base..base + (c - 0x60 + 1) as usize]),
                        pos: 0,
                    }
                }
                // DUP* case (16)
                c @ (0x80..=0x8f) => Inst {
                    opcode: Dup,
                    data: None,
                    pos: (c - 0x80 + 1) as usize,
                },
                // SWAP* case (16)
                c @ (0x90..=0x9f) => Inst {
                    opcode: Swap,
                    data: None,
                    pos: (c - 0x90 + 1) as usize,
                },
                _ => match FromPrimitive::from_u8(raw_opcode) {
                    Some(opcode) => Inst {
                        opcode,
                        data: None,
                        pos: 0,
                    },
                    None => Inst {
                        opcode: Invalid,
                        data: None,
                        pos: 0,
                    },
                },
            };
            let mut enter_call = false;
            let mut succ = match inst.opcode {
                Stop => self.finish_call(Bytes::empty(), Ok(()), ready).await,
                Add => call.add(),
                Mul => call.mul(),
                Sub => call.sub(),
                Div => call.div(),
                SDiv => call.sdiv(),
                Mod => call.rem(),
                SMod => call.smod(),
                AddMod => call.add_mod(),
                MulMod => call.mul_mod(),
                Exp => call.exp(),
                SignExtend => call.sign_extend(),
                Lt => call.lt(),
                Gt => call.gt(),
                Slt => call.slt(),
                Sgt => call.sgt(),
                Eql => call.eq(),
                IsZero => call.is_zero(),
                And => call.and(),
                Or => call.or(),
                Xor => call.xor(),
                Not => call.not(),
                Byte => call.byte(),
                Shl => call.shl(),
                Shr => call.shr(),
                Sar => call.sar(),
                Sha3 => call.sha3(),
                Addr => call.addr(),
                Balance => self.balance(ready).await,
                Origin => self.origin(),
                Caller => call.caller(),
                CallValue => call.call_value(),
                CallDataLoad => call.call_data_load(),
                CallDataSize => call.call_data_size(),
                CallDataCopy => call.call_data_copy(),
                CodeSize => call.code_size(),
                CodeCopy => call.code_copy(),
                GasPrice => self.gas_price(),
                ExtCodeSize => self.ext_code_size(ready).await,
                ExtCodeCopy => self.ext_code_copy(ready).await,
                ReturnDataSize => call.return_data_size(),
                ReturnDataCopy => call.return_data_copy(),
                ExtCodeHash => self.ext_code_hash(ready).await,
                BlockHash => self.block_hash(ready).await,
                Coinbase => self.coinbase(),
                Timestamp => self.timestamp(),
                Number => self.number(),
                Difficulty => self.difficulty(),
                GasLimit => self.gas_limit(),
                ChainId => self.chain_id(),
                SelfBalance => self.self_balance(ready).await,
                BaseFee => self.base_fee(),
                Pop => call.pop(),
                MLoad => call.mload(),
                MStore => call.mstore(),
                MStore8 => call.mstore8(),
                SLoad => self.sload(ready).await,
                SStore => self.sstore(ready).await,
                Jump => self.jump(),
                JumpI => self.jumpi(),
                PC => call.pc(),
                MSize => call.msize(),
                Gas => call.gas(),
                JumpDest => call.use_gas(GAS_JUMPDEST),
                Log0 => self.log(0),
                Log1 => self.log(1),
                Log2 => self.log(2),
                Log3 => self.log(3),
                Log4 => self.log(4),
                Push => call.push(inst.data.unwrap()),
                Dup => call.dup(inst.pos),
                Swap => call.swap(inst.pos),
                Create => {
                    enter_call = true;
                    self.create_begin(ready).await
                }
                Call => {
                    enter_call = true;
                    self.call_begin(ready).await
                }
                CallCode => {
                    enter_call = true;
                    self.call_code_begin(ready).await
                }
                Return => self.return_(ready).await,
                DelegateCall => {
                    enter_call = true;
                    self.delegate_call_begin(ready).await
                }
                Create2 => {
                    enter_call = true;
                    self.create2_begin(ready).await
                }
                StaticCall => {
                    enter_call = true;
                    self.static_call_begin(ready).await
                }
                Revert => self.revert(ready).await,
                SelfDestruct => self.self_destruct(ready).await,
                _ => Err(ExecError::InvalidOpcode),
            };
            while let Err(err) = succ {
                succ = self.finish_call(Bytes::empty(), Err(err), ready).await;
            }
            self.advance_pc(
                match &inst.data {
                    Some(d) => d.len() as u64,
                    None => 0,
                },
                enter_call,
            );
        }
        match self.status {
            TxExecStatus::Finished(res) => res,
            _ => unreachable!(),
        }
    }
}

/// Transaction execution state which encapsulates an on-going execution progress or a finished
/// result. It is the basic unit that is exchanged by [Core](struct.Core.html) to reschedule/resume the progress of a
/// transaction.
pub enum TxExecState {
    Running(TxExecHandle),
    Finished(TxExecResult),
}

impl TxExecState {
    /// Create the execution environment for a transaction.
    ///
    /// * `value` - amount the transaction transfers to the smart contract (`to`)
    /// * `from` - sender of the transaction
    /// * `gas` - gas limit
    /// * `state` - backing global (persistent) state for the entire execution
    /// * `committed_state` - the read-only state as of last block
    /// * `env` - auxiliary information for the execution (block/chain/mempool-relevant constants)
    pub fn new<S: WorldState + 'static>(
        from: Addr, to: Option<Addr>, value: Wei, gas: Gas, gas_price: Wei,
        input: Box<[u8]>, access_list: Option<TxAccessList>,
        state: Arc<RwLock<S>>, committed_state: S, env: Arc<super::TxExecEnv>,
    ) -> Self {
        let access_list = AccessList::new(
            &from,
            to.as_ref(),
            env.precompiled_contracts.keys(),
            access_list,
        );
        let ready = Arc::new(ReadyList::new());
        let mut ctx = TxExecContext {
            call_stack: Vec::with_capacity(MAX_CALL_DEPTH),
            /// dummy value, not used
            cur_call: Box::new(CallFrame::<S>::new(
                Arc::new(PlainCode::new(Vec::new().into())),
                Vec::new().into(),
                Wei::zero().clone(),
                Addr::zero().clone(),
                Addr::zero().clone(),
                CallType::Call(CallArgs {
                    snapshot: None,
                    ret_off: U256::zero(),
                    ret_len: U256::zero(),
                }),
                gas,
                false,
                env.block.fork,
            )),
            state,
            committed_state,
            refund: Cell::new(0),
            access_list,
            destroyed: HashSet::new(),
            dirty: HashSet::new(),
            status: TxExecStatus::Running,
            origin: from.clone(),
            gas_price,
            env,
        };
        let ready_ = ready.clone();
        let fut =
            async move {
                // FIXME: do the proper intrinsic gas calculation
                let gas = gas - 21000;
                let res = match to {
                    Some(to) => {
                        let nonce = TxExecReady::wrap(
                            &ready_,
                            ctx.state.read().get_nonce(&from),
                        )
                        .await;
                        ctx.state.write().set_nonce(&from, nonce + 1);
                        let snapshot = ctx.state.read().snapshot();
                        ctx.call_(
                            to,
                            from.clone(),
                            input,
                            gas,
                            value,
                            CallType::Tx(TxArgs {
                                snapshot,
                                aux: TxAux::Sender(from),
                            }),
                            &ready_,
                        )
                        .await
                    }
                    None => {
                        let snapshot = ctx.state.read().snapshot();
                        let nonce = TxExecReady::wrap(
                            &ready_,
                            ctx.state.read().get_nonce(&from),
                        )
                        .await;
                        let contract_addr =
                            crate::common::create_addr(&from, nonce);
                        ctx.create_(
                            contract_addr.clone(),
                            from,
                            input,
                            value,
                            gas,
                            CallType::Tx(TxArgs {
                                snapshot,
                                aux: TxAux::Contract(contract_addr),
                            }),
                            &ready_,
                        )
                        .await
                    }
                };
                if let Err(err) = res {
                    ctx.status = TxExecStatus::Finished(
                        TxExecResult::Reverted(Bytes::empty(), gas, err),
                    );
                }
                match ctx.status {
                    TxExecStatus::Running => ctx.exec(&ready_).await,
                    TxExecStatus::Finished(res) => res,
                }
            };
        TxExecState::Running(TxExecHandle {
            fut: Box::pin(fut),
            ready,
        })
    }
}
