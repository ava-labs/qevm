use crate::common::Gas;

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub enum Fork {
    Frontier,
    Homestead,
    TangerineWhistle, // EIP-150
    SpuriousDragon,   // EIP-158/161
    Byzantium,
    Constantinople,
    Istanbul, // EIP-1334, EIP-1884, EIP-2200
    Berlin,   // EIP-2929
    London,   // EIP-3529, EIP-3198
}

pub const MAX_CALL_DEPTH: usize = 1024;
pub const MAX_STACK_DEPTH: usize = 1024;
pub const MAX_CODE_SIZE: usize = 24576;
pub const MAX_MEM_SIZE: usize = 0x1fffffffe0;

// gas consumption parameters
pub const GAS_QUICK: Gas = 2;
pub const GAS_FASTEST: Gas = 3;
pub const GAS_FAST: Gas = 5;
pub const GAS_SHA3: Gas = 30;
pub const GAS_COPY_WORD: Gas = 3;
pub const GAS_SHA3_WORD: Gas = 6;
pub const GAS_MID: Gas = 8;
pub const GAS_SLOW: Gas = 10;
pub const GAS_EXT: Gas = 20;
pub const GAS_EXT_BASE_FRONTIER: Gas = 20;
pub const GAS_EXT_BASE_TANGERINE: Gas = 700; // EIP-150
pub const GAS_LOG: Gas = 375;
pub const GAS_LOG_TOPIC: Gas = 375;
pub const GAS_LOG_DATA: Gas = 8;
pub const GAS_CREATE: Gas = 32000;
pub const GAS_CREATE2: Gas = 32000;
pub const GAS_CALL_FRONTIER: Gas = 40;
pub const GAS_CALL_TANGERINE: Gas = 700; // EIP-150
pub const GAS_CALL_STIPEND: Gas = 2300;
pub const GAS_CALL_NEW_ACCOUNT: Gas = 25000;
pub const GAS_CALL_VALUE_TRANS: Gas = 9000;
pub const GAS_CREATE_DATA: u64 = 200;
pub const GAS_MEM_RESIZE_WORD: Gas = 3;
pub const GAS_JUMPDEST: Gas = 1;
pub const GAS_EXP_BYTE_FRONTIER: Gas = 10;
pub const GAS_EXP_BYTE_SPURIOUS_DRAGON: Gas = 50; // EIP-158
pub const QUAD_COEF_DIV: Gas = 512;
pub const GAS_SELF_DESTRUCT: Gas = 5000;
pub const GAS_CREATE_BY_SELF_DESTRUCT: Gas = 25000;
pub const GAS_BALANCE_FRONTIER: Gas = 20;
pub const GAS_BALANCE_TANGERINE: Gas = 400; // EIP-150
pub const GAS_BALANCE_ISTANBUL: Gas = 700; // EIP-1884
pub const GAS_EXT_CODE_SIZE_FRONTIER: Gas = 20;
pub const GAS_EXT_CODE_SIZE_TANGERINE: Gas = 700; // EIP-150
pub const GAS_SLOAD_FRONTIER: Gas = 50;
pub const GAS_SLOAD_TANGERINE: Gas = 200; // EIP-150
pub const GAS_SLOAD_ISTANBUL: Gas = 800; // EIP-1884, EIP-2200
pub const GAS_EXT_CODE_HASH_CONSTANTINOPLE: Gas = 400;
pub const GAS_EXT_CODE_HASH_ISTANBUL: Gas = 700; // EIP-1884
pub const GAS_SSTORE_SET: Gas = 20000;
pub const GAS_SSTORE_RESET: Gas = 5000;
pub const GAS_SSTORE_CLEAR: Gas = 5000;
pub const GAS_SSTORE_REFUND: Gas = 15000;
pub const GAS_SSTORE_SET_ISTANBUL: Gas = 20000;
pub const GAS_SSTORE_RESET_ISTANBUL: Gas = 5000;
pub const GAS_SSTORE_REFUND_ISTANBUL: Gas = 15000;
pub const GAS_SSTORE_SENTRY_ISTANBUL: Gas = 2300;
pub const GAS_WARM_STORAGE_READ_COST_BERLIN: Gas = 100; // EIP-2929
pub const GAS_COLD_SLOAD_COST_BERLIN: Gas = 2100; // EIP-2929
pub const GAS_COLD_ACCOUNT_ACCESS_COST_BERLIN: Gas = 2600; // EIP-2929
pub const GAS_SELF_DESTRUCT_REFUND: Gas = 24000;
pub const GAS_TX_ACCESS_LIST_STORAGE_KEY: Gas = 1900;
pub const GAS_SSTORE_REFUND_LONDON: Gas = GAS_SSTORE_RESET_ISTANBUL -
    GAS_COLD_SLOAD_COST_BERLIN +
    GAS_TX_ACCESS_LIST_STORAGE_KEY; // EIP-3529
pub const GAS_TX: Gas = 21000;
