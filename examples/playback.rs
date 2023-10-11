use std::collections::hash_map::HashMap;
use std::io::BufRead;
use std::io::Write;
use std::str::FromStr;
use std::sync::mpsc;
use std::sync::Arc;

use parking_lot::RwLock;
use serde::Deserialize;

use qevm::common::{Addr, Gas, Wei, U256};
use qevm::core::{TxExecResult, TxExecState, WorldStateW};
use qevm::processor::run_single_tx;
use qevm::state::{EmptyState, MemStateAuto, MemStateShared};
use qevm_tools as tools;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OuterTx {
    result: Tx,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Tx {
    block_number: String,
    from: String,
    gas: String,
    gas_price: String,
    hash: String,
    input: String,
    // nonce: String, // may be used for sanity checking
    to: Option<String>,
    value: String,
}

struct Playback {
    state: Arc<RwLock<MemStateAuto<'static>>>,
    committed_state: MemStateShared<'static>,
    core_in: mpsc::Sender<TxExecState>,
    core_out: mpsc::Receiver<TxExecState>,
    _core: qevm::core::Core,
}

impl Playback {
    async fn run_tx(&self, json: &str) -> Result<(), ()> {
        let tx = {
            let tx: Result<Tx, _> = serde_json::from_str(json).map_err(|_| {});
            match tx {
                Ok(tx) => tx,
                Err(_) => {
                    let outer: OuterTx =
                        serde_json::from_str(json).map_err(|_| {})?;
                    outer.result
                }
            }
        };
        let env = Arc::new(qevm::core::TxExecEnv {
            chain_id: 43114.into(),
            precompiled_contracts: HashMap::new(),
            block: qevm::core::BlockInfo {
                coinbase: Addr::zero().clone(),
                timestamp: 0.into(),
                number: U256::from_str(&tx.block_number).map_err(|_| {})?,
                difficulty: 0.into(),
                gas_limit: 8_000_000,
                base_fee: 0.into(),
                fork: qevm::core::Fork::London,
            },
            block_hash_getter: Box::new(|_number: u64| unimplemented!()),
        });
        let sender = Addr::from_str(&tx.from)?;
        let value = Wei::from_str(&tx.value)?;
        let gas = Gas::from_str_radix(&tx.gas[2..], 16).map_err(|_| {})?;
        let gas_price = Wei::from_str(&tx.gas_price)?;
        let input: Box<_> = hex::decode(&tx.input[2..]).map_err(|_| {})?.into();
        match tx.to {
            None => {
                match run_single_tx(
                    TxExecState::new(
                        sender.clone(),
                        None,
                        value,
                        gas,
                        gas_price,
                        input,
                        None,
                        self.state.clone(),
                        self.committed_state.clone().into(),
                        env,
                    ),
                    &self.core_in,
                    &self.core_out,
                )
                .await
                {
                    TxExecResult::Succeeded(data, _, addr) => {
                        let addr = addr.unwrap();
                        println!(
                            "contract 0x{} deployed (runtime=0x{}, {})",
                            hex::encode(addr.as_bytes()),
                            hex::encode(data.as_ref()),
                            tools::disasm(&data, false)
                        );
                    }
                    TxExecResult::Reverted(data, _, err) => {
                        println!(
                            "contract creation tx {} reverted (data={} err={:?})",
                            tx.hash,
                            hex::encode(data.as_ref()),
                            err
                        );
                    }
                };
            }
            Some(to) => {
                let to = Addr::from_str(&to)?;
                match run_single_tx(
                    TxExecState::new(
                        sender,
                        Some(to),
                        value,
                        gas,
                        gas_price,
                        input,
                        None,
                        self.state.clone(),
                        self.committed_state.clone().into(),
                        env,
                    ),
                    &self.core_in,
                    &self.core_out,
                )
                .await
                {
                    TxExecResult::Succeeded(data, _, _) => println!(
                        "tx {} returned (data={})",
                        tx.hash,
                        hex::encode(data.as_ref())
                    ),
                    TxExecResult::Reverted(data, _, err) => println!(
                        "tx {} reverted (data={} err={:?})",
                        tx.hash,
                        hex::encode(data.as_ref()),
                        err
                    ),
                }
            }
        }
        Ok(())
    }

    async fn run(&mut self, action: &str, data: &str) {
        match action {
            "tx" => self.run_tx(data).await.expect("invalid json format"),
            "create_account" => {
                let tokens: Vec<_> = data.split(' ').collect();
                if tokens.len() != 1 {
                    panic!("wrong number of arguments");
                }
                let acc =
                    Addr::from_str(tokens[0]).expect("invalid addresss format");
                println!("{}.create", acc);
                self.state.write().create_account(&acc).await
            }
            "set_nonce" => {
                let tokens: Vec<_> = data.split(' ').collect();
                if tokens.len() != 2 {
                    panic!("wrong number of arguments");
                }
                let acc =
                    Addr::from_str(tokens[0]).expect("invalid addresss format");
                let nonce =
                    u64::from_str(tokens[1]).expect("invalid nonce format");
                println!("{}.nonce = {}", acc, nonce);
                self.state.write().set_nonce(&acc, nonce)
            }
            "block_commit" => {
                self.committed_state = self.state.read().to_shared();
                self.committed_state.consolidate(None);
                println!("block_commit");
            }
            _ => (),
        }
    }
}

#[actix_rt::main]
async fn main() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .format(|buf, r| writeln!(buf, "{}: {}", r.level(), r.args()))
    .init();

    let (_core, core_in, core_out) = qevm::core::Core::new();
    let state = Arc::new(RwLock::new(MemStateAuto::new(Arc::new(EmptyState))));
    let committed_state = state.read().to_shared();
    let mut playback = Playback {
        state,
        committed_state,
        _core,
        core_in,
        core_out,
    };
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let s = line.unwrap();
        match s.find(' ') {
            Some(pos) => playback.run(&s[..pos], &s[pos + 1..]).await,
            None => (),
        }
    }
}
