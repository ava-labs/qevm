use std::cell::RefCell;
use std::sync::Arc;

use log::debug;
use sha3::Digest;

use super::alu;
use super::exec::CallType;
use super::memory::Memory;
use super::params::*;
use super::stack::Stack;
use super::{gas_checked_mul, get_data, Code, ExecError};
use crate::common::{checked_as_u64, Addr, Bytes, Gas, Wei, U256};

pub(super) struct CallFrame<S> {
    pub pc: u64,
    pub memory: Memory,
    pub stack: Stack,
    pub code: Arc<dyn Code>,
    /// address of the executing contract
    pub callee: Addr,
    /// address of the caller
    pub caller: Addr,
    pub call_type: CallType<S>,
    input: Box<[u8]>,
    pub value: Wei,
    pub last_returned: Bytes,
    pub unused_gas: Gas,
    pub read_only: bool,
    pub fork: Fork,
}

macro_rules! make_unary_op {
    ($name: ident, $gas: expr) => {
        #[inline(always)]
        pub fn $name(&mut self) -> Result<(), ExecError> {
            let a = self.stack.consume1()?;
            self.stack.push(alu::$name(a))?;
            self.use_gas($gas)
        }
    };
}

macro_rules! make_binary_op {
    ($name: ident, $gas: expr) => {
        #[inline(always)]
        pub fn $name(&mut self) -> Result<(), ExecError> {
            let (a, b) = self.stack.consume2()?;
            self.stack.push(alu::$name(a, b))?;
            self.use_gas($gas)
        }
    };
    ($name: ident, $gas: expr, $fork: ident) => {
        #[inline(always)]
        pub fn $name(&mut self) -> Result<(), ExecError> {
            if self.fork < Fork::$fork {
                return Err(ExecError::InvalidOpcode)
            }
            let (a, b) = self.stack.consume2()?;
            self.stack.push(alu::$name(a, b))?;
            self.use_gas($gas)
        }
    };
}

macro_rules! make_ternary_op {
    ($name: ident, $gas: expr) => {
        #[inline(always)]
        pub fn $name(&mut self) -> Result<(), ExecError> {
            let (a, b, c) = self.stack.consume3()?;
            self.stack.push(alu::$name(a, b, c))?;
            self.use_gas($gas)
        }
    };
}

impl<S> CallFrame<S> {
    #[inline]
    pub fn new(
        code: Arc<dyn Code>, input: Box<[u8]>, value: Wei, callee: Addr,
        caller: Addr, call_type: CallType<S>, gas: Gas, read_only: bool,
        fork: Fork,
    ) -> Self {
        Self {
            pc: 0x0,
            memory: Memory::new(),
            stack: Stack::new(),
            code,
            callee,
            caller,
            call_type,
            input,
            value,
            last_returned: Bytes::empty(),
            unused_gas: gas,
            read_only,
            fork,
        }
    }

    make_binary_op!(add, GAS_FASTEST);
    make_binary_op!(mul, GAS_FAST);
    make_binary_op!(sub, GAS_FASTEST);
    make_binary_op!(div, GAS_FAST);
    make_binary_op!(sdiv, GAS_FAST);
    make_binary_op!(rem, GAS_FAST);
    make_binary_op!(smod, GAS_FAST);
    make_ternary_op!(add_mod, GAS_MID);
    make_ternary_op!(mul_mod, GAS_MID);
    #[inline(always)]
    pub fn exp(&mut self) -> Result<(), ExecError> {
        let (a, b) = self.stack.consume2()?;
        self.stack.push(alu::exp(a, b))?;
        self.use_gas(gas_checked_mul(
            if self.fork >= Fork::SpuriousDragon {
                GAS_EXP_BYTE_SPURIOUS_DRAGON
            } else {
                GAS_EXP_BYTE_FRONTIER
            },
            (b.bits() as u64 + 7) >> 3,
        )?)
    }
    make_binary_op!(sign_extend, GAS_FAST);
    make_binary_op!(lt, GAS_FASTEST);
    make_binary_op!(gt, GAS_FASTEST);
    make_binary_op!(slt, GAS_FASTEST);
    make_binary_op!(sgt, GAS_FASTEST);
    make_binary_op!(eq, GAS_FASTEST);
    make_unary_op!(is_zero, GAS_FASTEST);
    make_binary_op!(and, GAS_FASTEST);
    make_binary_op!(or, GAS_FASTEST);
    make_binary_op!(xor, GAS_FASTEST);
    make_unary_op!(not, GAS_FASTEST);
    make_binary_op!(byte, GAS_FASTEST);
    make_binary_op!(shl, GAS_FASTEST, Constantinople);
    make_binary_op!(shr, GAS_FASTEST, Constantinople);
    make_binary_op!(sar, GAS_FASTEST, Constantinople);

    #[inline(always)]
    pub fn sha3(&mut self) -> Result<(), ExecError> {
        self.use_gas(GAS_SHA3)?;
        let (off, len) = self.stack.consume2()?;
        let (data, gas) = self.memory.get_slice(off, len)?;
        // EVM is big-endian
        self.stack
            .push(U256::from_big_endian(&sha3::Keccak256::digest(data)))?;
        self.use_gas(gas_checked_mul(
            Memory::to_word_size(len.low_u64()),
            GAS_SHA3_WORD,
        )?)?;
        self.use_gas(gas)
    }

    #[inline(always)]
    pub fn addr(&mut self) -> Result<(), ExecError> {
        self.use_gas(GAS_QUICK)?;
        self.stack.push(self.callee.clone().into())
    }

    #[inline(always)]
    pub fn caller(&mut self) -> Result<(), ExecError> {
        self.use_gas(GAS_QUICK)?;
        self.stack.push(self.caller.clone().into())
    }

    #[inline(always)]
    pub fn call_value(&mut self) -> Result<(), ExecError> {
        self.use_gas(GAS_QUICK)?;
        self.stack.push(self.value.clone().into())
    }

    #[inline(always)]
    pub fn call_data_load(&mut self) -> Result<(), ExecError> {
        self.use_gas(GAS_FASTEST)?;
        let data = if let Some(off) = checked_as_u64(&self.stack.consume1()?) {
            U256::from_big_endian(&get_data(&self.input, off, 32))
        } else {
            U256::zero()
        };
        self.stack.push(data)
    }

    #[inline(always)]
    pub fn call_data_size(&mut self) -> Result<(), ExecError> {
        self.use_gas(GAS_QUICK)?;
        self.stack.push(self.input.len().into())
    }

    #[inline(always)]
    pub fn code_size(&mut self) -> Result<(), ExecError> {
        self.use_gas(GAS_QUICK)?;
        self.stack.push(self.code.as_bytes().len().into())
    }

    #[inline(always)]
    pub fn code_copy(&mut self) -> Result<(), ExecError> {
        self.use_gas(GAS_FASTEST)?;
        let (mem_off, code_off, len) = self.stack.consume3()?;
        let (mem, gas) = self.memory.get_slice_mut(mem_off, len)?;
        let len = len.low_u64();
        let code_off = checked_as_u64(&code_off).unwrap_or(u64::MAX);
        mem.copy_from_slice(&get_data(self.code.as_bytes(), code_off, len));
        self.use_gas(gas_checked_mul(
            Memory::to_word_size(len),
            GAS_COPY_WORD,
        )?)?;
        self.use_gas(gas)
    }

    #[inline(always)]
    pub fn call_data_copy(&mut self) -> Result<(), ExecError> {
        self.use_gas(GAS_FASTEST)?;
        let (mem_off, data_off, len) = self.stack.consume3()?;
        let (mem, gas) = self.memory.get_slice_mut(mem_off, len)?;
        let len = len.low_u64();
        let data_off = checked_as_u64(&data_off).unwrap_or(u64::MAX);
        mem.copy_from_slice(&get_data(&self.input, data_off, len));
        self.use_gas(gas)?;
        self.use_gas(gas_checked_mul(Memory::to_word_size(len), GAS_COPY_WORD)?)
    }

    #[inline(always)]
    pub fn return_data_size(&mut self) -> Result<(), ExecError> {
        if self.fork < Fork::Byzantium {
            return Err(ExecError::InvalidOpcode)
        }
        self.use_gas(GAS_QUICK)?;
        self.stack.push(self.last_returned.len().into())
    }

    #[inline(always)]
    pub fn return_data_copy(&mut self) -> Result<(), ExecError> {
        if self.fork < Fork::Byzantium {
            return Err(ExecError::InvalidOpcode)
        }
        self.use_gas(GAS_FASTEST)?;
        let (mem_off, data_off, len) = self.stack.consume3()?;
        // NOTE: in geth, retutrndatacopy is handled differently
        // than calldatacopy (no padding, differnet boundary checking).
        // See also: https://github.com/ethereum/yellowpaper/issues/758
        let end = checked_as_u64(&data_off.overflowing_add(len).0)
            .ok_or(ExecError::ReturnDataOutOfBounds)?;
        if end as usize > self.last_returned.len() {
            return Err(ExecError::ReturnDataOutOfBounds)
        }
        let data_off = checked_as_u64(&data_off)
            .ok_or(ExecError::ReturnDataOutOfBounds)?;
        let (mem, gas) = self.memory.get_slice_mut(mem_off, len)?;
        mem.copy_from_slice(
            &self.last_returned[data_off as usize..end as usize],
        );
        self.use_gas(gas)?;
        self.use_gas(gas_checked_mul(
            Memory::to_word_size(len.low_u64()),
            GAS_COPY_WORD,
        )?)
    }

    #[inline(always)]
    pub fn pop(&mut self) -> Result<(), ExecError> {
        self.use_gas(GAS_QUICK)?;
        self.stack.consume1().map(|_| ())
    }

    #[inline(always)]
    pub fn mload(&mut self) -> Result<(), ExecError> {
        self.use_gas(GAS_FASTEST)?;
        let off = self.stack.consume1()?;
        let (mem, gas) = self.memory.get_slice(off, 32.into())?;
        self.stack.push(U256::from_big_endian(mem))?;
        self.use_gas(gas)
    }

    #[inline(always)]
    pub fn mstore(&mut self) -> Result<(), ExecError> {
        self.use_gas(GAS_FASTEST)?;
        let (off, val) = self.stack.consume2()?;
        let (mem, gas) = self.memory.get_slice_mut(off, 32.into())?;
        val.to_big_endian(mem);
        self.use_gas(gas)
    }

    #[inline(always)]
    pub fn mstore8(&mut self) -> Result<(), ExecError> {
        self.use_gas(GAS_FASTEST)?;
        let (off, val) = self.stack.consume2()?;
        let (mem, gas) = self.memory.get_slice_mut(off, U256::one())?;
        mem[0] = val.low_u64() as u8;
        self.use_gas(gas)
    }

    #[inline(always)]
    pub fn pc(&mut self) -> Result<(), ExecError> {
        self.use_gas(GAS_QUICK)?;
        self.stack.push(self.pc.into())
    }

    #[inline(always)]
    pub fn msize(&mut self) -> Result<(), ExecError> {
        self.use_gas(GAS_QUICK)?;
        self.stack.push(self.memory.len().into())
    }

    #[inline(always)]
    pub fn gas(&mut self) -> Result<(), ExecError> {
        self.use_gas(GAS_QUICK)?;
        self.stack.push(self.unused_gas.into())
    }

    #[inline(always)]
    pub fn push(&mut self, data: &[u8]) -> Result<(), ExecError> {
        self.use_gas(GAS_FASTEST)?;
        // Because `push` is never used recuresively, we use the thread local space to avoid the
        // repeated heap allocation (with some negligible RefCell deref overhead).
        thread_local! {
            static BYTES: RefCell<[u8; 32]> = RefCell::new([0; 32]);
        }
        // right-aligned, big endian
        BYTES.with(|b| {
            let mut b = b.borrow_mut();
            b.iter_mut().for_each(|m| *m = 0);
            b[32 - data.len()..].copy_from_slice(data);
            self.stack.push(U256::from_big_endian(&mut *b))
        })
    }

    #[inline(always)]
    pub fn dup(&mut self, pos: usize) -> Result<(), ExecError> {
        self.use_gas(GAS_FASTEST)?;
        self.stack.dup(pos)
    }

    #[inline(always)]
    pub fn swap(&mut self, pos: usize) -> Result<(), ExecError> {
        self.use_gas(GAS_FASTEST)?;
        self.stack.swap(pos)
    }

    #[inline(always)]
    pub fn use_gas(&mut self, gas: Gas) -> Result<(), ExecError> {
        if self.unused_gas < gas {
            debug!("Out of Gas: {} < {}", self.unused_gas, gas);
            return Err(ExecError::OutOfGas)
        }
        self.unused_gas -= gas;
        Ok(())
    }
}
