use std::sync::Arc;

use qevm::common::*;
use qevm::core::{WorldState, WorldStateR, WorldStateW};
use qevm::state::*;
use qevm_tools::dummy::DummyStateStore;

#[tokio::test]
async fn test_special_cases() {
    let null = Arc::new(EmptyState);
    // [s0] <- [s1: addr0(0 => 1)] <---[s2:addr0(1 => 2)] <- [s4:addr0(0 => 3)]
    //                             <-
    //                               \_[s3:addr0(2 => 3)] <- [s5:addr0(0 => 2)] <- [s6:addr1(0 => 2)]
    let s0 = MemStateShared::new(null);
    let addr0 = Addr::zero();
    let u0: U256 = 0.into();
    let u1: U256 = 1.into();
    let u2: U256 = 2.into();
    let u3: U256 = 3.into();
    let addr1 = u1.into();

    let mut m1 = MemStateMut::from_shared(&s0);
    m1.set_state(addr0, &u0.into(), &u1);
    let mut s1: MemStateShared = m1.into();
    let mut m2 = MemStateMut::from_shared(&s1);
    m2.set_state(addr0, &u1.into(), &u2);
    let mut s2: MemStateShared = m2.into();

    let mut m3 = MemStateMut::from_shared(&s1); // make a fork
    m3.set_state(addr0, &u2.into(), &u3);
    let mut s3: MemStateShared = m3.into();

    let mut m4 = MemStateMut::from_shared(&s2);
    m4.set_state(addr0, &u0.into(), &u3);
    let mut s4: MemStateShared = m4.into();

    let mut m5 = MemStateMut::from_shared(&s3);
    m5.set_state(addr0, &u0.into(), &u2);
    let mut s5: MemStateShared = m5.into();

    let mut m6 = MemStateMut::from_shared(&s5);
    m6.set_state(&addr1, &u0.into(), &u2);
    let s6: MemStateShared = m6.into();
    assert_eq!(s2.get_state(addr0, &u0.into()).await, u1);
    assert_eq!(s2.get_state(&addr1, &u0.into()).await, u0);
    assert_eq!(s6.get_state(&addr1, &u0.into()).await, u2);

    assert!(s0.consolidated_delta().is_some());
    for s in [&s1, &s2, &s3, &s4, &s5] {
        assert!(s.consolidated_delta().is_none());
    }

    assert_eq!(s2.get_state(addr0, &u0.into()).await, u1);
    assert_eq!(s2.get_state(addr0, &u1.into()).await, u2);
    assert_eq!(s3.get_state(addr0, &u2.into()).await, u3);
    assert_eq!(s4.get_state(addr0, &u0.into()).await, u3);
    assert_eq!(s5.get_state(addr0, &u0.into()).await, u2);

    for s in [&mut s1, &mut s2, &mut s3, &mut s4, &mut s5]
        .into_iter()
        .rev()
    {
        s.consolidate(None);
    }

    for s in [&s1, &s2, &s3, &s4, &s5] {
        assert!(s.consolidated_delta().is_some());
    }

    assert_eq!(s2.get_state(addr0, &u0.into()).await, u1);
    assert_eq!(s2.get_state(addr0, &u1.into()).await, u2);
    assert_eq!(s3.get_state(addr0, &u2.into()).await, u3);
    assert_eq!(s4.get_state(addr0, &u0.into()).await, u3);
    assert_eq!(s5.get_state(addr0, &u0.into()).await, u2);
}

#[tokio::test]
async fn test_deleted_accounts() {
    let null = Arc::new(EmptyState);
    // [s0] <- [s1: addr0(0 => 1, addr1(0 => 2))] <---[s2:addr0(0 => 1)] <- [s3:addr0(0 => 1, addr1(2 => 1)]
    let s0 = MemStateShared::new(null);
    let addr0 = Addr::zero();
    let u0: U256 = 0.into();
    let u1: U256 = 1.into();
    let u2: U256 = 2.into();
    let addr1 = u1.into();

    let mut m1 = MemStateMut::from_shared(&s0);
    m1.set_state(addr0, &u0.into(), &u1);
    m1.set_state(&addr1, &u0.into(), &u2);
    let mut s1: MemStateShared = m1.into();
    let mut m2 = MemStateMut::from_shared(&s1);
    m2.delete_account(&addr1);
    let mut s2: MemStateShared = m2.into();

    let mut m3 = MemStateMut::from_shared(&s2);
    m3.set_state(&addr1, &u2.into(), &u1);
    let mut s3: MemStateShared = m3.into();

    assert_eq!(s1.get_state(addr0, &u0.into()).await, u1);
    assert_eq!(s1.get_state(&addr1, &u0.into()).await, u2);
    assert_eq!(s2.get_state(addr0, &u0.into()).await, u1);
    assert_eq!(s2.exist(&addr1).await, false);
    assert_eq!(s3.get_state(addr0, &u0.into()).await, u1);
    assert_eq!(s3.get_state(&addr1, &u2.into()).await, u1);

    assert!(s0.consolidated_delta().is_some());
    for s in [&s1, &s2, &s3] {
        assert!(s.consolidated_delta().is_none());
    }

    for s in [&mut s1, &mut s2, &mut s3].into_iter().rev() {
        s.consolidate(None);
    }

    for s in [&s1, &s2, &s3] {
        assert!(s.consolidated_delta().is_some());
    }

    assert_eq!(s1.get_state(addr0, &u0.into()).await, u1);
    assert_eq!(s1.get_state(&addr1, &u0.into()).await, u2);
    assert_eq!(s2.get_state(addr0, &u0.into()).await, u1);
    assert_eq!(s2.exist(&addr1).await, false);
    assert_eq!(s3.get_state(addr0, &u0.into()).await, u1);
    assert_eq!(s3.get_state(&addr1, &u2.into()).await, u1);
}

async fn check_same<S: WorldState>(s: &S, s0: &DummyStateStore) -> bool {
    for acc in s0.accounts() {
        if s0.get_balance(acc).await != s.get_balance(acc).await ||
            s0.get_nonce(acc).await != s.get_nonce(acc).await ||
            s0.get_code(acc).await.as_bytes() !=
                s.get_code(acc).await.as_bytes()
        {
            return false
        }
        for key in s0.account_keys(acc).unwrap() {
            if s0.get_state(acc, key).await != s.get_state(acc, key).await {
                return false
            }
        }
    }
    true
}

#[tokio::test]
async fn test_random_cross_validate() {
    use rand::{Rng, SeedableRng};
    use sha3::Digest;
    let mut states = Vec::new();
    let addr_range = 10;
    let key_range = 100;
    let total_iter = 50000;
    let max_change = 10;
    let mut rng = rand::rngs::StdRng::from_seed([0; 32]);
    let mut next_val: U256 = 1.into();
    let mut next_balance: U256 = 1.into();
    let mut next_nonce = 1;
    let mut next_code = 0u64;
    states.push((
        MemStateAuto::new(Arc::new(EmptyState)),
        DummyStateStore::new(),
    ));
    for _ in 0..total_iter {
        let chosen = rng.gen_range(0..states.len());
        let mut s = states[chosen].0.snapshot();
        let mut s0 = states[chosen].1.snapshot();
        for _ in 0..rng.gen_range(0..max_change) {
            let addr: Addr = U256::from(rng.gen_range(0..addr_range)).into();
            match rng.gen_range(0.0..1.0) {
                r if r > 0.6 => {
                    let key: Hash =
                        U256::from(rng.gen_range(0..key_range)).into();
                    s.set_state(&addr, &key, &next_val);
                    s0.set_state(&addr, &key, &next_val);
                    next_val += 1.into();
                }
                r if r > 0.4 => {
                    let nb = next_balance.into();
                    s.set_balance(&addr, &nb);
                    s0.set_balance(&addr, &nb);
                    next_balance += 1.into();
                }
                r if r > 0.2 => {
                    s.set_nonce(&addr, next_nonce);
                    s0.set_nonce(&addr, next_nonce);
                    next_nonce += 1;
                }
                _ => {
                    let d = sha3::Keccak256::digest(next_code.to_le_bytes());
                    s.set_code(&addr, d.as_slice());
                    s0.set_code(&addr, d.as_slice());
                    next_code += 1;
                }
            }
        }
        assert!(check_same(&s, &s0).await);
        states.push((s, s0));
    }
    let mut avg_depth = 0;
    for (s, s0) in states.iter() {
        let mut s = s.to_shared();
        let d = s.depth();
        avg_depth += d;
        s.consolidate(None);
        assert!(s.depth() == 0);
        let s: MemStateAuto = s.into();
        assert!(check_same(&s, s0).await);
    }
    println!(
        "avg. depth = {:.2}",
        avg_depth as f32 / (states.len() as f32)
    );
}
