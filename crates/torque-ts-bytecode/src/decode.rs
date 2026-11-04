//! Turns the code words of a [`DsoFile`] into instructions, checks them for consistency and prints them.
//!
//! The operand layouts are Torque3D's (see the crate documentation for the files and their copyright lines).

use std::collections::{HashMap, HashSet};
use std::fmt::{self, Write as _};

use crate::dialect::Dialect;
use crate::dso::DsoFile;
use crate::opcodes::{IDENT_WORDS, Op, OperandShape, Tail};

/// Which pair of tables an immediate refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Table {
    Global,
    Function,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperandKind {
    /// A name ([`IDENT_WORDS`] words, 0 on disk) that the identifier table fills in at load time.
    Ident,
    /// 0 or 1.
    Flag,
    /// An absolute code index.
    Target,
    /// How many arguments a function declares.
    Count,
    /// A plain number.
    Immediate,
    /// One character.
    Char,
    /// A byte offset into a string table.
    StringIndex(Table),
    /// An index into a float table.
    FloatIndex(Table),
    /// A local variable's register.
    Register,
    /// `has body | line << 1` of a function declaration.
    BodyAndLine,
}

impl OperandKind {
    /// The code words the operand takes.
    pub fn words(self) -> u32 {
        if self == OperandKind::Ident {
            IDENT_WORDS
        } else {
            1
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Operand {
    pub kind: OperandKind,
    /// The code index of the operand's (first) word.
    pub address: u32,
    /// The operand's (first) word.
    pub word: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instruction {
    pub address: u32,
    pub op: Op,
    pub operands: Vec<Operand>,
}

impl Instruction {
    /// Code words occupied, including the opcode word.
    pub fn words(&self) -> u32 {
        1 + self.operands.iter().map(|o| o.kind.words()).sum::<u32>()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    InvalidOpcode { address: u32, word: u32 },
    TruncatedOperands { address: u32, op: Op },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::InvalidOpcode { address, word } => {
                write!(f, "invalid opcode {word:#x} at {address}")
            }
            DecodeError::TruncatedOperands { address, op } => {
                write!(
                    f,
                    "{} at {address} runs past the end of the code",
                    op.name()
                )
            }
        }
    }
}

impl std::error::Error for DecodeError {}

/// Reads the operands of one instruction.
struct Cursor<'a> {
    code: &'a [u32],
    at: usize,
    address: u32,
    op: Op,
    operands: Vec<Operand>,
}

impl Cursor<'_> {
    fn push(&mut self, kind: OperandKind) -> Result<u32, DecodeError> {
        let width = kind.words() as usize;
        let truncated = DecodeError::TruncatedOperands {
            address: self.address,
            op: self.op,
        };
        let end = self.at.checked_add(width).ok_or(truncated.clone())?;
        if end > self.code.len() {
            return Err(truncated);
        }
        let word = self.code[self.at];
        self.operands.push(Operand {
            kind,
            address: self.at as u32,
            word,
        });
        self.at = end;
        Ok(word)
    }
}

fn kind_of(shape: OperandShape, table: Table) -> OperandKind {
    match shape {
        OperandShape::Ident => OperandKind::Ident,
        OperandShape::Flag => OperandKind::Flag,
        OperandShape::Target => OperandKind::Target,
        OperandShape::Count => OperandKind::Count,
        OperandShape::Immediate => OperandKind::Immediate,
        OperandShape::Char => OperandKind::Char,
        OperandShape::StringOffset => OperandKind::StringIndex(table),
        OperandShape::FloatIndex => OperandKind::FloatIndex(table),
        OperandShape::Register => OperandKind::Register,
        OperandShape::BodyAndLine => OperandKind::BodyAndLine,
    }
}

/// Decodes the whole code array with the dialect's numbering. Never panics, whatever the words are.
pub fn decode_dso(dso: &DsoFile, dialect: &Dialect) -> Result<Vec<Instruction>, DecodeError> {
    let code = &dso.code;
    let mut out = Vec::new();
    let mut ip = 0usize;
    let mut function_end: Option<u32> = None;

    while ip < code.len() {
        let address = ip as u32;
        if function_end.is_some_and(|end| address >= end) {
            function_end = None;
        }
        let word = code[ip];
        let op = match dialect.opcodes.op(word) {
            Some(Op::Invalid) | None => return Err(DecodeError::InvalidOpcode { address, word }),
            Some(op) => op,
        };
        let table = if function_end.is_some() {
            Table::Function
        } else {
            Table::Global
        };

        let mut cursor = Cursor {
            code,
            at: ip + 1,
            address,
            op,
            operands: Vec::new(),
        };
        let mut target_word = 0u32;
        let mut count_word = 0u32;
        let mut flag_word = 0u32;
        for shape in op.operands() {
            let word = cursor.push(kind_of(*shape, table))?;
            match shape {
                OperandShape::Target => target_word = word,
                OperandShape::Count => count_word = word,
                OperandShape::Flag => flag_word = word,
                _ => {}
            }
        }
        match op.tail() {
            Tail::None => {}
            Tail::Arguments => {
                for kind in [
                    OperandKind::Register,
                    OperandKind::Flag,
                    OperandKind::Target,
                ] {
                    for _ in 0..count_word {
                        cursor.push(kind)?;
                    }
                }
                function_end = Some(target_word);
            }
            Tail::IterVariable => {
                cursor.push(if flag_word != 0 {
                    OperandKind::Ident
                } else {
                    OperandKind::Register
                })?;
                cursor.push(OperandKind::Target)?;
            }
        }

        ip = cursor.at;
        out.push(Instruction {
            address,
            op,
            operands: cursor.operands,
        });
    }
    Ok(out)
}

/// The most problems [`verify`] collects; a result this long may be a truncated list.
pub const MAX_PROBLEMS: usize = 50;

fn add(problems: &mut Vec<String>, message: String) {
    if problems.len() < MAX_PROBLEMS {
        problems.push(message);
    }
}

/// Checks that the instructions and the file's tables agree with each other. An empty result means
/// consistent; each entry describes one problem (at most [`MAX_PROBLEMS`] are reported).
pub fn verify(dso: &DsoFile, instructions: &[Instruction]) -> Vec<String> {
    let mut problems = Vec::new();
    let global = dso.global_string_table();
    let function = dso.function_string_table();

    let mut boundaries: HashSet<u32> = instructions.iter().map(|i| i.address).collect();
    boundaries.insert(dso.code.len() as u32);

    let ident_slots: HashSet<u32> = instructions
        .iter()
        .flat_map(|i| &i.operands)
        .filter(|o| o.kind == OperandKind::Ident)
        .map(|o| o.address)
        .collect();

    let mut table_positions: HashMap<u32, u32> = HashMap::new();
    for entry in &dso.identifiers {
        if !global.is_string_start(entry.string_index) {
            add(
                &mut problems,
                format!(
                    "identifier string index {} is not the start of a string in the global table",
                    entry.string_index
                ),
            );
        }
        for &position in &entry.positions {
            if table_positions
                .insert(position, entry.string_index)
                .is_some()
            {
                add(
                    &mut problems,
                    format!("identifier position {position} appears more than once"),
                );
            }
            if !ident_slots.contains(&position) {
                add(
                    &mut problems,
                    format!("identifier table position {position} is not an identifier operand"),
                );
            }
        }
    }

    // The end and the register count of the function being walked.
    let mut current: Option<(u32, u32)> = None;
    for instruction in instructions {
        if current.is_some_and(|(end, _)| instruction.address >= end) {
            current = None;
        }
        let mut report = |text: String| {
            add(
                &mut problems,
                format!(
                    "{} at {}: {text}",
                    instruction.op.name(),
                    instruction.address
                ),
            );
        };
        for operand in &instruction.operands {
            match operand.kind {
                OperandKind::Target if !boundaries.contains(&operand.word) => report(format!(
                    "branch target {} is not an instruction boundary",
                    operand.word
                )),
                OperandKind::Ident => {
                    if !table_positions.contains_key(&operand.address) && operand.word != 0 {
                        report(format!(
                            "identifier operand {} has value {} but no identifier entry",
                            operand.address, operand.word
                        ));
                    }
                    let high = (1..IDENT_WORDS)
                        .filter_map(|k| dso.code.get(operand.address as usize + k as usize))
                        .any(|&w| w != 0);
                    if high {
                        report(format!(
                            "identifier operand {} has a nonzero high word",
                            operand.address
                        ));
                    }
                }
                OperandKind::Register => {
                    if let Some((_, registers)) = current
                        && operand.word >= registers
                    {
                        report(format!(
                            "register {} is outside the function's {registers} registers",
                            operand.word
                        ));
                    }
                }
                OperandKind::StringIndex(t) => {
                    let table = if t == Table::Global {
                        &global
                    } else {
                        &function
                    };
                    if !table.is_string_start(operand.word) {
                        report(format!(
                            "string offset {} is not a string start in the {t:?} table",
                            operand.word
                        ));
                    }
                }
                OperandKind::FloatIndex(t) => {
                    let len = if t == Table::Global {
                        dso.global_floats.len()
                    } else {
                        dso.function_floats.len()
                    };
                    if operand.word as usize >= len {
                        report(format!(
                            "float index {} is outside the {t:?} table of {len}",
                            operand.word
                        ));
                    }
                }
                _ => {}
            }
        }
        if instruction.op == Op::FuncDecl {
            let word = |k: usize| instruction.operands.get(k).map_or(0, |o| o.word);
            // The end target is the fifth fixed operand, the register count the seventh.
            current = Some((word(4), word(6)));
        }
    }
    problems
}

/// Double-quotes string-table bytes without decoding them (the tables hold 8-bit text, not UTF-8): printable
/// ASCII as is, `"` and `\` backslash-escaped, every other byte as `\xNN`.
fn quote(bytes: Option<&[u8]>) -> String {
    let Some(bytes) = bytes else {
        return "<bad index>".to_string();
    };
    let mut out = String::with_capacity(bytes.len() + 2);
    out.push('"');
    for &b in bytes {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            0x20..=0x7e => out.push(char::from(b)),
            _ => {
                let _ = write!(out, "\\x{b:02x}");
            }
        }
    }
    out.push('"');
    out
}

/// One line per instruction: `000008: OP_LOADIMMED_STR "hi"`.
pub fn render(dso: &DsoFile, instructions: &[Instruction]) -> String {
    let global = dso.global_string_table();
    let function = dso.function_string_table();
    let mut idents: HashMap<u32, u32> = HashMap::new();
    for entry in &dso.identifiers {
        for &position in &entry.positions {
            idents.insert(position, entry.string_index);
        }
    }

    let mut out = String::new();
    for instruction in instructions {
        let _ = write!(out, "{:06}: {}", instruction.address, instruction.op.name());
        for operand in &instruction.operands {
            let text = match operand.kind {
                OperandKind::Ident => match idents.get(&operand.address) {
                    Some(&index) => quote(global.get(index)),
                    None => "-".to_string(),
                },
                OperandKind::Flag | OperandKind::Immediate => operand.word.to_string(),
                OperandKind::Target => format!("->{:06}", operand.word),
                OperandKind::Count => format!("n={}", operand.word),
                OperandKind::Register => format!("r{}", operand.word),
                OperandKind::BodyAndLine => {
                    format!("body={} line={}", operand.word & 1, operand.word >> 1)
                }
                OperandKind::Char => match char::from_u32(operand.word) {
                    Some(c) if !c.is_control() => format!("{c:?}"),
                    _ => format!("{:#x}", operand.word),
                },
                OperandKind::StringIndex(t) => {
                    let table = if t == Table::Global {
                        &global
                    } else {
                        &function
                    };
                    quote(table.get(operand.word))
                }
                OperandKind::FloatIndex(t) => {
                    let floats = if t == Table::Global {
                        &dso.global_floats
                    } else {
                        &dso.function_floats
                    };
                    floats
                        .get(operand.word as usize)
                        .map_or_else(|| "?".to_string(), |f| format!("{f:?}"))
                }
            };
            out.push(' ');
            out.push_str(&text);
        }
        out.push('\n');
    }
    out
}
