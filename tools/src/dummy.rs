use std::collections::hash_map::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use log::info;

use qevm::common::{Addr, Hash, Wei, U256};
use qevm::core::{Code, PlainCode, WorldStateR};

#[derive(Clone)]
struct DummyAccountState {
    state: HashMap<Hash, U256>,
    balance: Wei,
    nonce: u64,
    code: Arc<dyn Code>,
}

impl Default for DummyAccountState {
    fn default() -> Self {
        Self {
            state: HashMap::new(),
            balance: Wei::zero().clone(),
            nonce: 0,
            code: Arc::new(PlainCode::new(Vec::new().into())),
        }
    }
}

#[derive(Clone)]
pub struct DummyStateStore {
    accounts: HashMap<Addr, DummyAccountState>,
}

impl DummyStateStore {
    pub fn new() -> Self {
        Self {
            accounts: HashMap::new(),
        }
    }

    fn get_account(&mut self, contract: &Addr) -> &mut DummyAccountState {
        self.accounts
            .entry(contract.clone())
            .or_insert_with(DummyAccountState::default)
    }

    pub fn accounts(&self) -> impl Iterator<Item = &Addr> {
        self.accounts.keys()
    }

    pub fn account_keys(
        &self, contract: &Addr,
    ) -> Option<impl Iterator<Item = &Hash>> {
        self.accounts.get(contract).map(|acc| acc.state.keys())
    }

    pub fn len(&self) -> usize {
        self.accounts.len()
    }
}

#[async_trait]
impl qevm::core::WorldStateR for DummyStateStore {
    async fn get_state(&self, contract: &Addr, key: &Hash) -> U256 {
        info!("get_state({}, {})", contract, key);
        self.accounts
            .get(contract)
            .and_then(|acc| acc.state.get(key))
            .map(|val| val.clone())
            .unwrap_or_else(|| U256::zero())
    }
    async fn get_balance(&self, account: &Addr) -> Wei {
        info!("get_balance({})", account);
        self.accounts
            .get(account)
            .map(|acc| acc.balance.clone())
            .unwrap_or_else(|| Wei::zero().clone())
    }
    async fn get_code(&self, contract: &Addr) -> Arc<dyn Code> {
        info!("get_code({})", contract);
        self.accounts
            .get(contract)
            .map(|acc| acc.code.clone())
            .unwrap_or_else(|| qevm::state::empty_code())
    }
    async fn get_nonce(&self, contract: &Addr) -> u64 {
        info!("get_nonce({})", contract);
        self.accounts
            .get(contract)
            .map(|acc| acc.nonce)
            .unwrap_or(0)
    }
    async fn exist(&self, contract: &Addr) -> bool {
        info!("exist({})", contract);
        self.accounts.contains_key(contract)
    }
}

#[async_trait]
impl qevm::core::WorldStateW for DummyStateStore {
    fn set_state(&mut self, contract: &Addr, key: &Hash, val: &U256) {
        info!("set_state({}, {}, {})", contract, key, val);
        self.get_account(contract)
            .state
            .insert(key.clone(), val.clone());
    }
    fn set_balance(&mut self, contract: &Addr, balance: &Wei) {
        info!("set_balance({}, {})", contract, balance);
        self.get_account(contract).balance = balance.clone()
    }
    fn set_code(&mut self, contract: &Addr, code: &[u8]) {
        info!("set_code({}, {})", contract, hex::encode(code));
        self.get_account(contract).code = Arc::new(PlainCode::new(code.into()));
    }
    fn set_nonce(&mut self, contract: &Addr, nonce: u64) {
        info!("set_nonce({}, {})", contract, nonce);
        self.get_account(contract).nonce = nonce
    }
    async fn create_account(&mut self, addr: &Addr) {
        info!("create_account({})", addr);
        let old_balance = self.get_balance(addr).await;
        self.accounts
            .insert(addr.clone(), DummyAccountState::default());
        if !old_balance.is_zero() {
            let acc = self.accounts.get_mut(&addr).unwrap();
            acc.balance = old_balance;
        }
    }
    fn delete_account(&mut self, addr: &Addr) {
        info!("delete_account({})", addr);
        self.accounts.remove(addr);
    }
    fn add_log(
        &mut self, contract: &Addr, topics: &[Hash], data: &[u8],
        block_number: &U256,
    ) {
        let topics: Vec<_> = topics
            .iter()
            .map(|h| format!("0x{}", hex::encode(h.as_bytes())))
            .collect();
        info!(
            "add_log(number={} contract={} topics=({}) data={})",
            block_number,
            contract,
            topics.join(","),
            hex::encode(data)
        );
    }
}

impl qevm::core::WorldState for DummyStateStore {
    fn snapshot(&self) -> Self {
        info!("snapshot()");
        self.clone()
    }
    fn rollback(&mut self, mut state: Self) {
        info!("rollback()");
        std::mem::swap(self, &mut state);
    }
}
