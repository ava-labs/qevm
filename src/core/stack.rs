use super::params::*;
use super::ExecError;
use crate::common::U256;

pub struct Stack {
    stack: [U256; MAX_STACK_DEPTH],
    top: usize,
}

impl Stack {
    pub fn new() -> Self {
        Self {
            stack: [U256::zero(); MAX_STACK_DEPTH],
            top: 0,
        }
    }

    #[inline(always)]
    pub fn push(&mut self, val: U256) -> Result<(), ExecError> {
        if self.top == MAX_STACK_DEPTH {
            return Err(ExecError::StackOverflow)
        }
        self.stack[self.top] = val;
        self.top += 1;
        Ok(())
    }

    #[inline(always)]
    pub fn dup(&mut self, pos: usize) -> Result<(), ExecError> {
        if pos > self.top {
            return Err(ExecError::StackUnderflow)
        }
        self.push(self.stack[self.top - pos])
    }

    #[inline(always)]
    pub fn swap(&mut self, pos: usize) -> Result<(), ExecError> {
        if pos + 1 > self.top {
            return Err(ExecError::StackUnderflow)
        }
        let top = self.top - 1;
        self.stack.swap(top, top - pos);
        Ok(())
    }

    #[inline(always)]
    pub fn consume1(&mut self) -> Result<U256, ExecError> {
        if self.top > 0 {
            self.top -= 1;
            return Ok(self.stack[self.top])
        }
        Err(ExecError::StackUnderflow)
    }
    #[inline(always)]
    pub fn consume2(&mut self) -> Result<(U256, U256), ExecError> {
        if self.top > 1 {
            self.top -= 2;
            return Ok((self.stack[self.top + 1], self.stack[self.top]))
        }
        Err(ExecError::StackUnderflow)
    }

    #[inline(always)]
    pub fn consume3(&mut self) -> Result<(U256, U256, U256), ExecError> {
        if self.top > 2 {
            self.top -= 3;
            return Ok((
                self.stack[self.top + 2],
                self.stack[self.top + 1],
                self.stack[self.top],
            ))
        }
        Err(ExecError::StackUnderflow)
    }

    #[inline(always)]
    pub fn consume4(&mut self) -> Result<(U256, U256, U256, U256), ExecError> {
        if self.top > 3 {
            self.top -= 4;
            return Ok((
                self.stack[self.top + 3],
                self.stack[self.top + 2],
                self.stack[self.top + 1],
                self.stack[self.top],
            ))
        }
        Err(ExecError::StackUnderflow)
    }
}
