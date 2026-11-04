//! The operations of TGE 1.4's TorqueScript VM (the classic instruction set) and their operand layouts. An
//! [`OpcodeTable`] gives each operation its code word; [`OpcodeTable::TGE14`] is TGE 1.4's numbering (declaration
//! order, `Compiler::CompiledInstructions`).
//!
//! The operations and their order are those of TGE 1.4's `console/compiler.h`; the operand layouts are those
//! `console/compiledEval.cc` reads (Torque Game Engine 1.4, which Torque3D is the MIT release of; credited in the
//! crate documentation). The tables here are written for this crate.

macro_rules! define_ops {
    ($($variant:ident, $name:literal;)*) => {
        /// A bytecode operation.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Op {
            $($variant,)*
        }

        impl Op {
            /// Every operation, in declaration order.
            pub const ALL: &'static [Op] = &[$(Op::$variant,)*];

            /// The operation's name, for example `OP_FUNC_DECL`.
            pub const fn name(self) -> &'static str {
                match self {
                    $(Op::$variant => $name,)*
                }
            }
        }
    };
}

define_ops! {
    FuncDecl, "OP_FUNC_DECL";
    CreateObject, "OP_CREATE_OBJECT";
    AddObject, "OP_ADD_OBJECT";
    EndObject, "OP_END_OBJECT";
    JmpIfFNot, "OP_JMPIFFNOT";
    JmpIfNot, "OP_JMPIFNOT";
    JmpIfF, "OP_JMPIFF";
    JmpIf, "OP_JMPIF";
    JmpIfNotNp, "OP_JMPIFNOT_NP";
    JmpIfNp, "OP_JMPIF_NP";
    Jmp, "OP_JMP";
    Return, "OP_RETURN";
    CmpEq, "OP_CMPEQ";
    CmpGr, "OP_CMPGR";
    CmpGe, "OP_CMPGE";
    CmpLt, "OP_CMPLT";
    CmpLe, "OP_CMPLE";
    CmpNe, "OP_CMPNE";
    Xor, "OP_XOR";
    Mod, "OP_MOD";
    BitAnd, "OP_BITAND";
    BitOr, "OP_BITOR";
    Not, "OP_NOT";
    NotF, "OP_NOTF";
    OnesComplement, "OP_ONESCOMPLEMENT";
    Shr, "OP_SHR";
    Shl, "OP_SHL";
    And, "OP_AND";
    Or, "OP_OR";
    Add, "OP_ADD";
    Sub, "OP_SUB";
    Mul, "OP_MUL";
    Div, "OP_DIV";
    Neg, "OP_NEG";
    SetCurVar, "OP_SETCURVAR";
    SetCurVarCreate, "OP_SETCURVAR_CREATE";
    SetCurVarArray, "OP_SETCURVAR_ARRAY";
    SetCurVarArrayCreate, "OP_SETCURVAR_ARRAY_CREATE";
    LoadVarUint, "OP_LOADVAR_UINT";
    LoadVarFlt, "OP_LOADVAR_FLT";
    LoadVarStr, "OP_LOADVAR_STR";
    SaveVarUint, "OP_SAVEVAR_UINT";
    SaveVarFlt, "OP_SAVEVAR_FLT";
    SaveVarStr, "OP_SAVEVAR_STR";
    SetCurObject, "OP_SETCUROBJECT";
    SetCurObjectNew, "OP_SETCUROBJECT_NEW";
    SetCurField, "OP_SETCURFIELD";
    SetCurFieldArray, "OP_SETCURFIELD_ARRAY";
    LoadFieldUint, "OP_LOADFIELD_UINT";
    LoadFieldFlt, "OP_LOADFIELD_FLT";
    LoadFieldStr, "OP_LOADFIELD_STR";
    SaveFieldUint, "OP_SAVEFIELD_UINT";
    SaveFieldFlt, "OP_SAVEFIELD_FLT";
    SaveFieldStr, "OP_SAVEFIELD_STR";
    StrToUint, "OP_STR_TO_UINT";
    StrToFlt, "OP_STR_TO_FLT";
    StrToNone, "OP_STR_TO_NONE";
    FltToUint, "OP_FLT_TO_UINT";
    FltToStr, "OP_FLT_TO_STR";
    FltToNone, "OP_FLT_TO_NONE";
    UintToFlt, "OP_UINT_TO_FLT";
    UintToStr, "OP_UINT_TO_STR";
    UintToNone, "OP_UINT_TO_NONE";
    LoadImmedUint, "OP_LOADIMMED_UINT";
    LoadImmedFlt, "OP_LOADIMMED_FLT";
    TagToStr, "OP_TAG_TO_STR";
    LoadImmedStr, "OP_LOADIMMED_STR";
    LoadImmedIdent, "OP_LOADIMMED_IDENT";
    CallFuncResolve, "OP_CALLFUNC_RESOLVE";
    CallFunc, "OP_CALLFUNC";
    AdvanceStr, "OP_ADVANCE_STR";
    AdvanceStrAppendChar, "OP_ADVANCE_STR_APPENDCHAR";
    AdvanceStrComma, "OP_ADVANCE_STR_COMMA";
    AdvanceStrNul, "OP_ADVANCE_STR_NUL";
    RewindStr, "OP_REWIND_STR";
    TerminateRewindStr, "OP_TERMINATE_REWIND_STR";
    CompareStr, "OP_COMPARE_STR";
    Push, "OP_PUSH";
    PushFrame, "OP_PUSH_FRAME";
    Break, "OP_BREAK";
    Invalid, "OP_INVALID";
}

/// What one operand word means. This table is the single description of operand layout; the decoder, the
/// classic executor and the classic code generator read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperandShape {
    /// A string-table offset that the identifier table fills in at load time (0 on disk).
    Ident,
    /// 0 or 1.
    Flag,
    /// An absolute code index.
    Target,
    /// How many identifier words follow the fixed operands (only `FuncDecl`).
    Count,
    /// A plain number.
    Immediate,
    /// One character.
    Char,
    /// A byte offset into a string table (global or function table, depending on context).
    StringOffset,
    /// An index into a float table (global or function table, depending on context).
    FloatIndex,
}

impl Op {
    /// The number of operations.
    pub const COUNT: usize = Op::ALL.len();

    /// The position in [`Op::ALL`], which is the operation's code in TGE 1.4's numbering.
    pub const fn index(self) -> usize {
        self as usize
    }

    /// The fixed operand words after the opcode word.
    pub const fn operands(self) -> &'static [OperandShape] {
        use OperandShape::{Char, Count, Flag, FloatIndex, Ident, Immediate, StringOffset, Target};
        match self {
            Op::FuncDecl => &[Ident, Ident, Ident, Flag, Target, Count],
            Op::CreateObject => &[Ident, Flag, Target],
            Op::AddObject | Op::EndObject => &[Flag],
            Op::Jmp
            | Op::JmpIf
            | Op::JmpIfNot
            | Op::JmpIfF
            | Op::JmpIfFNot
            | Op::JmpIfNp
            | Op::JmpIfNotNp => &[Target],
            Op::SetCurVar | Op::SetCurVarCreate | Op::SetCurField | Op::LoadImmedIdent => &[Ident],
            Op::LoadImmedUint => &[Immediate],
            Op::LoadImmedFlt => &[FloatIndex],
            Op::LoadImmedStr | Op::TagToStr => &[StringOffset],
            Op::CallFunc | Op::CallFuncResolve => &[Ident, Ident, Immediate],
            Op::AdvanceStrAppendChar => &[Char],
            _ => &[],
        }
    }

    /// True when identifier words follow the fixed operands, as many as the `Count` operand says.
    pub const fn has_ident_tail(self) -> bool {
        matches!(self, Op::FuncDecl)
    }
}

/// Two entries of a numbering used the same code word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DuplicateCode(pub u32);

/// A numbering of the operations: which code word stands for which [`Op`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpcodeTable {
    codes: [u32; Op::COUNT],
}

const fn sequential_codes() -> [u32; Op::COUNT] {
    let mut codes = [0u32; Op::COUNT];
    let mut i = 0;
    while i < Op::COUNT {
        codes[i] = i as u32;
        i += 1;
    }
    codes
}

impl OpcodeTable {
    /// TGE 1.4's numbering: each operation's code is its position in [`Op::ALL`].
    pub const TGE14: OpcodeTable = OpcodeTable {
        codes: sequential_codes(),
    };

    /// A numbering as a constant: `codes[op.index()]` is the code of `op`.
    ///
    /// # Panics
    ///
    /// Panics (at compile time in a `const`) when two codes are the same.
    pub const fn numbered(codes: [u32; Op::COUNT]) -> OpcodeTable {
        let mut i = 0;
        while i < Op::COUNT {
            let mut j = 0;
            while j < i {
                assert!(codes[i] != codes[j], "two operations have the same code");
                j += 1;
            }
            i += 1;
        }
        OpcodeTable { codes }
    }

    /// `codes[op.index()]` is the code of `op`. Every code must be distinct.
    pub fn from_codes(codes: [u32; Op::COUNT]) -> Result<OpcodeTable, DuplicateCode> {
        for (i, c) in codes.iter().enumerate() {
            if codes[..i].contains(c) {
                return Err(DuplicateCode(*c));
            }
        }
        Ok(OpcodeTable { codes })
    }

    pub const fn code(&self, op: Op) -> u32 {
        self.codes[op.index()]
    }

    /// Linear search; a consumer that needs speed builds its own jump table.
    pub fn op(&self, code: u32) -> Option<Op> {
        self.codes
            .iter()
            .position(|&c| c == code)
            .map(|i| Op::ALL[i])
    }
}
