use bitvec::vec::BitVec;
use hex::{FromHex, ToHex};
use once_cell::sync::OnceCell;
pub use primitive_types::U256;
use primitive_types::{H160, H256};
use serde::{
    de::{self, Deserialize, Deserializer, Visitor},
    Serialize, Serializer,
};
use sha3::Digest;

use std::fmt;
use std::ops::Deref;
use std::str::FromStr;

#[cfg(feature = "actor")] pub use actor::*;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Wei(U256);

#[derive(Clone, PartialEq, Eq, Hash, Default, Debug)]
pub struct Addr(H160);

#[derive(Clone, PartialEq, Eq, Hash, Default, Debug)]
pub struct Hash(H256);

#[derive(Clone, Default)]
pub struct Bytes(Vec<u8>);

pub type Bytes32 = FixedBytes<32>;

#[derive(Debug)]
pub struct FixedBytes<const N: usize>([u8; N]);

pub type Gas = u64;

// Wei

impl Wei {
    #[inline(always)]
    pub fn checked_add(&self, other: &Wei) -> Option<Wei> {
        Some(Wei(self.0.checked_add(other.0)?))
    }

    #[inline(always)]
    pub fn checked_sub(&self, other: &Wei) -> Option<Wei> {
        Some(Wei(self.0.checked_sub(other.0)?))
    }

    #[inline(always)]
    pub fn checked_mul(&self, other: &Wei) -> Option<Wei> {
        Some(Wei(self.0.checked_mul(other.0)?))
    }

    #[inline]
    pub fn zero() -> &'static Self {
        static V: OnceCell<Wei> = OnceCell::new();
        V.get_or_init(|| U256::zero().into())
    }

    #[inline(always)]
    pub fn is_zero(&self) -> bool {
        self.0.is_zero()
    }

    #[inline]
    pub fn to_big_endian(&self, buff: &mut [u8]) {
        self.0.to_big_endian(buff)
    }
}

impl From<U256> for Wei {
    fn from(u: U256) -> Self {
        Self(u)
    }
}

impl From<u64> for Wei {
    fn from(u: u64) -> Self {
        Self(u.into())
    }
}

impl FromStr for Wei {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        Ok(Self(U256::from_str(s).map_err(|_| ())?))
    }
}

impl fmt::Display for Wei {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl fmt::LowerHex for Wei {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::LowerHex::fmt(&self.0, f)
    }
}

impl Serialize for Wei {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("0x{:x}", &self))
    }
}

impl<'de> Deserialize<'de> for Wei {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(deserializer.deserialize_identifier(U256Visitor)?.into())
    }
}

// Addr

impl Addr {
    #[inline]
    pub fn zero() -> &'static Self {
        static V: OnceCell<Addr> = OnceCell::new();
        V.get_or_init(|| U256::zero().into())
    }

    #[inline(always)]
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    #[inline(always)]
    pub fn from_slice(s: &[u8]) -> Self {
        Self(H160::from_slice(s))
    }
}

impl From<U256> for Addr {
    fn from(u: U256) -> Self {
        let mut bytes: [u8; 32] = Default::default();
        u.to_big_endian(&mut bytes);
        Self::from_slice(&bytes[12..])
    }
}

impl From<[u8; 20]> for Addr {
    fn from(bytes: [u8; 20]) -> Self {
        Self(H160(bytes))
    }
}

impl FromStr for Addr {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        Ok(Self(H160::from_str(s).map_err(|_| ())?))
    }
}

impl fmt::Display for Addr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Serialize for Addr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        BytesRef::serialize(&BytesRef(self.as_bytes()), serializer)
    }
}

impl<'de> Deserialize<'de> for Addr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error;
        let bytes = deserializer.deserialize_identifier(BytesVisitor)?.0;
        if bytes.len() != 20 {
            return Err(D::Error::invalid_length(
                bytes.len(),
                &"length of 20 bytes",
            ))
        }
        Ok(Addr::from_slice(&bytes))
    }
}

// U256

impl From<Wei> for U256 {
    fn from(w: Wei) -> Self {
        w.0
    }
}

impl AsRef<U256> for Wei {
    fn as_ref(&self) -> &U256 {
        &self.0
    }
}

impl From<Hash> for U256 {
    fn from(hash: Hash) -> Self {
        U256::from_big_endian(hash.as_bytes())
    }
}

impl From<Addr> for U256 {
    fn from(addr: Addr) -> Self {
        U256::from_big_endian(addr.as_bytes())
    }
}

// Hash

impl Hash {
    #[inline(always)]
    pub fn hash(slice: &[u8]) -> Self {
        Self::from_slice(sha3::Keccak256::digest(slice).as_slice())
    }

    #[inline(always)]
    pub fn empty_bytes_hash() -> &'static Self {
        static V: OnceCell<Hash> = OnceCell::new();
        V.get_or_init(|| {
            let hasher = sha3::Keccak256::new();
            Self::from_slice(hasher.finalize().as_slice())
        })
    }

    #[inline]
    pub fn zero() -> &'static Self {
        static V: OnceCell<Hash> = OnceCell::new();
        V.get_or_init(|| Self(H256::zero()))
    }

    #[inline(always)]
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    #[inline(always)]
    pub fn from_slice(s: &[u8]) -> Self {
        Self(H256::from_slice(s))
    }

    #[inline]
    pub fn to_fixed_bytes(self) -> [u8; 32] {
        self.0.to_fixed_bytes()
    }
}

impl From<[u8; 32]> for Hash {
    fn from(u: [u8; 32]) -> Self {
        Self(u.into())
    }
}

impl From<U256> for Hash {
    fn from(u: U256) -> Self {
        let mut bytes: [u8; 32] = Default::default();
        u.to_big_endian(&mut bytes);
        Self::from_slice(&bytes)
    }
}

impl FromStr for Hash {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        Ok(Self(H256::from_str(s).map_err(|_| ())?))
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Serialize for Hash {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        BytesRef::serialize(&BytesRef(self.as_bytes()), serializer)
    }
}

impl<'de> Deserialize<'de> for Hash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error;
        let bytes = deserializer.deserialize_identifier(BytesVisitor)?.0;
        if bytes.len() != 32 {
            return Err(D::Error::invalid_length(
                bytes.len(),
                &"length of 32 bytes",
            ))
        }
        Ok(Hash::from_slice(&bytes))
    }
}

impl From<&U256> for Bytes32 {
    fn from(u: &U256) -> Self {
        let mut bytes: [u8; 32] = Default::default();
        u.to_big_endian(&mut bytes);
        Self(bytes)
    }
}

impl Bytes {
    pub fn empty() -> Self {
        Self(Vec::new())
    }

    pub fn into_inner(self) -> Vec<u8> {
        self.0
    }
}

impl From<Vec<u8>> for Bytes {
    fn from(v: Vec<u8>) -> Self {
        Self(v)
    }
}

impl From<&[u8]> for Bytes {
    fn from(s: &[u8]) -> Self {
        Self(s.into())
    }
}

impl Deref for Bytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Display for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        <BytesRef as fmt::LowerHex>::fmt(&BytesRef(self), f)
    }
}

impl fmt::Debug for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        <BytesRef as fmt::LowerHex>::fmt(&BytesRef(self), f)
    }
}

impl From<Bytes32> for Bytes {
    fn from(u: crate::common::Bytes32) -> Self {
        (&u[..]).into()
    }
}

impl rlp::Encodable for Bytes {
    fn rlp_append(&self, s: &mut rlp::RlpStream) {
        s.encoder().encode_value(self)
    }
}

impl rlp::Decodable for Bytes {
    fn decode(rlp: &rlp::Rlp) -> Result<Self, rlp::DecoderError> {
        rlp.decoder().decode_value(|bytes| Ok(Self(bytes.to_vec())))
    }
}

impl Serialize for Bytes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        BytesRef::serialize(&BytesRef(self), serializer)
    }
}

impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_identifier(BytesVisitor)
    }
}

impl<const N: usize> Default for FixedBytes<N> {
    fn default() -> Self {
        FixedBytes([0; N])
    }
}

impl<const N: usize> Deref for FixedBytes<N> {
    type Target = [u8; N];
    fn deref(&self) -> &[u8; N] {
        &self.0
    }
}

impl<const N: usize> Serialize for FixedBytes<N> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        BytesRef::serialize(&BytesRef(&self.0), serializer)
    }
}

pub struct BytesRef<'a>(&'a [u8]);

impl<'a> From<&'a [u8]> for BytesRef<'a> {
    fn from(s: &'a [u8]) -> Self {
        Self(s)
    }
}

impl<'a> fmt::LowerHex for BytesRef<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.encode_hex::<String>())
    }
}

impl<'a> Serialize for BytesRef<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("0x{:x}", &self))
    }
}

#[derive(serde::Serialize, Debug)]
pub struct NullableAddr(pub Option<Addr>);

impl rlp::Decodable for NullableAddr {
    fn decode(rlp: &rlp::Rlp) -> Result<Self, rlp::DecoderError> {
        rlp.decoder().decode_value(|bytes| {
            if bytes.is_empty() {
                Ok(NullableAddr(None))
            } else {
                match bytes.len().cmp(&20) {
                    core::cmp::Ordering::Less => {
                        Err(rlp::DecoderError::RlpIsTooShort)
                    }
                    core::cmp::Ordering::Greater => {
                        Err(rlp::DecoderError::RlpIsTooBig)
                    }
                    core::cmp::Ordering::Equal => {
                        let mut t = [0u8; 20];
                        t.copy_from_slice(bytes);
                        Ok(Self(Some(t.into())))
                    }
                }
            }
        })
    }
}

impl rlp::Encodable for NullableAddr {
    fn rlp_append(&self, s: &mut rlp::RlpStream) {
        s.encoder().encode_value(match &self.0 {
            Some(addr) => addr.as_bytes(),
            None => &[],
        });
    }
}

pub struct BytesVisitor;
impl<'de> Visitor<'de> for BytesVisitor {
    type Value = Bytes;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter
            .write_str("starts with `0x` and has even number of hex digits")
    }

    fn visit_str<E>(self, value: &str) -> Result<Bytes, E>
    where
        E: de::Error,
    {
        if value.len() < 2 {
            return Err(de::Error::invalid_length(value.len(), &self))
        }
        let bytes = value.as_bytes();
        if bytes[0] == b'0' && (bytes[1] == b'x' || bytes[1] == b'X') {
            match Vec::from_hex(&value[2..]) {
                Ok(v) => Ok(v.into()),
                Err(_) => Err(de::Error::invalid_value(
                    de::Unexpected::Str(value),
                    &self,
                )),
            }
        } else {
            Err(de::Error::invalid_value(de::Unexpected::Str(value), &self))
        }
    }
}

pub struct U256Visitor;

impl<'de> Visitor<'de> for U256Visitor {
    type Value = U256;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter
            .write_str("a string that starts with `0x` and has 64 hex digits")
    }

    fn visit_str<E>(self, value: &str) -> Result<U256, E>
    where
        E: de::Error,
    {
        U256::from_str(value).map_err(|_| {
            de::Error::invalid_value(de::Unexpected::Str(value), &self)
        })
    }
}

pub struct U64Visitor;

impl<'de> Visitor<'de> for U64Visitor {
    type Value = u64;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter
            .write_str("a string that starts with `0x` and contains hex digits")
    }

    fn visit_str<E>(self, value: &str) -> Result<u64, E>
    where
        E: de::Error,
    {
        let bytes = value.as_bytes();
        if bytes[0] == b'0' && (bytes[1] == b'x' || bytes[1] == b'X') {
            Ok(u64::from_str_radix(&value[2..], 16).map_err(|_| {
                de::Error::invalid_value(de::Unexpected::Str(value), &self)
            })?)
        } else {
            Err(de::Error::invalid_value(de::Unexpected::Str(value), &self))
        }
    }
}

// NOTE: adapted from https://docs.rs/impl-rlp/latest/src/impl_rlp/lib.rs.html
macro_rules! impl_wrapped_uint_rlp {
    ($name: ident, $wrapper_name: ident, $size: expr) => {
        impl rlp::Encodable for $wrapper_name {
            fn rlp_append(&self, s: &mut rlp::RlpStream) {
                let leading_empty_bytes = $size * 8 - (self.0.bits() + 7) / 8;
                let mut buffer = [0u8; $size * 8];
                self.0.to_big_endian(&mut buffer);
                s.encoder().encode_value(&buffer[leading_empty_bytes..]);
            }
        }

        impl rlp::Decodable for $wrapper_name {
            fn decode(rlp: &rlp::Rlp) -> Result<Self, rlp::DecoderError> {
                rlp.decoder().decode_value(|bytes| {
                    if !bytes.is_empty() && bytes[0] == 0 {
                        Err(rlp::DecoderError::RlpInvalidIndirection)
                    } else if bytes.len() <= $size * 8 {
                        Ok($wrapper_name($name::from(bytes)))
                    } else {
                        Err(rlp::DecoderError::RlpIsTooBig)
                    }
                })
            }
        }
    };
}

// NOTE: adapted from https://docs.rs/impl-rlp/latest/src/impl_rlp/lib.rs.html
macro_rules! impl_wrapped_fixed_hash_rlp {
    ($name: ident, $wrapper_name: ident, $size: expr) => {
        impl rlp::Encodable for $wrapper_name {
            fn rlp_append(&self, s: &mut rlp::RlpStream) {
                s.encoder().encode_value(self.0.as_ref());
            }
        }

        impl rlp::Decodable for $wrapper_name {
            fn decode(rlp: &rlp::Rlp) -> Result<Self, rlp::DecoderError> {
                rlp.decoder().decode_value(|bytes| {
                    match bytes.len().cmp(&$size) {
                        core::cmp::Ordering::Less => {
                            Err(rlp::DecoderError::RlpIsTooShort)
                        }
                        core::cmp::Ordering::Greater => {
                            Err(rlp::DecoderError::RlpIsTooBig)
                        }
                        core::cmp::Ordering::Equal => {
                            let mut t = [0u8; $size];
                            t.copy_from_slice(bytes);
                            Ok($wrapper_name($name(t)))
                        }
                    }
                })
            }
        }
    };
}

impl_wrapped_uint_rlp!(U256, Wei, 4);
impl_wrapped_fixed_hash_rlp!(H160, Addr, 20);
impl_wrapped_fixed_hash_rlp!(H256, Hash, 32);

#[cfg(feature = "actor")]
mod actor {
    use super::*;
    use actix::prelude::*;

    #[derive(MessageResponse)]
    pub struct WrappedWei(pub Wei);

    #[derive(MessageResponse, Clone, Default, Debug)]
    pub struct WrappedGas(pub Gas);

    #[derive(MessageResponse, Default, Debug)]
    pub struct WrappedU256(pub U256);

    #[derive(MessageResponse, Default, Debug)]
    pub struct WrappedU64(pub u64);

    #[derive(MessageResponse)]
    pub struct WrappedHash(pub Hash);

    impl Serialize for WrappedU256 {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serializer.serialize_str(&format!("0x{:x}", &self.0))
        }
    }

    impl Serialize for WrappedU64 {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serializer.serialize_str(&format!("0x{:x}", &self.0))
        }
    }

    impl Serialize for WrappedGas {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serializer.serialize_str(&format!("0x{:x}", &self.0))
        }
    }

    impl<'de> Deserialize<'de> for WrappedU256 {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer.deserialize_identifier(U256Visitor).map(Self)
        }
    }

    impl<'de> Deserialize<'de> for WrappedGas {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer.deserialize_identifier(U64Visitor).map(Self)
        }
    }
}

#[derive(Debug)]
pub struct U256RLP(pub U256);
impl_wrapped_uint_rlp!(U256, U256RLP, 4);

pub fn create_addr(addr: &Addr, nonce: u64) -> Addr {
    let mut rlp_encoded = rlp::RlpStream::new_list(2);
    rlp_encoded.append(&addr.as_bytes()).append(&nonce);
    let rlp_encoded = rlp_encoded.out();
    Addr::from_slice(&sha3::Keccak256::digest(rlp_encoded).as_slice()[12..])
}

#[test]
fn test_create_addr() {
    let addr0 =
        Addr::from_str("0x6ac7ea33f8831ea9dcc53393aaa88b25a785dbf0").unwrap();
    assert_eq!(
        create_addr(&addr0, 0),
        Addr::from_str("0xcd234a471b72ba2f1ccf0a70fcaba648a5eecd8d").unwrap()
    );
    assert_eq!(
        create_addr(&addr0, 1),
        Addr::from_str("0x343c43a37d37dff08ae8c4a11544c718abb4fcf8").unwrap()
    );
}

pub fn create_addr2(addr: &Addr, salt: &[u8], init_hash: &[u8]) -> Addr {
    let mut hasher = sha3::Keccak256::new();
    hasher.update([0xff]);
    hasher.update(addr.as_bytes());
    hasher.update(salt);
    hasher.update(sha3::Keccak256::digest(init_hash));
    Addr::from_slice(&hasher.finalize().as_slice()[12..])
}

#[test]
fn test_create_addr2() {
    let addr0 =
        Addr::from_str("0x0000000000000000000000000000000000000000").unwrap();
    assert_eq!(
        create_addr2(&addr0, &hex::decode("0000000000000000000000000000000000000000000000000000000000000000").unwrap(), &hex::decode("00").unwrap()),
        Addr::from_str("0x4D1A2e2bB4F88F0250f26Ffff098B0b30B26BF38").unwrap()
    );
    let addr1 =
        Addr::from_str("0xdeadbeef00000000000000000000000000000000").unwrap();
    assert_eq!(
        create_addr2(&addr1, &hex::decode("000000000000000000000000feed000000000000000000000000000000000000").unwrap(), &hex::decode("00").unwrap()),
        Addr::from_str("0xD04116cDd17beBE565EB2422F2497E06cC1C9833").unwrap()
    );
}

pub fn gen_code_bitmap(code: &[u8]) -> BitVec {
    let mut bitmap = BitVec::repeat(false, code.len());
    let mut nskip = 0;
    for (i, b) in code.iter().enumerate() {
        if nskip > 0 {
            nskip -= 1;
            continue
        }
        bitmap.set(i, true);
        match b {
            0x60..=0x7f => nskip = b - 0x60 + 1,
            _ => (),
        }
    }
    bitmap
}

#[inline(always)]
pub fn checked_as_u64(x: &U256) -> Option<u64> {
    if x > &u64::MAX.into() {
        None
    } else {
        Some(x.as_u64())
    }
}

#[inline]
pub fn u256_1() -> &'static U256 {
    static V: OnceCell<U256> = OnceCell::new();
    V.get_or_init(U256::one)
}
