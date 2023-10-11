use std::collections::hash_map::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{mpsc, Arc};
use std::task::Poll;

use async_trait::async_trait;

mod alu;
mod call;
mod exec;
mod memory;
pub mod opcode;
pub mod params;
mod stack;

use crate::common::{Addr, Bytes, Gas, Hash, Wei, U256};
pub use exec::{
    PlainCode, Transferable, TxExecHandle, TxExecReady, TxExecResult,
    TxExecState,
};
pub use params::Fork;

/// An immutable code object that can be read-shared by threads. Necessary internal caching should
/// be implemented to make all the methods execute in constant (O(1)) time. For a simple,
/// standalone implementation, refer to [PlainCode]. In a more sophisticated implementation,
/// multiple code objects may internally share some code analysis results.
pub trait Code: Send + Sync {
    fn is_valid_jumpdest(&self, dest: &U256) -> bool;
    fn as_bytes(&self) -> &[u8];
    fn get_hash(&self) -> &Hash;
}

/// A world state object that is read-shared by threads. It uses an asynchronous interface, but it
/// is recommended to have a "fast path" that immediately resolves by some proper caching mechanism
/// for better performance (less switching/rescheduling).
#[async_trait]
pub trait WorldStateR: Send + Sync {
    /// Get the value from the `account` state space, indexed by `key`.
    async fn get_state(&self, account: &Addr, key: &Hash) -> U256;
    /// Get the balance of the `account`.
    async fn get_balance(&self, account: &Addr) -> Wei;
    /// Get code of the contract account. If code does not exist, the Code object should return
    /// zero-byte slice and empty hash (i.e. `Hash::zero()`)
    async fn get_code(&self, account: &Addr) -> Arc<dyn Code>;
    /// Get nonce of the account.
    async fn get_nonce(&self, account: &Addr) -> u64;
    /// Check if an account exists.
    async fn exist(&self, account: &Addr) -> bool;
    // TODO: get_log()
}

/// A world state object that is mutable. Writes may not need async methods as the implementation
/// can simply defer (buffer) the writes (see [MemStateAuto](../state/struct.MemStateAuto.html) in memory until
/// later to de-duplicate, so the interpreter can keep moving forward without waiting for the disk
/// I/O.
#[async_trait]
pub trait WorldStateW {
    /// Set the key under the given account to a specified value.
    fn set_state(&mut self, account: &Addr, key: &Hash, val: &U256);
    /// Set the balance of the account.
    fn set_balance(&mut self, account: &Addr, balance: &Wei);
    /// Set the code of the contract account.
    fn set_code(&mut self, account: &Addr, code: &[u8]);
    /// Set the nonce of the account.
    fn set_nonce(&mut self, account: &Addr, nonce: u64);
    /// Create an account. This could be a Read-Modify-Write workload, so it is async.
    async fn create_account(&mut self, addr: &Addr);
    /// Delete an account.
    fn delete_account(&mut self, addr: &Addr);
    /// Add a log entry to the storage.
    fn add_log(
        &mut self, account: &Addr, topics: &[Hash], data: &[u8],
        block_number: &U256,
    );
}

/// A sharable object that captures the entire world state of EVM. The object will be used within a
/// reader-writer lock so it is thread-safe as long as the reads can be naturally shared among
/// threads (the `Sync` requirement in [WorldStateR]).
pub trait WorldState: WorldStateR + WorldStateW {
    /// Create a quick clone of the existing state, which could be implemented in a Copy-on-Write
    /// fashion (see `src/state.rs`).
    fn snapshot(&self) -> Self;
    /// Rollback to a given snapshot.
    fn rollback(&mut self, state: Self);
}

/// Pre-compiled smart contract.
pub trait PrecompiledContract: Send + Sync {
    fn run(&self, input: &[u8]) -> (Bytes, Result<(), ExecError>);
}

pub type BlockHashByNumberFunc = Box<
    dyn Fn(u64) -> Pin<Box<dyn Future<Output = Hash> + Send>> + Sync + Send,
>;

/// Execution environment for EVM. This captures the external information that is required to run
/// an EVM interpreter.
pub struct TxExecEnv {
    /// Chain ID.
    pub chain_id: U256,
    /// Block-related information.
    pub block: BlockInfo,
    /// Precompiled smart contracts.
    pub precompiled_contracts: HashMap<Addr, Box<dyn PrecompiledContract>>,
    /// Callback function that returns the canonical block hash given a number (height).
    pub block_hash_getter: BlockHashByNumberFunc,
}

pub struct BlockInfo {
    pub coinbase: Addr,
    pub timestamp: U256,
    pub number: U256,
    pub difficulty: U256,
    pub gas_limit: u64,
    pub base_fee: U256,
    pub fork: params::Fork,
}

#[derive(Copy, Clone, Debug)]
pub enum ExecError {
    OutOfGas,
    CodeStoreOutOfGas,
    Depth,
    InsufficientBalance,
    ContractAddrCollision,
    Reverted,
    MaxCodeSizeExceeded,
    InvalidJump,
    WriteProtection,
    ReturnDataOutOfBounds,
    GasIntOverflow,
    InvalidCode,
    NonceIntOverflow,
    StackOverflow,
    StackUnderflow,
    OutOfMemory,
    InvalidOpcode,
}

/// A sequential emulation of EVM transactions. The core executor does *not* reorder the
/// transactions and it is the processor's (the one that maintains and feeds [TxExecState] to Core)
/// responsibility to ensure the correct ordering (or commutative property) of the transactions so
/// that the changes made by different [TxExecState] objects to the shared [WorldState] object make
/// sense. (May need to rollback the state if the speculative, parallel execution generate some
/// data race that violates the EVM strong ordering semantics.)
pub struct Core {
    runner: Option<std::thread::JoinHandle<()>>,
}

impl Core {
    /// Create an EVM interpreter. The returned tuple consists of the interpreter itself, the
    /// inbound channel to feed [TxExecState] into the interpreter, and the outbound channel after
    /// the interpreter (fully or partially) executes the [TxExecState]. The processor (user of the
    /// interpreter) needs to keep feeding the [TxExecState] state object until the execution is
    /// reported to be finished `TxExecState::Finished(data)`, where data contains the result from
    /// the transaction execution.
    pub fn new(
    ) -> (Self, mpsc::Sender<TxExecState>, mpsc::Receiver<TxExecState>) {
        let (in_tx, in_rx): (mpsc::Sender<TxExecState>, _) = mpsc::channel();
        let (out_tx, out_rx): (mpsc::Sender<TxExecState>, _) = mpsc::channel();
        let mut core = Self { runner: None };
        let runner = std::thread::spawn(move || {
            let waker = futures::task::noop_waker();
            let mut cx = futures::task::Context::from_waker(&waker);
            while let Ok(mut b) = in_rx.recv() {
                if let TxExecState::Running(h) = &mut b {
                    if let Poll::Ready(res) = Pin::new(&mut h.fut).poll(&mut cx)
                    {
                        b = TxExecState::Finished(res)
                    }
                }
                if let Err(_) = out_tx.send(b) {
                    panic!("core output channel was shutdown too soon");
                }
            }
        });
        core.runner = Some(runner);
        (core, in_tx, out_rx)
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        if let Some(t) = self.runner.take() {
            t.join().unwrap()
        }
    }
}

#[allow(dead_code)]
fn gas_checked_add(x: Gas, y: Gas) -> Result<Gas, ExecError> {
    x.checked_add(y).ok_or(ExecError::GasIntOverflow)
}

fn gas_checked_mul(x: Gas, y: Gas) -> Result<Gas, ExecError> {
    x.checked_mul(y).ok_or(ExecError::GasIntOverflow)
}

fn get_data(src: &[u8], mut off: u64, len: u64) -> Vec<u8> {
    let src_len = src.len() as u64;
    if off > src_len {
        off = src_len
    }
    let mut end = off + len;
    if end > src_len {
        end = src_len
    }
    let mut data = src[off as usize..end as usize].to_vec();
    // right-pad bytes
    data.resize(len as usize, 0);
    data
}

#[test]
fn test_get_data() {
    assert_eq!(
        get_data(&hex::decode("00010203").unwrap(), 0, 4),
        hex::decode("00010203").unwrap()
    );
}
