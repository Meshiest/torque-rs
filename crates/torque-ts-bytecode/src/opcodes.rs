//! The operations of the TorqueScript VM (Torque3D's instruction set) and their operand layouts. An
//! [`OpcodeTable`] gives each operation its code word; [`OpcodeTable::TORQUE3D`] is Torque3D's numbering
//! (declaration order, `Compiler::CompiledInstructions`).
//!
//! The operations and their order are those of Torque3D's MIT-licensed `console/torquescript/compiler.h`
//! (Copyright (c) 2012 GarageGames, LLC); the operand layouts are those `console/torquescript/compiledEval.cpp`
//! reads (Copyright (c) 2013 GarageGames, LLC; Copyright (c) 2015 Faust Logic, Inc.; Copyright (c) 2021 TGEMIT
//! Authors & Contributors). The tables here are written for this crate.

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
    DefaultEnd, "OP_DEFAULT_END";
    CreateObject, "OP_CREATE_OBJECT";
    AddObject, "OP_ADD_OBJECT";
    EndObject, "OP_END_OBJECT";
    FinishObject, "OP_FINISH_OBJECT";
    JmpIfFNot, "OP_JMPIFFNOT";
    JmpIfNot, "OP_JMPIFNOT";
    JmpNotString, "OP_JMPNOTSTRING";
    JmpIfF, "OP_JMPIFF";
    JmpIf, "OP_JMPIF";
    JmpIfNotNp, "OP_JMPIFNOT_NP";
    JmpIfNp, "OP_JMPIF_NP";
    Jmp, "OP_JMP";
    Return, "OP_RETURN";
    ReturnVoid, "OP_RETURN_VOID";
    ReturnFlt, "OP_RETURN_FLT";
    ReturnUint, "OP_RETURN_UINT";
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
    Inc, "OP_INC";
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
    LoadLocalVarUint, "OP_LOAD_LOCAL_VAR_UINT";
    LoadLocalVarFlt, "OP_LOAD_LOCAL_VAR_FLT";
    LoadLocalVarStr, "OP_LOAD_LOCAL_VAR_STR";
    SaveLocalVarUint, "OP_SAVE_LOCAL_VAR_UINT";
    SaveLocalVarFlt, "OP_SAVE_LOCAL_VAR_FLT";
    SaveLocalVarStr, "OP_SAVE_LOCAL_VAR_STR";
    SetCurObject, "OP_SETCUROBJECT";
    SetCurObjectNew, "OP_SETCUROBJECT_NEW";
    SetCurObjectInternal, "OP_SETCUROBJECT_INTERNAL";
    SetCurField, "OP_SETCURFIELD";
    SetCurFieldArray, "OP_SETCURFIELD_ARRAY";
    SetCurFieldType, "OP_SETCURFIELD_TYPE";
    LoadFieldUint, "OP_LOADFIELD_UINT";
    LoadFieldFlt, "OP_LOADFIELD_FLT";
    LoadFieldStr, "OP_LOADFIELD_STR";
    SaveFieldUint, "OP_SAVEFIELD_UINT";
    SaveFieldFlt, "OP_SAVEFIELD_FLT";
    SaveFieldStr, "OP_SAVEFIELD_STR";
    PopStk, "OP_POP_STK";
    LoadImmedUint, "OP_LOADIMMED_UINT";
    LoadImmedFlt, "OP_LOADIMMED_FLT";
    TagToStr, "OP_TAG_TO_STR";
    LoadImmedStr, "OP_LOADIMMED_STR";
    DocblockStr, "OP_DOCBLOCK_STR";
    LoadImmedIdent, "OP_LOADIMMED_IDENT";
    CallFunc, "OP_CALLFUNC";
    AdvanceStrAppendChar, "OP_ADVANCE_STR_APPENDCHAR";
    RewindStr, "OP_REWIND_STR";
    TerminateRewindStr, "OP_TERMINATE_REWIND_STR";
    CompareStr, "OP_COMPARE_STR";
    Push, "OP_PUSH";
    PushFrame, "OP_PUSH_FRAME";
    Assert, "OP_ASSERT";
    Break, "OP_BREAK";
    IterBegin, "OP_ITER_BEGIN";
    IterBeginStr, "OP_ITER_BEGIN_STR";
    Iter, "OP_ITER";
    IterEnd, "OP_ITER_END";
    Invalid, "OP_INVALID";
}

/// What one operand means. This table is the single description of operand layout; the decoder reads it, and
/// the compiler writes to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperandShape {
    /// A name: [`IDENT_WORDS`] words, all 0 on disk; the identifier table lists the first word's position and
    /// the loader stores the name there.
    Ident,
    /// 0 or 1.
    Flag,
    /// An absolute code index.
    Target,
    /// How many arguments a function declares (each has a register, a flag and a default target after the
    /// fixed operands).
    Count,
    /// A plain number.
    Immediate,
    /// One character.
    Char,
    /// A byte offset into a string table (global or function table, depending on context).
    StringOffset,
    /// An index into a float table (global or function table, depending on context).
    FloatIndex,
    /// A local variable's register in the current frame.
    Register,
    /// A function declaration's `has body | line << 1`.
    BodyAndLine,
}

/// The code words a name operand takes: the loader writes a 64-bit pointer over them.
pub const IDENT_WORDS: u32 = 2;

impl OperandShape {
    /// The code words the operand takes.
    pub const fn words(self) -> u32 {
        match self {
            OperandShape::Ident => IDENT_WORDS,
            _ => 1,
        }
    }
}

/// The operands that follow the fixed ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tail {
    None,
    /// `OP_FUNC_DECL`: for each of the `Count` arguments its register, then each one's flags (bit 0: it has a
    /// default value), then each one's default-value code start (0 for none).
    Arguments,
    /// `OP_ITER_BEGIN` and `OP_ITER_BEGIN_STR`: after the `Flag` operand (the loop variable is global), the
    /// variable (a name when global, else a register), then the target past the loop.
    IterVariable,
}

impl Op {
    /// The number of operations.
    pub const COUNT: usize = Op::ALL.len();

    /// The position in [`Op::ALL`], which is the operation's code in Torque3D's numbering.
    pub const fn index(self) -> usize {
        self as usize
    }

    /// The fixed operands after the opcode word.
    pub const fn operands(self) -> &'static [OperandShape] {
        use OperandShape::{
            BodyAndLine, Char, Count, Flag, FloatIndex, Ident, Immediate, Register, StringOffset,
            Target,
        };
        match self {
            Op::FuncDecl => &[Ident, Ident, Ident, BodyAndLine, Target, Count, Immediate],
            Op::CreateObject => &[Ident, Flag, Flag, Flag, Immediate, Target],
            Op::AddObject | Op::EndObject | Op::SetCurObjectInternal => &[Flag],
            Op::IterBegin | Op::IterBeginStr => &[Flag],
            Op::Jmp
            | Op::JmpIf
            | Op::JmpIfNot
            | Op::JmpNotString
            | Op::JmpIfF
            | Op::JmpIfFNot
            | Op::JmpIfNp
            | Op::JmpIfNotNp
            | Op::Iter => &[Target],
            Op::SetCurVar | Op::SetCurVarCreate | Op::SetCurField | Op::LoadImmedIdent => &[Ident],
            Op::Inc
            | Op::LoadLocalVarUint
            | Op::LoadLocalVarFlt
            | Op::LoadLocalVarStr
            | Op::SaveLocalVarUint
            | Op::SaveLocalVarFlt
            | Op::SaveLocalVarStr => &[Register],
            Op::LoadImmedUint | Op::SetCurFieldType | Op::PushFrame => &[Immediate],
            Op::LoadImmedFlt => &[FloatIndex],
            Op::LoadImmedStr | Op::TagToStr | Op::DocblockStr | Op::Assert => &[StringOffset],
            Op::CallFunc => &[Ident, Ident, Immediate],
            Op::AdvanceStrAppendChar => &[Char],
            _ => &[],
        }
    }

    /// The operands after the fixed ones.
    pub const fn tail(self) -> Tail {
        match self {
            Op::FuncDecl => Tail::Arguments,
            Op::IterBegin | Op::IterBeginStr => Tail::IterVariable,
            _ => Tail::None,
        }
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
    /// Torque3D's numbering: each operation's code is its position in [`Op::ALL`].
    pub const TORQUE3D: OpcodeTable = OpcodeTable {
        codes: sequential_codes(),
    };

    /// `codes[op.index()]` is the code of `op`. Every code must be distinct.
    pub fn from_codes(codes: [u32; Op::COUNT]) -> Result<OpcodeTable, DuplicateCode> {
        for (i, c) in codes.iter().enumerate() {
            if codes[..i].contains(c) {
                return Err(DuplicateCode(*c));
            }
        }
        Ok(OpcodeTable { codes })
    }

    pub fn code(&self, op: Op) -> u32 {
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
