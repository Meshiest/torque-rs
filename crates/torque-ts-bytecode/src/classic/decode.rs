//! Turns the code words of a classic [`DsoFile`] into instructions, checks them for consistency and prints them.

use std::collections::{HashMap, HashSet};
use std::fmt::{self, Write as _};

use crate::classic::ClassicDialect;
use crate::classic::opcodes::{Op, OperandShape};
use crate::dso::DsoFile;

/// Which pair of tables an immediate refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Table {
    Global,
    Function,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperandKind {
    /// A string-table offset that the identifier table fills in at load time (0 on disk).
    Ident,
    /// 0 or 1.
    Flag,
    /// An absolute code index.
    Target,
    /// How many identifiers follow.
    Count,
    /// A plain number.
    Immediate,
    /// One character.
    Char,
    /// A byte offset into a string table.
    StringIndex(Table),
    /// An index into a float table.
    FloatIndex(Table),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Operand {
    pub kind: OperandKind,
    /// The code index of this operand word.
    pub address: u32,
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
        1 + self.operands.len() as u32
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

fn push(
    code: &[u32],
    cursor: &mut usize,
    kind: OperandKind,
    operands: &mut Vec<Operand>,
    address: u32,
    op: Op,
) -> Result<u32, DecodeError> {
    let Some(&word) = code.get(*cursor) else {
        return Err(DecodeError::TruncatedOperands { address, op });
    };
    operands.push(Operand {
        kind,
        address: *cursor as u32,
        word,
    });
    *cursor += 1;
    Ok(word)
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
    }
}

/// Decodes the whole code array with the dialect's numbering. Never panics, whatever the words are.
pub fn decode_dso(
    dso: &DsoFile,
    dialect: &ClassicDialect,
) -> Result<Vec<Instruction>, DecodeError> {
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

        let mut operands = Vec::new();
        let mut cursor = ip + 1;
        let mut target_word = 0u32;
        let mut count_word = 0u32;
        for shape in op.operands() {
            let word = push(
                code,
                &mut cursor,
                kind_of(*shape, table),
                &mut operands,
                address,
                op,
            )?;
            match shape {
                OperandShape::Target => target_word = word,
                OperandShape::Count => count_word = word,
                _ => {}
            }
        }
        if op.has_ident_tail() {
            for _ in 0..count_word {
                push(
                    code,
                    &mut cursor,
                    OperandKind::Ident,
                    &mut operands,
                    address,
                    op,
                )?;
            }
            function_end = Some(target_word);
        }

        ip = cursor;
        out.push(Instruction {
            address,
            op,
            operands,
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
pub fn verify(
    dso: &DsoFile,
    dialect: &ClassicDialect,
    instructions: &[Instruction],
) -> Vec<String> {
    let mut problems = Vec::new();
    let global = dialect.global_strings(dso);
    let function = dialect.function_strings(dso);

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

    for instruction in instructions {
        for operand in &instruction.operands {
            match operand.kind {
                OperandKind::Target if !boundaries.contains(&operand.word) => add(
                    &mut problems,
                    format!(
                        "{} at {}: branch target {} is not an instruction boundary",
                        instruction.op.name(),
                        instruction.address,
                        operand.word
                    ),
                ),
                OperandKind::Ident
                    if !table_positions.contains_key(&operand.address) && operand.word != 0 =>
                {
                    add(
                        &mut problems,
                        format!(
                            "{} at {}: identifier operand {} has value {} but no identifier entry",
                            instruction.op.name(),
                            instruction.address,
                            operand.address,
                            operand.word
                        ),
                    );
                }
                OperandKind::StringIndex(t) => {
                    let table = if t == Table::Global {
                        &global
                    } else {
                        &function
                    };
                    if !table.is_string_start(operand.word) {
                        add(
                            &mut problems,
                            format!(
                                "{} at {}: string offset {} is not a string start in the {:?} table",
                                instruction.op.name(),
                                instruction.address,
                                operand.word,
                                t
                            ),
                        );
                    }
                }
                OperandKind::FloatIndex(t) => {
                    let len = if t == Table::Global {
                        dso.global_floats.len()
                    } else {
                        dso.function_floats.len()
                    };
                    if operand.word as usize >= len {
                        add(
                            &mut problems,
                            format!(
                                "{} at {}: float index {} is outside the {:?} table of {}",
                                instruction.op.name(),
                                instruction.address,
                                operand.word,
                                t,
                                len
                            ),
                        );
                    }
                }
                _ => {}
            }
        }
    }
    problems
}

/// Double-quotes string-table bytes without decoding them (the tables hold 8-bit text, not UTF-8):
/// printable ASCII as is, `"` and `\` backslash-escaped, every other byte as `\xNN`.
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
pub fn render(dso: &DsoFile, dialect: &ClassicDialect, instructions: &[Instruction]) -> String {
    let global = dialect.global_strings(dso);
    let function = dialect.function_strings(dso);
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
