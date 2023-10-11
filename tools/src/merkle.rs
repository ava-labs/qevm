use std::io::{Read, Write};
use std::marker::PhantomData;
use std::sync::Arc;
use sync_unsafe_cell::SyncUnsafeCell;

use async_trait::async_trait;
use memory_db::{KeyFunction, PrefixedKey};
use parking_lot::Mutex;
use reference_trie::{ExtensionLayout, RefHasher};
use rocksdb::{WriteBatch, DB};
use trie_db::{
    DBValue, HashDB, HashDBRef, Hasher, Trie, TrieDB, TrieDBMut, TrieMut,
};

use qevm::common::{Addr, Hash, Wei, U256};
use qevm::core::{PlainCode, WorldStateR};
use qevm::state::{AccountRootR, WorldStateStore};

/// A simple implementation that keeps the merkle trie forrest with RocksDB and all changes are
/// Copy-on-Write (the DB will only grow in its size).
pub struct MerkleDB(Arc<MerkleDBInner>);

struct MerkleDBInner {
    db: SyncUnsafeCell<ReadWriteDB<RefHasher, PrefixedKey<RefHasher>>>,
}

struct ReadWriteDBInner {
    db: DB,
    wb: Option<WriteBatch>,
}

impl ReadWriteDBInner {
    fn get_wb(&mut self) -> &mut WriteBatch {
        self.wb.get_or_insert_with(|| WriteBatch::default())
    }
}

struct ReadWriteDB<H: Hasher, KF: KeyFunction<H>> {
    inner: Mutex<ReadWriteDBInner>,
    prefix: Vec<u8>,
    null_node_hash: H::Out,
    null_node_data: Vec<u8>,
    _kf: PhantomData<KF>,
}

struct Counter(i64);

impl Counter {
    fn encode(&self) -> DBValue {
        self.0.to_le_bytes().into()
    }

    fn decode(raw: &[u8]) -> Option<Self> {
        Some(Self(i64::from_le_bytes(raw[..8].try_into().ok()?)))
    }
}

impl std::ops::Deref for Counter {
    type Target = i64;
    fn deref(&self) -> &i64 {
        &self.0
    }
}

impl<H: Hasher, KF: KeyFunction<H, Key = Vec<u8>>> ReadWriteDB<H, KF> {
    const CNT_SUFFIX: [u8; 1] = [0x0; 1];
    const DATA_SUFFIX: [u8; 1] = [0x1; 1];

    fn new(prefix: Vec<u8>, db: DB) -> Self {
        let null_node_data = vec![0u8];
        Self {
            inner: Mutex::new(ReadWriteDBInner { db, wb: None }),
            prefix,
            null_node_hash: H::hash(&null_node_data),
            null_node_data,
            _kf: PhantomData,
        }
    }

    fn finalize_key(
        &self, key: &H::Out, prefix: hash_db::Prefix, suffix: &[u8],
    ) -> Vec<u8> {
        let mut buff = self.prefix.clone();
        buff.extend_from_slice(&KF::key(key, prefix));
        buff.extend_from_slice(suffix);
        buff
    }

    fn commit(&self) {
        let mut inner = self.inner.lock();
        if let Some(wb) = inner.wb.take() {
            inner.db.write(wb).unwrap();
        }
    }
}

impl<H: Hasher, KF: KeyFunction<H, Key = Vec<u8>> + Sync + Send>
    hash_db::AsHashDB<H, DBValue> for ReadWriteDB<H, KF>
{
    fn as_hash_db(&self) -> &dyn HashDB<H, DBValue> {
        self
    }
    fn as_hash_db_mut<'a>(
        &'a mut self,
    ) -> &'a mut (dyn HashDB<H, DBValue> + 'a) {
        self
    }
}

impl<H: Hasher, KF: KeyFunction<H, Key = Vec<u8>> + Sync + Send>
    HashDB<H, DBValue> for ReadWriteDB<H, KF>
{
    fn get(
        &self, key: &H::Out, prefix: (&[u8], Option<u8>),
    ) -> Option<DBValue> {
        if key == &self.null_node_hash {
            return Some(self.null_node_data.clone())
        }
        let cnt_key = self.finalize_key(key, prefix, &Self::CNT_SUFFIX);
        let inner = self.inner.lock();
        inner.db.get(&cnt_key).ok()?.and_then(|r| {
            let cnt = Counter::decode(&r).unwrap();
            if *cnt > 0 {
                let data_key =
                    self.finalize_key(key, prefix, &Self::DATA_SUFFIX);
                inner.db.get(&data_key).ok()?
            } else {
                None
            }
        })
    }
    fn contains(&self, key: &H::Out, prefix: (&[u8], Option<u8>)) -> bool {
        if key == &self.null_node_hash {
            return true
        }
        let cnt_key = self.finalize_key(key, prefix, &Self::CNT_SUFFIX);
        let inner = self.inner.lock();
        match inner.db.get(&cnt_key).ok() {
            Some(v) => match v {
                Some(v) => *Counter::decode(&v).unwrap() > 0,
                None => false,
            },
            None => false,
        }
    }
    fn emplace(
        &mut self, key: H::Out, prefix: (&[u8], Option<u8>), value: DBValue,
    ) {
        if value == self.null_node_data {
            return
        }
        let cnt_key = self.finalize_key(&key, prefix, &Self::CNT_SUFFIX);
        let data_key = self.finalize_key(&key, prefix, &Self::DATA_SUFFIX);
        let mut inner = self.inner.lock();
        match inner.db.get(&cnt_key).unwrap() {
            Some(v) => {
                let wb = inner.get_wb();
                let cnt = Counter::decode(&v).unwrap();
                if *cnt <= 0 {
                    wb.put(data_key, value);
                }
                wb.put(cnt_key, Counter(*cnt + 1).encode());
            }
            None => {
                let wb = inner.get_wb();
                wb.put(cnt_key, Counter(1).encode());
                wb.put(data_key, value);
            }
        }
    }
    fn insert(&mut self, prefix: (&[u8], Option<u8>), value: &[u8]) -> H::Out {
        if value == self.null_node_data {
            return self.null_node_hash
        }
        let key = H::hash(value);
        HashDB::emplace(self, key, prefix, value.into());
        key
    }
    fn remove(&mut self, _key: &H::Out, _prefix: (&[u8], Option<u8>)) {
        return
        // ignore remove ops to achieve a copy-on-write store
        /*
        if key == &self.null_node_hash {
            return
        }
        let cnt_key = self.finalize_key(key, prefix, &Self::CNT_SUFFIX);
        let data_key = self.finalize_key(key, prefix, &Self::DATA_SUFFIX);
        let mut inner = self.inner.lock();
        match inner.db.get(&cnt_key).unwrap() {
            Some(v) => {
                let wb = inner.get_wb();
                let cnt = Counter::decode(&v);
                wb.put(cnt_key, Counter(*cnt - 1).encode());
            }
            None => {
                let wb = inner.get_wb();
                wb.put(cnt_key, Counter(-1).encode());
                wb.put(data_key, DBValue::default());
            }
        }
        */
    }
}

impl<'a, H: Hasher, KF: KeyFunction<H, Key = Vec<u8>> + Sync + Send>
    HashDBRef<H, DBValue> for ReadWriteDB<H, KF>
{
    fn get(
        &self, key: &H::Out, prefix: (&[u8], Option<u8>),
    ) -> Option<DBValue> {
        HashDB::get(self, key, prefix)
    }
    fn contains(&self, key: &H::Out, prefix: (&[u8], Option<u8>)) -> bool {
        HashDB::contains(self, key, prefix)
    }
}

struct AccountState {
    root_hash: Hash,
    balance: Wei,
    nonce: u64,
    code: Vec<u8>,
}

impl AccountState {
    fn encode(&self) -> DBValue {
        let mut buff = Vec::new();
        buff.write(self.root_hash.as_bytes()).unwrap();
        buff.write(&qevm::common::Bytes32::from(self.balance.as_ref())[..])
            .unwrap();
        buff.write(&self.nonce.to_le_bytes()).unwrap();
        buff.write(&(self.code.len() as u64).to_le_bytes()).unwrap();
        buff.write(&self.code).unwrap();
        buff
    }

    fn decode(mut raw: &[u8]) -> Option<Self> {
        let mut buff = Vec::new();
        buff.resize(32, 0);
        raw.read_exact(&mut buff).ok()?; // read 32 bytes (root_hash)
        let root_hash = Hash::from_slice(&buff[..32]);
        raw.read_exact(&mut buff).ok()?; // read 32 bytes (balance)
        let balance = U256::from_big_endian(&buff[..32]).into();
        raw.read_exact(&mut buff[..8]).ok()?; // read 8 bytes (nonce)
        let nonce = u64::from_le_bytes(buff[..8].try_into().unwrap());
        raw.read_exact(&mut buff[..8]).ok()?; // read 8 bytes (length of code)
        let code_len = u64::from_le_bytes(buff[..8].try_into().unwrap());
        buff.resize(code_len as usize, 0);
        raw.read_exact(&mut buff).ok()?; // read `code_len` bytes
        let code = buff;
        Some(Self {
            root_hash,
            balance,
            nonce,
            code,
        })
    }
}

impl MerkleDB {
    pub fn new(rocksdb: DB, prefix: &[u8]) -> Self {
        Self(Arc::new(MerkleDBInner {
            db: SyncUnsafeCell::new(ReadWriteDB::new(prefix.to_vec(), rocksdb)),
        }))
    }

    pub fn get_reader<'a>(&'a self, root: Option<&Hash>) -> MerkleDBReader {
        let root = match root {
            Some(r) => r.clone().to_fixed_bytes(),
            None => [0; 32],
        };
        MerkleDBReader {
            root,
            mdb: self.0.clone(),
        }
    }
    pub fn db(&self) -> parking_lot::MappedMutexGuard<DB> {
        parking_lot::MutexGuard::map(self.0.get_inner().inner.lock(), |e| {
            &mut e.db
        })
    }
    pub fn current_writebatch(
        &self,
    ) -> parking_lot::MappedMutexGuard<WriteBatch> {
        parking_lot::MutexGuard::map(self.0.get_inner().inner.lock(), |e| {
            e.get_wb()
        })
    }

    pub fn prove(&self, keys: &[&[u8]], root: &[u8]) -> Vec<Vec<u8>> {
        let mut r = <RefHasher as Hasher>::Out::default();
        r.copy_from_slice(&root);
        let trie =
            TrieDB::<ExtensionLayout>::new(self.0.get_inner(), &r).unwrap();
        trie_db::proof::generate_proof(&trie, keys).unwrap()
    }
    pub fn verify(
        proof: &[Vec<u8>], items: &[(Vec<u8>, Option<Vec<u8>>)], root: &[u8],
    ) -> bool {
        let mut r = <RefHasher as Hasher>::Out::default();
        r.copy_from_slice(&root);
        trie_db::proof::verify_proof::<ExtensionLayout, _, _, _>(
            &r, proof, items,
        )
        .is_ok()
    }
    pub fn insert(
        &mut self, key: &[u8], val: &[u8], root: Option<&[u8]>,
    ) -> [u8; 32] {
        let wdb = self.0.get_inner_mut();
        let mut new_root = <RefHasher as Hasher>::Out::default();
        let mut trie = match root {
            Some(r) => {
                new_root.copy_from_slice(&r);
                TrieDBMut::<ExtensionLayout>::from_existing(wdb, &mut new_root)
                    .unwrap()
            }
            None => TrieDBMut::<ExtensionLayout>::new(wdb, &mut new_root),
        };
        trie.insert(key, val).unwrap();
        drop(trie);
        wdb.commit();
        new_root
    }

    pub fn get(&self, key: &[u8], root: &[u8; 32]) -> Option<Vec<u8>> {
        self.0.get(key, root)
    }
    pub fn contains(&self, key: &[u8], root: &[u8; 32]) -> bool {
        self.0.contains(key, root)
    }

    pub fn commit_writebatch(&self) {
        self.0.get_inner_mut().commit();
    }
}

impl MerkleDBInner {
    fn prefix_key(addr: &Addr, key: &Hash) -> Vec<u8> {
        let mut prefixed_key = Vec::new();
        prefixed_key.write(addr.as_bytes()).unwrap();
        prefixed_key.write(key.as_bytes()).unwrap();
        prefixed_key
    }
    fn get_inner(&self) -> &ReadWriteDB<RefHasher, PrefixedKey<RefHasher>> {
        unsafe { &*self.db.get() }
    }
    fn get_inner_mut(
        &self,
    ) -> &mut ReadWriteDB<RefHasher, PrefixedKey<RefHasher>> {
        unsafe { &mut *self.db.get() }
    }

    fn get(&self, key: &[u8], root: &[u8; 32]) -> Option<Vec<u8>> {
        let trie =
            TrieDB::<ExtensionLayout>::new(self.get_inner(), root).ok()?;
        trie.get(key).ok()?
    }
    fn contains(&self, key: &[u8], root: &[u8; 32]) -> bool {
        let trie =
            TrieDB::<ExtensionLayout>::new(self.get_inner(), root).unwrap();
        trie.contains(key).unwrap()
    }
}

pub struct MerkleDBReader {
    root: [u8; 32],
    mdb: Arc<MerkleDBInner>,
}

#[async_trait]
impl AccountRootR<Hash> for MerkleDBReader {
    async fn get_account_root(&self, contract: &Addr) -> Hash {
        self.mdb
            .get(contract.as_bytes(), &self.root)
            .map(|raw| {
                let acc = AccountState::decode(&raw).unwrap();
                acc.root_hash
            })
            .unwrap_or_else(|| Hash::zero().clone())
    }
}

#[async_trait]
impl WorldStateR for MerkleDBReader {
    async fn get_state(&self, contract: &Addr, key: &Hash) -> U256 {
        self.mdb
            .get(contract.as_bytes(), &self.root)
            .and_then(|raw| {
                let acc = AccountState::decode(&raw).unwrap();
                self.mdb.get(
                    &MerkleDBInner::prefix_key(contract, key),
                    acc.root_hash.as_bytes().try_into().unwrap(),
                )
            })
            .map(|val| U256::from_big_endian(&val))
            .unwrap_or_else(|| U256::zero())
    }
    async fn get_balance(&self, account: &Addr) -> Wei {
        self.mdb
            .get(account.as_bytes(), &self.root)
            .map(|raw| {
                let acc = AccountState::decode(&raw).unwrap();
                acc.balance
            })
            .unwrap_or_else(|| Wei::zero().clone())
    }
    async fn get_code(&self, contract: &Addr) -> Arc<dyn qevm::core::Code> {
        self.mdb
            .get(contract.as_bytes(), &self.root)
            .map(|raw| {
                let acc = AccountState::decode(&raw).unwrap();
                Arc::new(PlainCode::new(acc.code.into()))
                    as Arc<dyn qevm::core::Code>
            })
            .unwrap_or_else(qevm::state::empty_code)
    }
    async fn get_nonce(&self, account: &Addr) -> u64 {
        self.mdb
            .get(account.as_bytes(), &self.root)
            .map(|raw| {
                let acc = AccountState::decode(&raw).unwrap();
                acc.nonce
            })
            .unwrap_or(0)
    }
    async fn exist(&self, account: &Addr) -> bool {
        self.mdb.contains(account.as_bytes(), &self.root)
    }
}

#[async_trait]
impl WorldStateStore for MerkleDB {
    // TODO: improve these types
    type Error = ();
    type StateRoot = Hash;
    type AccountRoot = Hash;
    fn read(&self, root: Option<&Self::StateRoot>) -> Arc<dyn WorldStateR> {
        Arc::new(self.get_reader(root))
    }
    async fn commit(
        &self, root: Option<&Self::StateRoot>,
        deltas: &qevm::state::StateDelta,
        reader: &dyn AccountRootR<Hash>,
    ) -> Result<Self::StateRoot, Self::Error> {
        // TODO: log items are ignored for now
        let accounts = futures::future::join_all(deltas.accounts.iter().map(
            |(addr, acc)| async {
                let addr = addr.clone();
                let balance = match &acc.balance {
                    Some(b) => b.clone(),
                    None => reader.get_balance(&addr).await,
                };
                let nonce = match acc.nonce {
                    Some(n) => n,
                    None => reader.get_nonce(&addr).await,
                };
                // TODO: improve the efficiency here so the code does not have to be written each time
                // (reduce the amplification)
                let code = match &acc.code {
                    Some(c) => c.clone(),
                    None => reader.get_code(&addr).await,
                }
                .as_bytes()
                .to_vec();
                let old_root = reader.get_account_root(&addr).await;
                (addr, balance, nonce, code, &acc.state, old_root)
            },
        ))
        .await;
        let wdb = self.0.get_inner_mut();
        let accounts: Vec<_> = accounts
            .into_iter()
            .map(|(addr, balance, nonce, code, state, old_root)| {
                let mut new_root;
                {
                    let mut trie = if &old_root == Hash::zero() {
                        new_root = <RefHasher as Hasher>::Out::default();
                        TrieDBMut::<ExtensionLayout>::new(wdb, &mut new_root)
                    } else {
                        new_root = old_root.to_fixed_bytes();
                        TrieDBMut::<ExtensionLayout>::from_existing(
                            wdb,
                            &mut new_root,
                        )
                        .unwrap()
                    };
                    for (key, val) in state {
                        trie.insert(
                            &MerkleDBInner::prefix_key(&addr, key),
                            &qevm::common::Bytes32::from(val)[..],
                        )
                        .unwrap();
                    }
                }

                let acc = AccountState {
                    root_hash: new_root.into(),
                    balance,
                    nonce,
                    code,
                };
                (addr, acc)
            })
            .collect();
        let mut new_root = <RefHasher as Hasher>::Out::default();
        {
            let mut trie = match root {
                Some(r) => {
                    new_root.copy_from_slice(r.as_bytes());
                    TrieDBMut::<ExtensionLayout>::from_existing(
                        wdb,
                        &mut new_root,
                    )
                    .unwrap()
                }
                None => TrieDBMut::<ExtensionLayout>::new(wdb, &mut new_root),
            };
            for (addr, acc) in accounts.into_iter() {
                trie.insert(addr.as_bytes(), &acc.encode()).unwrap();
            }
        }
        Ok(new_root.into())
    }
}
