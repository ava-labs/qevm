use std::io::Write;

use once_cell::sync::OnceCell;
use rlp_derive::{RlpDecodable, RlpEncodable};
use sha3::Digest;

use crate::common::{
    u256_1, Addr, Bytes, Gas, Hash, NullableAddr, Wei, U256, U256RLP,
};

#[derive(PartialEq, Eq)]
pub enum TxType {
    Legacy = 0x0,
    AccessList,
    DynamicFee,
}

pub struct Tx {
    inner: Box<dyn TxLike>,
    tx_hash: Hash,
    from: Addr,
}

// `Tx` is read-only
unsafe impl Sync for Tx {}

impl std::ops::Deref for Tx {
    type Target = dyn TxLike + 'static;
    fn deref(&self) -> &(dyn TxLike + 'static) {
        &*self.inner
    }
}

impl Tx {
    fn new<T: TxLike + 'static>(
        tx: T, tx_hash: Hash, chain_id: &U256,
    ) -> Option<Self> {
        let from = tx.recover_sender(chain_id)?;
        Some(Self {
            inner: Box::new(tx),
            tx_hash,
            from,
        })
    }

    pub fn from(&self) -> &Addr {
        &self.from
    }

    pub fn hash(&self) -> &Hash {
        &self.tx_hash
    }

    const ACCESS_LIST: u8 = 1;
    const DYNAMIC_FEE: u8 = 2;

    pub fn decode(bytes: &[u8], chain_id: &U256) -> Option<Tx> {
        let rlp = rlp::Rlp::new(bytes);
        let tx_hash = Hash::hash(bytes);
        if rlp.is_list() {
            rlp.as_val()
                .ok()
                .and_then(|tx: TxLegacy| Tx::new(tx, tx_hash, chain_id))
        } else if rlp.is_data() {
            let bytes = rlp.as_raw();
            if bytes.len() == 0 {
                return None
            }
            let rlp = rlp::Rlp::new(&bytes[1..]);
            match bytes[0] {
                Self::ACCESS_LIST => {
                    rlp.as_val().ok().and_then(|tx: TxAccessList| {
                        Tx::new(tx, tx_hash, chain_id)
                    })
                }
                Self::DYNAMIC_FEE => {
                    rlp.as_val().ok().and_then(|tx: TxDynamicFee| {
                        Tx::new(tx, tx_hash, chain_id)
                    })
                }
                _ => None,
            }
        } else {
            None
        }
    }
}

pub trait TxLike: Send + std::fmt::Debug {
    fn encode(&self) -> Vec<u8>;
    fn type_(&self) -> TxType;
    fn chain_id(&self) -> U256;
    fn access_list(&self) -> Option<&[AccessTuple]>;
    fn data(&self) -> &Bytes;
    fn gas(&self) -> Gas;
    fn gas_fee_cap(&self) -> &Wei;
    fn gas_tip_cap(&self) -> &Wei;
    fn gas_price(&self) -> &Wei;
    fn value(&self) -> &Wei;
    fn nonce(&self) -> u64;
    fn to(&self) -> Option<&Addr>;
    fn v(&self) -> &U256;
    fn r(&self) -> &U256;
    fn s(&self) -> &U256;
    fn sig_hash(&self, chain_id: U256) -> Hash;
    fn protected(&self) -> bool {
        true
    }

    fn recover_sender(&self, chain_id: &U256) -> Option<Addr> {
        if &self.chain_id() != chain_id {
            return None
        }
        let t = self.type_();
        let mut v = self.v().clone();
        let r = self.r().clone();
        let s = self.s().clone();
        match t {
            TxType::DynamicFee | TxType::AccessList => {
                v += 27u64.into();
                recover_plain(&self.sig_hash(chain_id.clone()), r, s, v, true)
            }
            TxType::Legacy => {
                if !self.protected() {
                    // Homestead signer Hash()
                    let mut stream = rlp::RlpStream::new_list(6);
                    stream
                        .append(&self.nonce())
                        .append(self.gas_price())
                        .append(&self.gas());
                    match self.to() {
                        Some(addr) => stream.append(addr),
                        None => stream.append_empty_data(),
                    }
                    .append(self.value())
                    .append(self.data());
                    let mut hasher = sha3::Keccak256::new();
                    hasher.update(&[self.type_() as u8]);
                    hasher.update(stream.out());
                    let sig_hash =
                        Hash::from_slice(hasher.finalize().as_slice());
                    return recover_plain(&sig_hash, r, s, v, true)
                }
                let mut cm = *chain_id;
                cm <<= 1;
                v -= cm;
                v -= 8u64.into();
                recover_plain(&self.sig_hash(chain_id.clone()), r, s, v, true)
            }
        }
    }
}

#[derive(RlpDecodable, RlpEncodable, Debug)]
pub struct TxLegacy {
    nonce: u64,
    gas_price: Wei,
    gas: Gas,
    to: NullableAddr,
    value: Wei,
    data: Bytes,
    v: U256RLP,
    r: U256RLP,
    s: U256RLP,
}

impl TxLegacy {
    fn derive_chain_id(&self) -> U256 {
        let v = &self.v.0;
        if v.bits() <= 64 {
            let v = v.low_u64();
            if v == 27 || v == 28 {
                return U256::zero()
            }
            return ((v - 35) / 2).into()
        }
        (v - 35) / 2
    }
}

impl TxLike for TxLegacy {
    fn encode(&self) -> Vec<u8> {
        rlp::encode(self).as_mut().to_vec()
    }

    fn type_(&self) -> TxType {
        TxType::Legacy
    }
    fn chain_id(&self) -> U256 {
        self.derive_chain_id()
    }
    fn access_list(&self) -> Option<&[AccessTuple]> {
        None
    }
    fn data(&self) -> &Bytes {
        &self.data
    }
    fn gas(&self) -> Gas {
        self.gas
    }
    fn gas_fee_cap(&self) -> &Wei {
        &self.gas_price
    }
    fn gas_tip_cap(&self) -> &Wei {
        &self.gas_price
    }
    fn gas_price(&self) -> &Wei {
        &self.gas_price
    }
    fn value(&self) -> &Wei {
        &self.value
    }
    fn nonce(&self) -> u64 {
        self.nonce
    }
    fn to(&self) -> Option<&Addr> {
        self.to.0.as_ref()
    }
    fn v(&self) -> &U256 {
        &self.v.0
    }
    fn r(&self) -> &U256 {
        &self.r.0
    }
    fn s(&self) -> &U256 {
        &self.s.0
    }
    fn sig_hash(&self, chain_id: U256) -> Hash {
        let mut stream = rlp::RlpStream::new_list(9);
        stream
            .append(&self.nonce)
            .append(&self.gas_price)
            .append(&self.gas);
        match &self.to.0 {
            Some(addr) => stream.append(addr),
            None => stream.append_empty_data(),
        }
        .append(&self.value)
        .append(&self.data)
        .append(&U256RLP(chain_id))
        .append(&0u64)
        .append(&0u64);
        Hash::hash(&stream.out())
    }
    fn protected(&self) -> bool {
        // NOTE: check if self.v() is absent if v becomes nullable in the future
        let v = self.v();
        if v.bits() <= 8 {
            let v = v.low_u64();
            return v != 27 && v != 28 && v != 1 && v != 0
        }
        true
    }
}

#[derive(RlpDecodable, RlpEncodable, Debug)]
pub struct TxDynamicFee {
    chain_id: U256RLP,
    nonce: u64,
    gas_tip_cap: Wei,
    gas_fee_cap: Wei,
    gas: Gas,
    to: NullableAddr,
    value: Wei,
    data: Bytes,
    access_list: Vec<AccessTuple>,
    v: U256RLP,
    r: U256RLP,
    s: U256RLP,
}

impl TxLike for TxDynamicFee {
    fn encode(&self) -> Vec<u8> {
        let mut rlp0 = rlp::RlpStream::new();
        rlp0.append(self);
        let mut buff = vec![Tx::DYNAMIC_FEE];
        buff.write(rlp0.out().as_ref()).unwrap();
        let mut rlp = rlp::RlpStream::new();
        rlp.append_raw(&buff, 1);
        rlp.out().as_ref().to_vec()
    }

    fn type_(&self) -> TxType {
        TxType::DynamicFee
    }
    fn chain_id(&self) -> U256 {
        self.chain_id.0
    }
    fn access_list(&self) -> Option<&[AccessTuple]> {
        Some(&self.access_list)
    }
    fn data(&self) -> &Bytes {
        &self.data
    }
    fn gas(&self) -> Gas {
        self.gas
    }
    fn gas_fee_cap(&self) -> &Wei {
        &self.gas_fee_cap
    }
    fn gas_tip_cap(&self) -> &Wei {
        &self.gas_tip_cap
    }
    fn gas_price(&self) -> &Wei {
        &self.gas_fee_cap
    }
    fn value(&self) -> &Wei {
        &self.value
    }
    fn nonce(&self) -> u64 {
        self.nonce
    }
    fn to(&self) -> Option<&Addr> {
        self.to.0.as_ref()
    }
    fn v(&self) -> &U256 {
        &self.v.0
    }
    fn r(&self) -> &U256 {
        &self.r.0
    }
    fn s(&self) -> &U256 {
        &self.s.0
    }
    fn sig_hash(&self, chain_id: U256) -> Hash {
        let mut stream = rlp::RlpStream::new_list(9);
        stream
            .append(&U256RLP(chain_id))
            .append(&self.nonce)
            .append(&self.gas_tip_cap)
            .append(&self.gas_fee_cap)
            .append(&self.gas);
        match &self.to.0 {
            Some(addr) => stream.append(addr),
            None => stream.append_empty_data(),
        }
        .append(&self.value)
        .append(&self.data)
        .append(&AccessListEncoder(&self.access_list));
        let mut hasher = sha3::Keccak256::new();
        hasher.update(&[self.type_() as u8]);
        hasher.update(stream.out());
        Hash::from_slice(hasher.finalize().as_slice())
    }
}

#[derive(RlpDecodable, RlpEncodable, Debug)]
pub struct TxAccessList {
    chain_id: U256RLP,
    nonce: u64,
    gas_price: Wei,
    gas: Gas,
    to: NullableAddr,
    value: Wei,
    data: Bytes,
    access_list: Vec<AccessTuple>,
    v: U256RLP,
    r: U256RLP,
    s: U256RLP,
}

impl TxLike for TxAccessList {
    fn encode(&self) -> Vec<u8> {
        let mut buff = vec![Tx::ACCESS_LIST];
        buff.write(rlp::encode(self).as_ref()).unwrap();
        let mut rlp = rlp::RlpStream::new();
        rlp.append_raw(&buff, 1);
        rlp.out().as_ref().to_vec()
    }

    fn type_(&self) -> TxType {
        TxType::AccessList
    }
    fn chain_id(&self) -> U256 {
        self.chain_id.0
    }
    fn access_list(&self) -> Option<&[AccessTuple]> {
        Some(&self.access_list)
    }
    fn data(&self) -> &Bytes {
        &self.data
    }
    fn gas(&self) -> Gas {
        self.gas
    }
    fn gas_fee_cap(&self) -> &Wei {
        &self.gas_price
    }
    fn gas_tip_cap(&self) -> &Wei {
        &self.gas_price
    }
    fn gas_price(&self) -> &Wei {
        &self.gas_price
    }
    fn value(&self) -> &Wei {
        &self.value
    }
    fn nonce(&self) -> u64 {
        self.nonce
    }
    fn to(&self) -> Option<&Addr> {
        self.to.0.as_ref()
    }
    fn v(&self) -> &U256 {
        &self.v.0
    }
    fn r(&self) -> &U256 {
        &self.r.0
    }
    fn s(&self) -> &U256 {
        &self.s.0
    }
    fn sig_hash(&self, chain_id: U256) -> Hash {
        let mut stream = rlp::RlpStream::new_list(8);
        stream
            .append(&U256RLP(chain_id))
            .append(&self.nonce)
            .append(&self.gas_price)
            .append(&self.gas);
        match &self.to.0 {
            Some(addr) => stream.append(addr),
            None => stream.append_empty_data(),
        }
        .append(&self.value)
        .append(&self.data)
        .append(&AccessListEncoder(&self.access_list));
        let mut hasher = sha3::Keccak256::new();
        hasher.update(&[self.type_() as u8]);
        hasher.update(stream.out());
        Hash::from_slice(hasher.finalize().as_slice())
    }
}

// TODO: AccessTuple is currently a dummy struct
#[derive(RlpDecodable, RlpEncodable, Debug)]
pub struct AccessTuple;

struct AccessListEncoder<'a>(&'a [AccessTuple]);

impl<'a> rlp::Encodable for AccessListEncoder<'a> {
    fn rlp_append(&self, s: &mut rlp::RlpStream) {
        let mut s = s.begin_list(self.0.len());
        for a in self.0.iter() {
            s = s.append(a);
        }
    }
}

#[inline]
fn secp256k1_n() -> &'static U256 {
    use std::str::FromStr;
    static V: OnceCell<U256> = OnceCell::new();
    V.get_or_init(|| U256::from_str("0xfffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141").unwrap())
}

#[inline]
fn secp256k1_half_n() -> &'static U256 {
    static V: OnceCell<U256> = OnceCell::new();
    V.get_or_init(|| secp256k1_n() / 2)
}

fn recover_plain(
    tx_hash: &Hash, r: U256, s: U256, vb: U256, homestead: bool,
) -> Option<Addr> {
    use crate::common::Bytes32;
    if vb.bits() > 8 {
        return None
    }
    let v = vb.low_u64() - 27;
    // `ValidateSignatureValues` in geth
    if &r < u256_1() || &s < u256_1() {
        return None
    }
    if homestead && &s > secp256k1_half_n() {
        return None
    }
    if &r >= secp256k1_n() || &s >= secp256k1_n() || (v != 0 && v != 1) {
        return None
    }
    //
    let r: Bytes32 = (&r).into();
    let s: Bytes32 = (&s).into();
    let mut r1 = libsecp256k1::curve::Scalar([0; 8]);
    let mut s1 = libsecp256k1::curve::Scalar([0; 8]);
    drop(r1.set_b32(&r));
    drop(s1.set_b32(&s));
    let sig = libsecp256k1::Signature { r: r1, s: s1 };
    let msg = libsecp256k1::Message::parse_slice(tx_hash.as_bytes()).ok()?;
    let recover_id = libsecp256k1::RecoveryId::parse(v as u8).ok()?;
    let pubkey = libsecp256k1::recover(&msg, &sig, &recover_id)
        .ok()?
        .serialize();
    assert!(pubkey[0] == 4);
    Some(Addr::from_slice(
        &sha3::Keccak256::digest(&pubkey[1..]).as_slice()[12..],
    ))
}
