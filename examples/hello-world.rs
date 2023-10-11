use std::collections::hash_map::HashMap;
use std::io::Write;
use std::str::FromStr;
use std::sync::Arc;

use parking_lot::RwLock;

use qevm::common::Addr;
use qevm::core::{TxExecResult, TxExecState};
use qevm::processor::run_single_tx;
use qevm::state::{EmptyState, MemStateAuto};
use qevm_tools as tools;

#[actix_rt::main]
async fn main() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .format(|buf, r| writeln!(buf, "{}: {}", r.level(), r.args()))
    .init();

    let state = Arc::new(RwLock::new(MemStateAuto::new(Arc::new(EmptyState))));
    let env = Arc::new(qevm::core::TxExecEnv {
        chain_id: 0.into(),
        precompiled_contracts: HashMap::new(),
        block: qevm::core::BlockInfo {
            coinbase: Addr::zero().clone(),
            timestamp: 0.into(),
            number: 0.into(),
            difficulty: 0.into(),
            gas_limit: 1000000,
            base_fee: 0.into(),
            fork: qevm::core::Fork::London,
        },
        block_hash_getter: Box::new(|_number: u64| unimplemented!()),
    });

    let code = hex::decode("60806040526000805463ffffffff1916905534801561001d57600080fd5b506101c48061002d6000396000f3fe608060405234801561001057600080fd5b506004361061002b5760003560e01c8063247fbaa314610030575b600080fd5b61004361003e3660046100c8565b61005a565b6040516100519291906100f5565b60405180910390f35b600080546060919083908290819061007990849063ffffffff16610158565b82546101009290920a63ffffffff81810219909316918316021790915560005460408051808201909152600d81526c48656c6c6f2c20576f726c642160981b6020820152969116945092505050565b6000602082840312156100da57600080fd5b813563ffffffff811681146100ee57600080fd5b9392505050565b604081526000835180604084015260005b818110156101235760208187018101516060868401015201610106565b81811115610135576000606083860101525b5063ffffffff93909316602083015250601f91909101601f191601606001919050565b600063ffffffff80831681851680830382111561018557634e487b7160e01b600052601160045260246000fd5b0194935050505056fea264697066735822122099dc4dc06dd14a16b616dff4a116b5da7d7ff983f15d95b639cfcc47afd275b364736f6c634300080b0033").unwrap();
    //println!("{}", tools::disasm(&code, true));
    println!("code: {}", tools::disasm(&code, false));

    let alice =
        Addr::from_str("0x71C7656EC7ab88b098defB751B7401B5f6d8976F").unwrap();

    let (_core, core_in, core_out) = qevm::core::Core::new();
    let committed_state = state.read().to_shared();
    let addr = match run_single_tx(
        TxExecState::new(
            alice.clone(),
            None,
            0.into(),
            1000000,
            0.into(),
            code.into(),
            None,
            state.clone(),
            committed_state.clone().into(),
            env.clone(),
        ),
        &core_in,
        &core_out,
    )
    .await
    {
        TxExecResult::Succeeded(data, _, addr) => {
            let addr = addr.unwrap();
            println!(
                "contract deployed (data={} addr={})",
                hex::encode(data.as_ref()),
                addr
            );
            addr
        }
        TxExecResult::Reverted(..) => unreachable!(),
    };
    for _ in 0..3 {
        match run_single_tx(TxExecState::new(
                alice.clone(),
                Some(addr.clone()),
                0.into(),
                1000000,
                0.into(),
                hex::decode("247fbaa3000000000000000000000000000000000000000000000000000000000000002a").unwrap().into(),
                None,
                state.clone(),
                committed_state.clone().into(),
                env.clone(),
        ), &core_in, &core_out).await {
            TxExecResult::Succeeded(data, _, _) => println!("tx returned (data={})", hex::encode(data.as_ref())),
            TxExecResult::Reverted(data, _, err) => println!("tx reverted (data={} err={:?})", hex::encode(data.as_ref()), err),
        }
    }
}
