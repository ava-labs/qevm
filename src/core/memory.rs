use once_cell::sync::OnceCell;

use super::params::*;
use super::ExecError;
use crate::common::{Gas, U256};

pub struct Memory {
    space: Vec<u8>,
    last_gas: Gas,
}

impl Memory {
    #[inline(always)]
    pub fn get_max_mem() -> &'static U256 {
        static V: OnceCell<U256> = OnceCell::new();
        V.get_or_init(|| MAX_MEM_SIZE.into())
    }

    #[inline(always)]
    pub fn to_word_size(size: u64) -> u64 {
        if size > u64::MAX - 31 {
            (u64::MAX >> 5) + 1
        } else {
            (size + 31) >> 5
        }
    }

    pub fn new() -> Self {
        Self {
            space: Vec::new(),
            last_gas: 0,
        }
    }

    #[inline(always)]
    pub fn resize(&mut self, size: U256) -> Result<Gas, ExecError> {
        if &size > Self::get_max_mem() {
            return Err(ExecError::OutOfMemory)
        }
        let size64 = size.as_u64();
        Ok(if size64 > self.space.len() as u64 {
            let new_mem_size_words = Self::to_word_size(size64);
            let new_mem_size = (new_mem_size_words << 5) as usize;
            self.space.resize(new_mem_size, 0);
            let sqr = new_mem_size_words * new_mem_size_words;
            let new_gas =
                new_mem_size_words * GAS_MEM_RESIZE_WORD + sqr / QUAD_COEF_DIV;
            let fee = new_gas - self.last_gas;
            self.last_gas = new_gas;
            fee
        } else {
            0
        })
    }

    #[inline(always)]
    pub fn get_slice_mut(
        &mut self, off: U256, len: U256,
    ) -> Result<(&mut [u8], Gas), ExecError> {
        let end = off + len;
        self.resize(end)
            .map(|gas| (&mut self.space[off.as_usize()..end.as_usize()], gas))
    }

    #[inline(always)]
    pub fn get_slice(
        &mut self, off: U256, len: U256,
    ) -> Result<(&[u8], Gas), ExecError> {
        self.get_slice_mut(off, len).map(|e| (&*e.0, e.1))
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.space.len()
    }

    #[inline(always)]
    pub fn set(
        &mut self, off: U256, len: U256, data: &[u8],
    ) -> Result<Gas, ExecError> {
        let (slice, gas) = self.get_slice_mut(off, len)?;
        // the geth behavior (uses `copy()` in go code)
        let min_len = std::cmp::min(slice.len(), data.len());
        slice[..min_len].copy_from_slice(&data[..min_len]);
        Ok(gas)
    }
}
