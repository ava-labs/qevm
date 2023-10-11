//! See [core](../core/struct.Core.html) for more information on the definition of a processor.

use std::sync::mpsc;

use crate::core::{TxExecResult, TxExecState};

/// Non-blocking, single-threaded scheduler to fully execute one transaction.
///
/// It will keep feeding the transaction back to the core until it finishes execution.
pub async fn run_single_tx(
    mut tx: TxExecState, core_in: &mpsc::Sender<TxExecState>,
    core_out: &mpsc::Receiver<TxExecState>,
) -> TxExecResult {
    loop {
        core_in.send(tx).unwrap();
        tx = core_out.recv().unwrap();
        match tx {
            TxExecState::Finished(ret) => return ret,
            TxExecState::Running(ref h) => {
                for r in h.drain_ready_list() {
                    r.await
                }
            }
        }
    }
}
