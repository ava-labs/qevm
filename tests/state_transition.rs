use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::{fs, u64};

use parking_lot::RwLock;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Map;
use serde_json::Value;

use qevm::common::{Addr, Gas, Hash, Wei, U256};
use qevm::core::{TxExecResult, TxExecState, WorldState, WorldStateR};
use qevm::processor::run_single_tx;
use qevm::state::{EmptyState, MemStateAuto};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TxsJson {
    data: Vec<String>,
    gas_limit: Vec<String>,
    gas_price: String,
    nonce: String,
    sender: String,
    to: String,
    value: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct PreStateJson {
    balance: String,
    code: String,
    nonce: String,
    storage: HashMap<String, String>,
}

// Not necessary to include all fields for each account. Only
// those fields need to be tested are present.
#[derive(Serialize, Deserialize)]
struct ExpectedPostStateJson {
    balance: Option<String>,
    code: Option<String>,
    nonce: Option<String>,
    storage: Option<HashMap<String, String>>,
    shouldnotexist: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BlockInfo {
    current_coinbase: String,
    current_difficulty: String,
    current_gas_limit: String,
    current_number: String,
    current_timestamp: String,
    previous_hash: String,
    current_base_fee: Option<String>,
}

struct AccountState {
    balance: Option<Wei>,
    code: Option<Vec<u8>>,
    nonce: Option<u64>,
    storage: Option<HashMap<Hash, U256>>,
}

// File name suffix for filler json test data.
static SUFFIX_FILLER_JSON: &str = "Filler.json";
// File name suffix for json test data.
static SUFFIX_JSON: &str = ".json";

static RESOURCE_TEST_DIR: &str = "/resources/test";

fn get_all_files_or_folders(dir_path: &str, is_file: bool) -> HashSet<String> {
    let mut entries: HashSet<String> = HashSet::new();
    for entry in fs::read_dir(dir_path).unwrap() {
        let path = entry
            .as_ref()
            .map(|e| e.path())
            .expect("entry should not be empty");
        assert_eq!(!path.is_dir(), is_file);
        let name_obj = entry.map(|e| e.file_name()).unwrap();
        let name_str = name_obj.into_string().expect("invalid file name");
        if path.is_file() {
            let prefix = if name_str.ends_with(SUFFIX_FILLER_JSON) {
                &name_str[..name_str.len() - SUFFIX_FILLER_JSON.len()]
            } else {
                &name_str[..name_str.len() - SUFFIX_JSON.len()]
            };
            entries.insert(prefix.to_string());
        } else {
            entries.insert(name_str);
        }
    }
    entries
}

fn load_json(json_path: &str, test_name: &str) -> Map<String, Value> {
    let data = fs::read_to_string(json_path).expect("Unable to read file");

    let json: Value = serde_json::from_str(&data)
        .expect("JSON does not have correct format.");
    json[test_name].as_object().unwrap().clone()
}

async fn init_accounts<S: WorldState + 'static>(
    state: &Arc<RwLock<S>>, json_body: &Map<String, Value>,
) {
    // Extract the PreState of different accounts.
    let pre_states = &json_body["pre"];
    for (address, pre_state) in pre_states.as_object().unwrap() {
        let pre_state_str = serde_json::to_string(pre_state).unwrap();
        let pre_state: PreStateJson =
            serde_json::from_str(&pre_state_str).unwrap();

        // Create account
        let addr = Addr::from_str(address).expect("invalid address format");
        state.write().create_account(&addr).await;

        // Setup balance
        let balance =
            Wei::from_str(&pre_state.balance).expect("invalid balance format");
        state.write().set_balance(&addr, &balance);

        // Setup code
        let code =
            hex::decode(&pre_state.code[2..]).expect("invalid code format");
        state.write().set_code(&addr, &code);

        // Setup nonce
        let nonce = u64::from_str_radix(&pre_state.nonce[2..], 16)
            .expect("invalid nonce format");
        state.write().set_nonce(&addr, nonce);

        // Setup storage value
        for (key, val) in pre_state.storage {
            let key = Hash::from(U256::from_str(&key).unwrap());
            let val = U256::from_str(&val).unwrap();
            state.write().set_state(&addr, &key, &val);
        }
    }
}

fn load_post_account_state(
    filler_json: &Map<String, Value>,
) -> (HashMap<Addr, AccountState>, HashSet<Addr>) {
    let mut post_accounts: HashMap<Addr, AccountState> = HashMap::new();
    let mut not_exist_accounts: HashSet<Addr> = HashSet::new();

    // Extract the expect result information.
    let expect = &filler_json["expect"];
    for record in expect.as_array().unwrap() {
        for (address, result) in record["result"].as_object().unwrap() {
            let result_str = serde_json::to_string(result).unwrap();
            let post_state: ExpectedPostStateJson =
                serde_json::from_str(&result_str).unwrap();

            let addr = Addr::from_str(address).expect("invalid address format");
            let balance = post_state.balance.map(|b| {
                U256::from_dec_str(&b).expect("invalid balance format")
            });
            let balance = balance.map(Wei::from);
            let code = post_state.code.map(|c| {
                if !c.is_empty() {
                    hex::decode(&c[2..]).expect("invalid code format")
                } else {
                    Vec::new()
                }
            });
            let nonce = post_state
                .nonce
                .map(|n| u64::from_str(&n).expect("invalid nonce format"));
            let storage = post_state.storage.map(|s| {
                let mut storage: HashMap<Hash, U256> = HashMap::new();
                for (key, val) in s {
                    let key = U256::from_str(&key).unwrap();
                    storage
                        .insert(Hash::from(key), U256::from_str(&val).unwrap());
                }
                storage
            });
            let should_not_exist_acc = post_state
                .shouldnotexist
                .map(|s| match &*s {
                    "true" => true,
                    "1" => true,
                    _ => false,
                })
                .unwrap_or(false);
            if should_not_exist_acc {
                not_exist_accounts.insert(addr.clone());
            }

            let account: AccountState = AccountState {
                balance,
                code,
                nonce,
                storage,
            };
            post_accounts.insert(addr, account);
        }
    }
    (post_accounts, not_exist_accounts)
}

fn load_block_info(json: &Map<String, Value>) -> BlockInfo {
    // Extract the BlockInfo.
    let env_str = serde_json::to_string(&json["env"]).unwrap();

    serde_json::from_str::<BlockInfo>(&env_str).unwrap()
}

fn load_transactions(json: &Map<String, Value>) -> TxsJson {
    // Extract transactions information.
    let tx_str = serde_json::to_string(&json["transaction"]).unwrap();
    let tx: TxsJson = serde_json::from_str(&tx_str).unwrap();
    tx
}

// This test verifies the VM's in memory world states post transaction execution
// with initial state setup. The expected test data is referenced from state transition
// test set of https://github.com/ethereum/tests.
#[tokio::test]
async fn test_state_transitions() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let test_resource_root =
        format!("{}{}", manifest_dir.display(), RESOURCE_TEST_DIR);
    let dirs = get_all_files_or_folders(&test_resource_root, false);
    for entry in dirs {
        let dir = format!("{}/{}", test_resource_root, entry);
        let files = get_all_files_or_folders(&dir, true);
        for file in files {
            let file_path = format!("{}/{}", dir, file);
            test_state_transition(&file_path, &file).await;
        }
    }
}

async fn test_state_transition(file_path: &str, file_name: &str) {
    // 1. Get both filler and generated test files and parse them.
    let filler_json_path = format!("{}{}", file_path, SUFFIX_FILLER_JSON);
    let filler_json = load_json(&filler_json_path, file_name);

    let json_path = format!("{}.json", file_path);
    let json = load_json(&json_path, file_name);

    // 2. Initialize the world state of the VM before transaction execution.
    let block_info = load_block_info(&filler_json);
    let env = Arc::new(qevm::core::TxExecEnv {
        // Use Avalanche chain ID for now, TODO: does it matter?
        chain_id: 43114.into(),
        precompiled_contracts: HashMap::new(),
        block: qevm::core::BlockInfo {
            coinbase: Addr::from_str(&block_info.current_coinbase).unwrap(),
            timestamp: U256::from_str(&block_info.current_timestamp).unwrap(),
            number: U256::from_str(&block_info.current_number).unwrap(),
            difficulty: U256::from_str(&block_info.current_difficulty).unwrap(),
            gas_limit: 8_000_000,
            base_fee: 0.into(),
            // TODO: test with more forks.
            fork: qevm::core::Fork::London,
        },
        block_hash_getter: Box::new(|_number: u64| unimplemented!()),
    });

    // 3. Setup the account states of the VM.
    let (_core, core_in, core_out) = qevm::core::Core::new();
    let state = Arc::new(RwLock::new(MemStateAuto::new(Arc::new(EmptyState))));
    let committed_state = state.read().to_shared();
    init_accounts(&state, &json).await;

    // 4. Load the transaction itself, and the expected account states
    //    post execution.
    let txs = load_transactions(&json);
    let (post_states, not_exist_acc) = load_post_account_state(&filler_json);

    // 5. Execute the transaction(s) in the VM. The transaction may share
    //    the same sender and receiver, etc but differ at input.
    for data in txs.data {
        let sender = Addr::from_str(&txs.sender).unwrap();
        let to = if txs.to.is_empty() {
            None
        } else {
            Some(Addr::from_str(&txs.to).unwrap())
        };
        let value = Wei::from_str(&txs.value[0]).unwrap();
        let gas = Gas::from_str_radix(&txs.gas_limit[0][2..], 16).unwrap();
        let gas_price = Wei::from_str(&txs.gas_price).unwrap();
        let input: Box<[u8]> = hex::decode(&data[2..]).unwrap().into();

        match to {
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
                        state.clone(),
                        committed_state.clone().into(),
                        env.clone(),
                    ),
                    &core_in,
                    &core_out,
                )
                .await
                {
                    TxExecResult::Succeeded(_, _, _) => {
                        // TODO: verify the contract address and the deployed code.
                    }
                    TxExecResult::Reverted(_, _, _) => {
                        // TODO: verify expected exception.
                    }
                };
            }
            Some(to) => {
                match run_single_tx(
                    TxExecState::new(
                        sender,
                        Some(to),
                        value,
                        gas,
                        gas_price,
                        input,
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
                    TxExecResult::Succeeded(_, _, _) => {
                        // TODO: verify the output data.
                    }
                    TxExecResult::Reverted(_, _, _) => {
                        // TODO: verify expected exception.
                    }
                }
            }
        }
    }

    // 6. Compare the expected post state with the new VM's state.
    // TODO: Verify the log information.
    for (addr, post_state) in post_states {
        match post_state.balance {
            None => {}
            Some(expected_balance) => {
                assert_eq!(
                    expected_balance,
                    state.read().get_balance(&addr).await,
                    "balance validation failed at: {} with account {}",
                    file_name,
                    addr
                );
            }
        }

        match post_state.code {
            None => {}
            Some(expected_code) => {
                assert_eq!(
                    expected_code,
                    state.read().get_code(&addr).await.as_bytes(),
                    "code validation failed at: {} with account {}",
                    file_name,
                    addr
                );
            }
        }

        match post_state.nonce {
            None => {}
            Some(expected_nonce) => {
                assert_eq!(
                    expected_nonce,
                    state.read().get_nonce(&addr).await,
                    "nonce validation failed at: {} with account {}",
                    file_name,
                    addr
                );
            }
        }

        match post_state.storage {
            None => {}
            Some(expected_storage) => {
                for (key, val) in expected_storage {
                    assert_eq!(
                        val,
                        state.read().get_state(&addr, &key).await,
                        "storage validation failed: {} with account {}",
                        file_name,
                        addr
                    );
                }
            }
        }
    }

    // Check deleted account should not exist in the world state.
    for addr in not_exist_acc {
        assert_eq!(
            false,
            state.read().exist(&addr).await,
            "deleted account check failed at: {} with account {}",
            file_name,
            addr
        );
    }
}
