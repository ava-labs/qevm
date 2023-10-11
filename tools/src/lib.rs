pub mod dummy;
pub mod merkle;
use num_traits::FromPrimitive;
pub fn opcode_to_str(opcode: u8) -> Option<&'static str> {
    let opcode: Option<qevm::core::opcode::Opcode> =
        FromPrimitive::from_u8(opcode);
    use qevm::core::opcode::Opcode::*;
    opcode.map(|opcode| match opcode {
        Stop => "Stop",
        Add => "Add",
        Mul => "Mul",
        Sub => "Sub",
        Div => "Div",
        SDiv => "SDiv",
        Mod => "Mod",
        SMod => "SMod",
        AddMod => "AddMod",
        MulMod => "MulMod",
        Exp => "Exp",
        SignExtend => "SignExtend",
        Lt => "Lt",
        Gt => "Gt",
        Slt => "Slt",
        Sgt => "Sgt",
        Eql => "Eql",
        IsZero => "IsZero",
        And => "And",
        Or => "Or",
        Xor => "Xor",
        Not => "Not",
        Byte => "Byte",
        Shl => "Shl",
        Shr => "Shr",
        Sar => "Sar",
        Sha3 => "Sha3",
        Addr => "Addr",
        Balance => "Balance",
        Origin => "Origin",
        Caller => "Caller",
        CallValue => "CallValue",
        CallDataLoad => "CallDataLoad",
        CallDataSize => "CallDataSize",
        CallDataCopy => "CallDataCopy",
        CodeSize => "CodeSize",
        CodeCopy => "CodeCopy",
        GasPrice => "GasPrice",
        ExtCodeSize => "ExtCodeSize",
        ExtCodeCopy => "ExtCodeCopy",
        ReturnDataSize => "ReturnDataSize",
        ReturnDataCopy => "ReturnDataCopy",
        ExtCodeHash => "ExtCodeHash",
        BlockHash => "BlockHash",
        Coinbase => "Coinbase",
        Timestamp => "Timestamp",
        Number => "Number",
        Difficulty => "Difficulty",
        GasLimit => "GasLimit",
        ChainId => "ChainId",
        SelfBalance => "SelfBalance",
        BaseFee => "BaseFee",
        Pop => "Pop",
        MLoad => "MLoad",
        MStore => "MStore",
        MStore8 => "MStore8",
        SLoad => "SLoad",
        SStore => "SStore",
        Jump => "Jump",
        JumpI => "JumpI",
        PC => "PC",
        MSize => "MSize",
        Gas => "Gas",
        JumpDest => "[JumpDest]",
        Push1 => "Push1",
        Push2 => "Push2",
        Push3 => "Push3",
        Push4 => "Push4",
        Push5 => "Push5",
        Push6 => "Push6",
        Push7 => "Push7",
        Push8 => "Push8",
        Push9 => "Push9",
        Push10 => "Push10",
        Push11 => "Push11",
        Push12 => "Push12",
        Push13 => "Push13",
        Push14 => "Push14",
        Push15 => "Push15",
        Push16 => "Push16",
        Push17 => "Push17",
        Push18 => "Push18",
        Push19 => "Push19",
        Push20 => "Push20",
        Push21 => "Push21",
        Push22 => "Push22",
        Push23 => "Push23",
        Push24 => "Push24",
        Push25 => "Push25",
        Push26 => "Push26",
        Push27 => "Push27",
        Push28 => "Push28",
        Push29 => "Push29",
        Push30 => "Push30",
        Push31 => "Push31",
        Push32 => "Push32",
        Dup1 => "Dup1",
        Dup2 => "Dup2",
        Dup3 => "Dup3",
        Dup4 => "Dup4",
        Dup5 => "Dup5",
        Dup6 => "Dup6",
        Dup7 => "Dup7",
        Dup8 => "Dup8",
        Dup9 => "Dup9",
        Dup10 => "Dup10",
        Dup11 => "Dup11",
        Dup12 => "Dup12",
        Dup13 => "Dup13",
        Dup14 => "Dup14",
        Dup15 => "Dup15",
        Dup16 => "Dup16",
        Swap1 => "Swap1",
        Swap2 => "Swap2",
        Swap3 => "Swap3",
        Swap4 => "Swap4",
        Swap5 => "Swap5",
        Swap6 => "Swap6",
        Swap7 => "Swap7",
        Swap8 => "Swap8",
        Swap9 => "Swap9",
        Swap10 => "Swap10",
        Swap11 => "Swap11",
        Swap12 => "Swap12",
        Swap13 => "Swap13",
        Swap14 => "Swap14",
        Swap15 => "Swap15",
        Swap16 => "Swap16",
        Log0 => "Log0",
        Log1 => "Log1",
        Log2 => "Log2",
        Log3 => "Log3",
        Log4 => "Log4",
        Push => "Push",
        Dup => "Dup",
        Swap => "Swap",
        Create => "Create",
        Call => "Call",
        CallCode => "CallCode",
        Return => "Return",
        DelegateCall => "DelegateCall",
        Create2 => "Create2",
        StaticCall => "StaticCall",
        Revert => "Revert",
        Invalid => "Invalid",
        SelfDestruct => "SelfDestruct",
    })
}

pub fn disasm(code: &[u8], line_breaks: bool) -> String {
    let bitmap = qevm::common::gen_code_bitmap(code);
    let mut asm = Vec::new();
    let mut bytes = Vec::new();
    for (i, (c, b)) in code.iter().zip(bitmap.iter().by_vals()).enumerate() {
        if b {
            if bytes.len() > 0 {
                asm.push(format!(
                    "{}0x{}",
                    if line_breaks { " " } else { "" },
                    hex::encode(&bytes)
                ));
                bytes.clear();
            }
            if asm.len() > 0 && line_breaks {
                asm.push("\n".into());
            }
            let prefix = if line_breaks {
                format!("{:04x} ", i)
            } else {
                "".into()
            };
            match opcode_to_str(*c) {
                Some(s) => asm.push(format!("{}{}", prefix, s)),
                None => asm.push(format!("{}0x{:02x}?", prefix, *c)),
            }
        } else {
            bytes.push(*c);
        }
    }
    if bytes.len() > 0 {
        asm.push(format!("0x{}", hex::encode(bytes)));
    }
    asm.join(if line_breaks { "" } else { " " })
}
