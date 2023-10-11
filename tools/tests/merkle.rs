use qevm::common::*;
use qevm::core::{WorldStateR, WorldStateW};
use qevm::state::*;
use qevm_tools::dummy::DummyStateStore;
use qevm_tools::merkle::MerkleDB;

fn check_item(mdb: &MerkleDB, root: &[u8; 32], key: &[u8], val: Option<&[u8]>) {
    let ret = mdb.get(key, root);
    assert!(match val {
        Some(val) =>
            if let Some(v) = ret {
                v == val
            } else {
                false
            },
        None => ret.is_none(),
    });
}

fn print_proof(p: &[Vec<u8>]) {
    println!("proof(");
    for t in p {
        println!("{:x}", BytesRef::from(&t[..]));
    }
    println!(")");
}

#[actix_rt::test]
async fn test_merkle_basic() {
    let path = "./merkle-test";
    rocksdb::DB::destroy(&rocksdb::Options::default(), path).unwrap();
    let (root0, root1, root2, root3);
    {
        let db = rocksdb::DB::open_default(path).unwrap();
        let mut mdb = MerkleDB::new(db, b"merkle");
        root0 = mdb.insert(b"a", b"hello", None);
        println!("{:x}", BytesRef::from(&root0[..]));
        root1 = mdb.insert(b"b", b"world", Some(&root0));
        println!("{:x}", BytesRef::from(&root1[..]));
        root2 = mdb.insert(b"a", b"hello2", Some(&root0));
        println!("{:x}", BytesRef::from(&root2[..]));
        root3 = mdb.insert(b"b", b"world2", Some(&root1));
        println!("{:x}", BytesRef::from(&root3[..]));
    }

    let db = rocksdb::DB::open_default(path).unwrap();
    let mdb = MerkleDB::new(db, b"merkle");

    check_item(&mdb, &root0, b"a", Some(b"hello"));
    check_item(&mdb, &root0, b"b", None);
    check_item(&mdb, &root0, b"c", None);

    check_item(&mdb, &root1, b"a", Some(b"hello"));
    check_item(&mdb, &root1, b"b", Some(b"world"));
    check_item(&mdb, &root1, b"c", None);

    check_item(&mdb, &root2, b"a", Some(b"hello2"));
    check_item(&mdb, &root2, b"b", None);
    check_item(&mdb, &root2, b"c", None);

    check_item(&mdb, &root3, b"a", Some(b"hello"));
    check_item(&mdb, &root3, b"b", Some(b"world2"));
    check_item(&mdb, &root3, b"c", None);

    let p0 = mdb.prove(&[b"a", b"b", b"c"], &root0);
    assert!(MerkleDB::verify(
        &p0,
        &[
            (b"a".to_vec(), Some(b"hello".to_vec())),
            (b"b".to_vec(), None),
            (b"c".to_vec(), None),
        ],
        &root0
    ));
    print_proof(&p0);

    let p1 = mdb.prove(&[b"a", b"b", b"c"], &root1);
    assert!(MerkleDB::verify(
        &p1,
        &[
            (b"a".to_vec(), Some(b"hello".to_vec())),
            (b"b".to_vec(), Some(b"world".to_vec())),
            (b"c".to_vec(), None),
        ],
        &root1
    ));
    print_proof(&p1);

    let p2 = mdb.prove(&[b"a", b"b", b"c"], &root2);
    assert!(MerkleDB::verify(
        &p2,
        &[
            (b"a".to_vec(), Some(b"hello2".to_vec())),
            (b"b".to_vec(), None),
            (b"c".to_vec(), None),
        ],
        &root2
    ));
    print_proof(&p2);

    let p3 = mdb.prove(&[b"a", b"b", b"c"], &root3);
    assert!(MerkleDB::verify(
        &p3,
        &[
            (b"a".to_vec(), Some(b"hello".to_vec())),
            (b"b".to_vec(), Some(b"world2".to_vec())),
            (b"c".to_vec(), None),
        ],
        &root3
    ));
    print_proof(&p3);
}

// TODO: this code is pasted from qevm crate, try to keep one
async fn check_same<S: WorldStateR + ?Sized>(
    s: &S, s0: &DummyStateStore,
) -> bool {
    let mut nitems = 0;
    for acc in s0.accounts() {
        if s0.get_balance(acc).await != s.get_balance(acc).await ||
            s0.get_nonce(acc).await != s.get_nonce(acc).await ||
            s0.get_code(acc).await.as_bytes() !=
                s.get_code(acc).await.as_bytes()
        {
            return false
        }
        for key in s0.account_keys(acc).unwrap() {
            nitems += 1;
            if s0.get_state(acc, key).await != s.get_state(acc, key).await {
                return false
            }
        }
    }
    println!("checked {} accounts and {} items", s0.len(), nitems);
    true
}

async fn rand_state<'a>(
    s: &mut MemStateAuto<'a>, s0: &mut DummyStateStore, seed: u64,
    addr_range: u64, key_range: u64, total_iter: usize, max_change: usize,
) {
    use rand::{Rng, SeedableRng};
    use sha3::Digest;
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let mut next_val: U256 = 1.into();
    let mut next_balance: U256 = 1.into();
    let mut next_nonce = 1;
    let mut next_code = 0u64;
    for _ in 0..total_iter {
        for _ in 0..rng.gen_range(0..max_change) {
            let addr: Addr = U256::from(rng.gen_range(0..addr_range)).into();
            match rng.gen_range(0.0..1.0) {
                r if r > 0.6 => {
                    let key: Hash =
                        U256::from(rng.gen_range(0..key_range)).into();
                    let val = s.get_state(&addr, &key).await;
                    let val0 = s0.get_state(&addr, &key).await;
                    s.set_state(&addr, &key, &(val + next_val));
                    s0.set_state(&addr, &key, &(val0 + next_val));
                    next_val += 1.into();
                }
                r if r > 0.4 => {
                    let nb = next_balance.clone().into();
                    let b = s.get_balance(&addr).await;
                    let b0 = s0.get_balance(&addr).await;
                    s.set_balance(&addr, &(b.checked_add(&nb).unwrap()));
                    s0.set_balance(&addr, &(b0.checked_add(&nb).unwrap()));
                    next_balance += 1.into();
                }
                r if r > 0.2 => {
                    s.set_nonce(&addr, next_nonce);
                    s0.set_nonce(&addr, next_nonce);
                    next_nonce += 1;
                }
                _ => {
                    let d = sha3::Keccak256::digest(&next_code.to_le_bytes());
                    s.set_code(&addr, d.as_slice());
                    s0.set_code(&addr, d.as_slice());
                    next_code += 1;
                }
            }
        }
    }
}

#[actix_rt::test]
async fn test_random_cross_validate() {
    let path = "./merkle-random";
    let mut rstate = DummyStateStore::new();
    let mut root = None;
    rocksdb::DB::destroy(&rocksdb::Options::default(), path).unwrap();
    for seed in 0..5 {
        root = {
            let root = root.as_ref();
            let db = rocksdb::DB::open_default(path).unwrap();
            let mdb = MerkleDB::new(db, b"merkle");
            let mreader = mdb.read(root);
            let mut s = MemStateAuto::new(mreader);
            rand_state(&mut s, &mut rstate, seed, 1000, 100, 1000, 10).await;
            let mut s = s.to_shared();
            s.consolidate(None);
            let res = Some(
                mdb.commit(
                    root,
                    s.consolidated_delta().unwrap(),
                    &mdb.get_reader(root),
                )
                .await
                .unwrap(),
            );
            mdb.commit_writebatch();
            res
        };
        let db = rocksdb::DB::open_default(path).unwrap();
        let mdb = MerkleDB::new(db, b"merkle");
        let mreader = mdb.read(root.as_ref());
        assert!(check_same(mreader.as_ref(), &rstate).await);
    }
}
