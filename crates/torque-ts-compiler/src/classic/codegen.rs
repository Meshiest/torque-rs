//! The classic code generator: TGE 1.4's two passes over the tree, node by node (`precompile`/`compile` and
//! `precompileStmt`/`compileStmt` of every node class in `astNodes.cc`, driven by `CodeBlock::compile` and
//! `CodeBlock::compileExec` of `codeBlock.cc`), writing the classic instruction set in the dialect's numbering.
//!
//! * `precompile` computes each node's size and fills the string and float tables. The ORDER of those calls
//!   decides the order of the tables. Where TGE sums the sizes of two operands in one expression (binary nodes:
//!   float, integer, `$=`, `@`, `,`; the two branches of `?:`; the index and the object of `obj.f[i] op= v`) the
//!   left one is visited first, as the source reads; [`Generation::right_operand_first`] visits the right one
//!   first. A function's parameter names enter the global table when the second pass writes them, or before the
//!   function's name in the first pass ([`Generation::params_in_first_pass`]).
//! * `compile` writes the code. Jump targets come from the offsets `precompile` computed, as in TGE, so where the
//!   two passes disagree on a size (`"a" @ "b"` compiled for no value, as a `for` initialiser can be) the code
//!   is the same short code with the same wrong targets, and `precompile size mismatch` is reported. The words
//!   the second pass then never writes (after the file's final `OP_RETURN`: the code is laid out shorter than its
//!   size) are left uninitialised by TGE; here they hold [`gap_word`] (`OP_RETURN`), so a function whose end
//!   offset points past the written code ends the file quietly when `OP_FUNC_DECL` jumps there.
//! * Identifiers (variable, function, field, class and namespace names) are added to the global string table
//!   case-sensitively by `precompile` and looked up again ignoring case when the code is written; every slot
//!   that names one goes into the identifier table (`STEtoU32` in its DSO-writing form).
//!
//! Follows TGE 1.4's `console/astNodes.cc`, `console/compiler.cc` and `console/codeBlock.cc` (each Copyright (C)
//! GarageGames.com, Inc.); the MIT-licensed Torque3D successors are credited in the crate documentation.

use std::any::TypeId;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use torque_ts_bytecode::DsoFile;
use torque_ts_bytecode::classic::{ClassicDialect, Op};
use torque_ts_text::{f64_to_u32, format_g, to_f32, to_f64};

use crate::ast::{
    AssignOp, BinaryOp, Case, Concat, Expr, ExprKind, Function, ObjectDecl, Script, Stmt, StmtKind,
    UnaryOp,
};
use crate::classic::Generation;
use crate::tables::{FloatTable, IdentTable, StringTable};
use crate::{Diagnostic, Level};

/// What the code words the second pass never wrote hold after a `precompile size mismatch`: `OP_RETURN` in the
/// dialect's numbering, so code that jumps there ends as a return does.
pub fn gap_word(dialect: &ClassicDialect) -> u32 {
    dialect.opcodes.code(Op::Return)
}

/// `TypeReq`: what a node's value is wanted as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Type {
    None,
    UInt,
    Float,
    String,
}

fn conversion(from: Type, to: Type) -> Op {
    match (from, to) {
        (Type::String, Type::UInt) => Op::StrToUint,
        (Type::String, Type::Float) => Op::StrToFlt,
        (Type::String, Type::None) => Op::StrToNone,
        (Type::Float, Type::UInt) => Op::FltToUint,
        (Type::Float, Type::String) => Op::FltToStr,
        (Type::Float, Type::None) => Op::FltToNone,
        (Type::UInt, Type::Float) => Op::UintToFlt,
        (Type::UInt, Type::String) => Op::UintToStr,
        (Type::UInt, Type::None) => Op::UintToNone,
        _ => Op::Invalid,
    }
}

/// The operand type and opcode of a binary integer node (`getSubTypeOperand`).
fn int_operand(op: BinaryOp) -> (Type, Op) {
    match op {
        BinaryOp::BitXor => (Type::UInt, Op::Xor),
        BinaryOp::Mod => (Type::UInt, Op::Mod),
        BinaryOp::BitAnd => (Type::UInt, Op::BitAnd),
        BinaryOp::BitOr => (Type::UInt, Op::BitOr),
        BinaryOp::Lt => (Type::Float, Op::CmpLt),
        BinaryOp::Gt => (Type::Float, Op::CmpGr),
        BinaryOp::Ge => (Type::Float, Op::CmpGe),
        BinaryOp::Le => (Type::Float, Op::CmpLe),
        BinaryOp::Eq => (Type::Float, Op::CmpEq),
        BinaryOp::Ne => (Type::Float, Op::CmpNe),
        BinaryOp::Or => (Type::UInt, Op::Or),
        BinaryOp::And => (Type::UInt, Op::And),
        BinaryOp::Shr => (Type::UInt, Op::Shr),
        BinaryOp::Shl => (Type::UInt, Op::Shl),
        BinaryOp::Add => (Type::Float, Op::Add),
        BinaryOp::Sub => (Type::Float, Op::Sub),
        BinaryOp::Mul => (Type::Float, Op::Mul),
        BinaryOp::Div => (Type::Float, Op::Div),
    }
}

fn is_float_op(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div
    )
}

/// `getAssignOpTypeOp`.
fn assign_operand(op: AssignOp) -> (Type, Op) {
    match op {
        // `x++` and `x--` are `x += 1` and `x -= 1` (the tree's constant is the float 1).
        AssignOp::Add | AssignOp::Increment => (Type::Float, Op::Add),
        AssignOp::Sub | AssignOp::Decrement => (Type::Float, Op::Sub),
        AssignOp::Mul => (Type::Float, Op::Mul),
        AssignOp::Div => (Type::Float, Op::Div),
        AssignOp::Mod => (Type::UInt, Op::Mod),
        AssignOp::BitAnd => (Type::UInt, Op::BitAnd),
        AssignOp::BitXor => (Type::UInt, Op::Xor),
        AssignOp::BitOr => (Type::UInt, Op::BitOr),
        AssignOp::Shl => (Type::UInt, Op::Shl),
        AssignOp::Shr => (Type::UInt, Op::Shr),
    }
}

/// `consoleStringToNumber` (`StrConstNode`/`ConstantNode::precompile`): `dAtof` (the generation's `atof`),
/// rounded to a float; `true`/`false` (any case) when that is 0; else 0 and a warning when the node has a file.
fn string_to_number(text: &[u8], atof: fn(&[u8]) -> f64) -> (f64, bool) {
    let value = to_f64(to_f32(atof(text)));
    if value != 0.0 {
        return (value, false);
    }
    if text.eq_ignore_ascii_case(b"true") {
        (1.0, false)
    } else if text.eq_ignore_ascii_case(b"false") {
        (0.0, false)
    } else {
        (0.0, true)
    }
}

/// What `precompile` left on a node for `compile` (TGE keeps it in the node).
#[derive(Debug, Clone, Copy, Default)]
struct Info {
    /// A table index or offset.
    index: u32,
    /// A string or identifier's number (`fVal`).
    fval: f64,
    /// `if`, loop, `?:` and `!`: the test is an integer (else a float).
    integer: bool,
    /// `if`: else offset; loop: loop-block start; function: end offset; object: fail offset.
    a: u32,
    /// `if`: endif offset; loop: continue offset.
    b: u32,
    /// loop: break offset.
    c: u32,
}

impl Info {
    fn is_default(&self) -> bool {
        self.index == 0
            && self.fval.to_bits() == 0
            && !self.integer
            && self.a == 0
            && self.b == 0
            && self.c == 0
    }
}

/// An expression node, or one of the nodes TGE makes for a `switch` (`getSwitchOR`).
#[derive(Clone, Copy)]
enum Node<'a> {
    Expr(&'a Expr),
    /// `test == value` or `test $= value` for one case value.
    CaseTest {
        string: bool,
        test: &'a Expr,
        value: &'a Expr,
    },
    /// `||` of a case's value tests: `values[i]`'s test, or the rest.
    CaseOr {
        string: bool,
        test: &'a Expr,
        values: &'a [Expr],
    },
}

/// A statement node: a statement, or the `if` a `case` becomes.
#[derive(Clone, Copy)]
enum StmtRef<'a> {
    Stmt(&'a Stmt),
    /// `cases[0]` of a switch, its `else` the rest of the chain or the default.
    Case {
        string: bool,
        test: &'a Expr,
        cases: &'a [Case],
        default: Option<&'a [Stmt]>,
    },
}

/// A hasher for node addresses: a multiply and a fold, much cheaper than the default's flood-resistant one,
/// which keys the script does not choose do not need (the maps keyed by the script's texts and numbers keep
/// the default).
#[derive(Default)]
struct KeyHasher(u64);

impl Hasher for KeyHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(u64::from(b));
        }
    }

    fn write_u64(&mut self, n: u64) {
        let h = (self.0.rotate_left(5) ^ n).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        self.0 = h ^ (h >> 32);
    }

    fn write_usize(&mut self, n: usize) {
        self.write_u64(n as u64);
    }

    fn write_u8(&mut self, n: u8) {
        self.write_u64(u64::from(n));
    }
}

type KeyMap<K, V> = HashMap<K, V, BuildHasherDefault<KeyHasher>>;

/// Identifies a tree node: its address and its type. The address alone is not enough: a node and its first
/// field (an `ObjectDecl` and its inline `class` expression) may share one, whatever the layout rustc picks.
type Key = (usize, TypeId);

fn key<T: 'static>(node: &T) -> Key {
    (node as *const T as usize, TypeId::of::<T>())
}

pub(crate) struct Gen<'a> {
    file: Option<&'a [u8]>,
    dialect: &'a ClassicDialect,
    generation: Generation,
    global_strings: StringTable,
    function_strings: StringTable,
    global_floats: FloatTable,
    function_floats: FloatTable,
    /// The current tables are the function tables (inside a function declaration's precompile).
    function_tables: bool,
    /// `CodeBlock::smInFunction`: statements record line breaks only inside a function.
    in_function: bool,
    break_count: u32,
    line_breaks: Vec<u32>,
    idents: IdentTable,
    info: KeyMap<Key, Info>,
    code: Vec<u32>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    /// Code words written beyond the precompiled size (a size mismatch); never written to the file.
    overflow: u64,
    /// Nodes visited by both passes: a `switch` visits its tested expression once per case value, so a small
    /// file can make a large program; past [`MAX_VISITS`] the generator gives up.
    visits: u64,
    /// The string-table offsets of numbers converted to text, by (function tables, float, value bits): the
    /// same constants are converted again each time a `switch` precompiles its test expression.
    number_strings: HashMap<(bool, bool, u64), u32>,
}

/// What a run's compile warns about a size mismatch with (`CodeBlock::compileExec`).
pub const RUN_MISMATCH: &[u8] = b"precompile size mismatch";

/// The most node visits one file may cost (both passes): about 64 million, a code of several hundred MiB.
pub const MAX_VISITS: u64 = 1 << 26;
/// The largest code one file may have, in words (TGE has no bound).
pub const MAX_CODE_WORDS: u32 = 1 << 24;

impl<'a> Gen<'a> {
    pub(crate) fn new(
        file: Option<&'a [u8]>,
        dialect: &'a ClassicDialect,
        generation: Generation,
    ) -> Gen<'a> {
        Gen {
            file,
            dialect,
            generation,
            global_strings: StringTable::default(),
            function_strings: StringTable::default(),
            global_floats: FloatTable::default(),
            function_floats: FloatTable::default(),
            function_tables: false,
            in_function: false,
            break_count: 0,
            line_breaks: Vec::new(),
            idents: IdentTable::default(),
            info: KeyMap::default(),
            code: Vec::new(),
            diagnostics: Vec::new(),
            overflow: 0,
            visits: 0,
            number_strings: HashMap::new(),
        }
    }

    /// `strings().add(text, true, false)` for the text of a number (`text` makes it), remembered per table.
    fn number_string(&mut self, float: bool, bits: u64, text: impl FnOnce() -> String) -> u32 {
        let key = (self.function_tables, float, bits);
        if let Some(&index) = self.number_strings.get(&key) {
            return index;
        }
        let index = self.strings().add(text().as_bytes(), true, false);
        self.number_strings.insert(key, index);
        index
    }

    fn warn(&mut self, line: u32, text: &str) {
        // Evaluated text has no file: the message names `(null)`, as C runtimes print a null `%s`.
        let file = self.file.map_or_else(
            || "(null)".to_string(),
            |f| String::from_utf8_lossy(f).into_owned(),
        );
        self.diagnostics.push(Diagnostic {
            level: Level::Warning,
            text: format!("{file} ({line}): {text}").into_bytes(),
        });
    }

    fn strings(&mut self) -> &mut StringTable {
        if self.function_tables {
            &mut self.function_strings
        } else {
            &mut self.global_strings
        }
    }

    fn floats(&mut self) -> &mut FloatTable {
        if self.function_tables {
            &mut self.function_floats
        } else {
            &mut self.global_floats
        }
    }

    /// `precompileIdent`: the name goes into the global table, case-sensitively.
    fn precompile_ident(&mut self, name: &[u8]) {
        self.global_strings.add(name, true, false);
    }

    fn info(&self, k: Key) -> Info {
        self.info.get(&k).copied().unwrap_or_default()
    }

    /// Records a node's info. Most nodes have the default one (a constant used as an integer, a variable)
    /// and a missing entry reads as the default, so only the others are kept: a file of a million
    /// statements then keeps one entry per statement, not one per node.
    fn set_info(&mut self, k: Key, info: Info) {
        if info.is_default() {
            self.info.remove(&k);
        } else {
            self.info.insert(k, info);
        }
    }

    fn add_break_count(&mut self) {
        if self.in_function {
            self.break_count = self.break_count.saturating_add(1);
        }
    }

    fn add_break_line(&mut self, line: u32, ip: u32) {
        if self.in_function {
            self.line_breaks.push(line);
            self.line_breaks.push(ip);
        }
    }

    // ---------------------------------------------------------------- precompile

    pub(crate) fn precompile_block(&mut self, stmts: &'a [Stmt], loops: u32) -> u32 {
        let mut sum: u32 = 0;
        for stmt in stmts {
            sum = sum.wrapping_add(self.precompile_stmt(StmtRef::Stmt(stmt), loops));
        }
        sum
    }

    fn precompile_stmt(&mut self, stmt: StmtRef<'a>, loops: u32) -> u32 {
        if !self.visit() {
            return 0;
        }
        let (line, kind) = match stmt {
            StmtRef::Stmt(s) => (s.line, &s.kind),
            StmtRef::Case { .. } => return self.precompile_if(stmt, loops),
        };
        match kind {
            StmtKind::Break | StmtKind::Continue => {
                if loops > 0 {
                    self.add_break_count();
                    return 2;
                }
                let what = if matches!(kind, StmtKind::Break) {
                    "break"
                } else {
                    "continue"
                };
                self.warn(line, &format!("{what} outside of loop... ignoring."));
                0
            }
            StmtKind::Expr(e) => {
                self.add_break_count();
                self.precompile(Node::Expr(e), Type::None)
            }
            StmtKind::Return(e) => {
                self.add_break_count();
                match e {
                    None => 1,
                    Some(e) => 1u32.wrapping_add(self.precompile(Node::Expr(e), Type::String)),
                }
            }
            StmtKind::If { .. } | StmtKind::Switch { .. } => self.precompile_if(stmt, loops),
            StmtKind::Loop { .. } => self.precompile_loop(stmt, loops),
            // Not in the classic grammar (its parser makes none): no code.
            StmtKind::Foreach { .. } => 0,
            StmtKind::Function(f) => self.precompile_function(f, None),
            StmtKind::Package { name, functions } => {
                let mut sum: u32 = 0;
                for f in functions {
                    sum = sum.wrapping_add(self.precompile_function(f, Some(name)));
                }
                sum
            }
        }
    }

    /// The parts of an `if` (a real one, or the one a `case` becomes).
    fn if_parts(stmt: StmtRef<'a>) -> Option<(Node<'a>, IfBlock<'a>, Option<IfBlock<'a>>, Key)> {
        Some(match stmt {
            StmtRef::Stmt(s) => match &s.kind {
                StmtKind::If {
                    test,
                    then,
                    otherwise,
                } => (
                    Node::Expr(test),
                    IfBlock::Stmts(then),
                    otherwise.as_deref().map(IfBlock::Stmts),
                    key(s),
                ),
                StmtKind::Switch {
                    string,
                    test,
                    cases,
                    default,
                } => match cases.split_first() {
                    // A switch always has a case: the grammar requires one.
                    Some((first, _)) => (
                        Node::CaseOr {
                            string: *string,
                            test,
                            values: &first.values,
                        },
                        IfBlock::Stmts(&first.body),
                        Self::case_else(*string, test, cases, default.as_deref()),
                        key(s),
                    ),
                    None => (Node::Expr(test), IfBlock::Stmts(&[]), None, key(s)),
                },
                _ => return None,
            },
            StmtRef::Case {
                string,
                test,
                cases,
                default,
            } => match cases.split_first() {
                Some((first, _)) => (
                    Node::CaseOr {
                        string,
                        test,
                        values: &first.values,
                    },
                    IfBlock::Stmts(&first.body),
                    Self::case_else(string, test, cases, default),
                    key(first),
                ),
                None => (Node::Expr(test), IfBlock::Stmts(&[]), None, key(test)),
            },
        })
    }

    /// The `else` of the `if` made from `cases[0]`: the next case's `if`, or the default.
    fn case_else(
        string: bool,
        test: &'a Expr,
        cases: &'a [Case],
        default: Option<&'a [Stmt]>,
    ) -> Option<IfBlock<'a>> {
        match cases.get(1..) {
            Some(rest) if !rest.is_empty() => Some(IfBlock::Case(StmtRef::Case {
                string,
                test,
                cases: rest,
                default,
            })),
            _ => default.map(IfBlock::Stmts),
        }
    }

    fn precompile_if_block(&mut self, block: IfBlock<'a>, loops: u32) -> u32 {
        match block {
            IfBlock::Stmts(stmts) => self.precompile_block(stmts, loops),
            IfBlock::Case(case) => self.precompile_stmt(case, loops),
        }
    }

    fn precompile_if(&mut self, stmt: StmtRef<'a>, loops: u32) -> u32 {
        let Some((test, then, otherwise, k)) = Self::if_parts(stmt) else {
            return 0;
        };
        self.add_break_count();
        let integer = self.preferred(test) == Type::UInt;
        let expr_size = self.precompile(test, if integer { Type::UInt } else { Type::Float });
        let if_size = self.precompile_if_block(then, loops);
        let mut info = Info {
            integer,
            ..Info::default()
        };
        match otherwise {
            None => info.b = if_size.wrapping_add(2).wrapping_add(expr_size),
            Some(block) => {
                info.a = expr_size
                    .wrapping_add(2)
                    .wrapping_add(if_size)
                    .wrapping_add(2);
                let else_size = self.precompile_if_block(block, loops);
                info.b = info.a.wrapping_add(else_size);
            }
        }
        self.set_info(k, info);
        info.b
    }

    fn precompile_loop(&mut self, stmt: StmtRef<'a>, loops: u32) -> u32 {
        let StmtRef::Stmt(s) = stmt else { return 0 };
        let StmtKind::Loop {
            init,
            test,
            step,
            body,
            post_test,
        } = &s.kind
        else {
            return 0;
        };
        self.add_break_count();
        let init_size = init
            .as_ref()
            .map_or(0, |e| self.precompile(Node::Expr(e), Type::None));
        let integer = self.preferred(Node::Expr(test)) == Type::UInt;
        let test_size = self.precompile(
            Node::Expr(test),
            if integer { Type::UInt } else { Type::Float },
        );
        let block_size = self.precompile_block(body, loops.saturating_add(1));
        let step_size = step
            .as_ref()
            .map_or(0, |e| self.precompile(Node::Expr(e), Type::None));
        // A `do`-`while` loop has no test before its block.
        let start = if *post_test {
            init_size
        } else {
            init_size.wrapping_add(test_size).wrapping_add(2)
        };
        let cont = start.wrapping_add(block_size);
        let brk = cont
            .wrapping_add(step_size)
            .wrapping_add(test_size)
            .wrapping_add(2);
        self.set_info(
            key(s),
            Info {
                integer,
                a: start,
                b: cont,
                c: brk,
                ..Info::default()
            },
        );
        brk
    }

    fn precompile_function(&mut self, f: &'a Function, package: Option<&'a [u8]>) -> u32 {
        self.function_tables = true;
        if self.generation.params_in_first_pass {
            for p in &f.params {
                self.precompile_ident(&p.name);
            }
        }
        let argc = u32::try_from(f.params.len()).unwrap_or(u32::MAX);
        self.in_function = true;
        self.precompile_ident(&f.name);
        if let Some(ns) = &f.namespace {
            self.precompile_ident(ns);
        }
        if let Some(pkg) = package {
            self.precompile_ident(pkg);
        }
        let body = f.body.as_deref().map_or(0, |b| self.precompile_block(b, 0));
        self.in_function = false;
        self.function_tables = false;
        let end = argc.wrapping_add(body).wrapping_add(8);
        self.set_info(
            key(f),
            Info {
                a: end,
                ..Info::default()
            },
        );
        end
    }

    /// `getPreferredType`.
    fn preferred(&self, node: Node<'a>) -> Type {
        let e = match node {
            Node::Expr(e) => e,
            Node::CaseTest { .. } | Node::CaseOr { .. } => return Type::UInt,
        };
        match &e.kind {
            ExprKind::Int(_) | ExprKind::StrEq { .. } | ExprKind::New(_) => Type::UInt,
            ExprKind::Unary {
                op: UnaryOp::Neg, ..
            }
            | ExprKind::Float(_) => Type::Float,
            ExprKind::Unary { .. } => Type::UInt,
            ExprKind::Binary { op, .. } => {
                if is_float_op(*op) {
                    Type::Float
                } else {
                    Type::UInt
                }
            }
            ExprKind::Str { .. }
            | ExprKind::Ident(_)
            | ExprKind::Concat { .. }
            | ExprKind::CommaCat { .. }
            | ExprKind::Call { .. }
            | ExprKind::MethodCall { .. }
            | ExprKind::FieldAssign { .. } => Type::String,
            ExprKind::Var { .. }
            | ExprKind::Field { .. }
            | ExprKind::TagDeref(_)
            | ExprKind::InternalField { .. }
            | ExprKind::Assert { .. } => Type::None,
            ExprKind::Ternary { then, .. } => self.preferred(Node::Expr(then)),
            ExprKind::Assign { value, .. } => self.preferred(Node::Expr(value)),
            ExprKind::AssignOp { op, .. } | ExprKind::FieldAssignOp { op, .. } => {
                assign_operand(*op).0
            }
        }
    }

    /// Counts a visit; false once the budget is spent (the passes then stop descending).
    fn visit(&mut self) -> bool {
        self.visits = self.visits.saturating_add(1);
        self.visits <= MAX_VISITS
    }

    fn precompile(&mut self, node: Node<'a>, ty: Type) -> u32 {
        if !self.visit() {
            return 0;
        }
        let e = match node {
            Node::Expr(e) => e,
            Node::CaseTest {
                string,
                test,
                value,
            } => {
                return if string {
                    self.precompile_streq(test, value, true, ty)
                } else {
                    self.precompile_int_binary(
                        BinaryOp::Eq,
                        Node::Expr(test),
                        Node::Expr(value),
                        ty,
                    )
                };
            }
            Node::CaseOr {
                string,
                test,
                values,
            } => {
                let (left, right) = Self::case_or_parts(string, test, values);
                return match right {
                    None => self.precompile(left, ty),
                    Some(right) => self.precompile_int_binary(BinaryOp::Or, left, right, ty),
                };
            }
        };
        let k = key(e);
        match &e.kind {
            ExprKind::Binary { op, left, right } if is_float_op(*op) => {
                let (l, r) = self.precompile_pair((left, Type::Float), (right, Type::Float));
                r.wrapping_add(l)
                    .wrapping_add(1)
                    .wrapping_add(u32::from(ty != Type::Float))
            }
            ExprKind::Binary { op, left, right } => {
                self.precompile_int_binary(*op, Node::Expr(left), Node::Expr(right), ty)
            }
            ExprKind::StrEq { equal, left, right } => {
                self.precompile_streq(left, right, *equal, ty)
            }
            ExprKind::Concat { kind, left, right } => {
                let (l, r) = self.precompile_pair((left, Type::String), (right, Type::String));
                r.wrapping_add(l)
                    .wrapping_add(2)
                    .wrapping_add(u32::from(*kind != Concat::Plain))
                    .wrapping_add(u32::from(ty != Type::String))
            }
            ExprKind::CommaCat { left, right } => {
                let (l, r) = self.precompile_pair((left, Type::String), (right, Type::String));
                r.wrapping_add(l)
                    .wrapping_add(2)
                    .wrapping_add(u32::from(ty != Type::String))
            }
            ExprKind::Unary {
                op: UnaryOp::Neg,
                expr,
            } => {
                let size = self.precompile(Node::Expr(expr), Type::Float);
                size.wrapping_add(if ty != Type::Float { 2 } else { 1 })
            }
            ExprKind::Unary { op, expr } => {
                let pref = self.preferred(Node::Expr(expr));
                let integer =
                    !((*op == UnaryOp::Not && pref == Type::Float) || pref == Type::String);
                self.set_info(
                    k,
                    Info {
                        integer,
                        ..Info::default()
                    },
                );
                let size = self.precompile(
                    Node::Expr(expr),
                    if integer { Type::UInt } else { Type::Float },
                );
                size.wrapping_add(if ty != Type::UInt { 2 } else { 1 })
            }
            ExprKind::Ternary {
                test,
                then,
                otherwise,
            } => {
                let integer = self.preferred(Node::Expr(test)) == Type::UInt;
                let size = self.precompile(
                    Node::Expr(test),
                    if integer { Type::UInt } else { Type::Float },
                );
                self.set_info(
                    k,
                    Info {
                        integer,
                        ..Info::default()
                    },
                );
                let (t, f) = self.precompile_pair((then, ty), (otherwise, ty));
                size.wrapping_add(t).wrapping_add(f).wrapping_add(4)
            }
            ExprKind::Var { name, index } => {
                if ty == Type::None {
                    return 0;
                }
                self.precompile_ident(name);
                match index {
                    None => 3,
                    Some(i) => self.precompile(Node::Expr(i), Type::String).wrapping_add(6),
                }
            }
            ExprKind::Int(v) => {
                let index = match ty {
                    Type::None => return 0,
                    Type::String => {
                        self.number_string(false, u64::from(*v as u32), || v.to_string())
                    }
                    Type::Float => self.floats().add(f64::from(*v)),
                    Type::UInt => 0,
                };
                self.set_info(
                    k,
                    Info {
                        index,
                        ..Info::default()
                    },
                );
                2
            }
            ExprKind::Float(v) => {
                let index = match ty {
                    Type::None => return 0,
                    Type::String => self.number_string(true, v.to_bits(), || format_g(*v)),
                    Type::Float => self.floats().add(*v),
                    Type::UInt => 0,
                };
                self.set_info(
                    k,
                    Info {
                        index,
                        ..Info::default()
                    },
                );
                2
            }
            ExprKind::Str { text, tagged } => match ty {
                Type::None => 0,
                Type::String => {
                    let index = self.strings().add(text, true, *tagged);
                    self.set_info(
                        k,
                        Info {
                            index,
                            ..Info::default()
                        },
                    );
                    2
                }
                Type::UInt | Type::Float => {
                    self.precompile_number(k, e.line, text, ty);
                    2
                }
            },
            ExprKind::Ident(name) => match ty {
                Type::None => 0,
                Type::String => {
                    self.precompile_ident(name);
                    2
                }
                Type::UInt | Type::Float => {
                    self.precompile_number(k, e.line, name, ty);
                    2
                }
            },
            ExprKind::Assign { name, index, value } => {
                let mut sub = self.preferred(Node::Expr(value));
                if sub == Type::None {
                    sub = ty;
                }
                if sub == Type::None {
                    sub = Type::String;
                }
                self.set_info(
                    k,
                    Info {
                        index: sub as u32,
                        ..Info::default()
                    },
                );
                let add = u32::from(ty != sub);
                let size = self.precompile(Node::Expr(value), sub);
                self.precompile_ident(name);
                match index {
                    None => size.wrapping_add(add).wrapping_add(3),
                    Some(i) => {
                        let extra = if sub == Type::String { 8 } else { 6 };
                        self.precompile(Node::Expr(i), Type::String)
                            .wrapping_add(size)
                            .wrapping_add(add)
                            .wrapping_add(extra)
                    }
                }
            }
            ExprKind::AssignOp {
                name,
                index,
                op,
                value,
            } => {
                let (sub, _) = assign_operand(*op);
                self.precompile_ident(name);
                let mut size = self.precompile(Node::Expr(value), sub);
                if ty != sub {
                    size = size.wrapping_add(1);
                }
                match index {
                    None => size.wrapping_add(5),
                    Some(i) => size
                        .wrapping_add(self.precompile(Node::Expr(i), Type::String))
                        .wrapping_add(8),
                }
            }
            ExprKind::Call {
                namespace,
                name,
                args,
            } => {
                let mut size = u32::from(ty != Type::String);
                self.precompile_ident(name);
                if let Some(ns) = namespace {
                    self.precompile_ident(ns);
                }
                for a in args {
                    size = size
                        .wrapping_add(self.precompile(Node::Expr(a), Type::String))
                        .wrapping_add(1);
                }
                size.wrapping_add(5)
            }
            ExprKind::MethodCall { object, name, args } => {
                let mut size = u32::from(ty != Type::String);
                self.precompile_ident(name);
                for a in std::iter::once(&**object).chain(args) {
                    size = size
                        .wrapping_add(self.precompile(Node::Expr(a), Type::String))
                        .wrapping_add(1);
                }
                size.wrapping_add(5)
            }
            ExprKind::Field {
                object,
                name,
                index,
            } => {
                if ty == Type::None {
                    return 0;
                }
                self.precompile_ident(name);
                let mut size: u32 = 0;
                if let Some(i) = index {
                    size = self.precompile(Node::Expr(i), Type::String).wrapping_add(3);
                }
                self.precompile(Node::Expr(object), Type::String)
                    .wrapping_add(size)
                    .wrapping_add(4)
            }
            ExprKind::FieldAssign {
                object,
                name,
                index,
                value,
            } => {
                let mut size = u32::from(ty != Type::String);
                self.precompile_ident(name);
                size = size.wrapping_add(self.precompile(Node::Expr(value), Type::String));
                if let Some(o) = object {
                    size = size.wrapping_add(self.precompile(Node::Expr(o), Type::String));
                }
                match index {
                    None => size.wrapping_add(6),
                    Some(i) => self
                        .precompile(Node::Expr(i), Type::String)
                        .wrapping_add(4)
                        .wrapping_add(size)
                        .wrapping_add(5),
                }
            }
            ExprKind::FieldAssignOp {
                object,
                name,
                index,
                op,
                value,
            } => {
                let (sub, _) = assign_operand(*op);
                self.precompile_ident(name);
                let mut size = self.precompile(Node::Expr(value), sub);
                if ty != sub {
                    size = size.wrapping_add(1);
                }
                match index {
                    None => self
                        .precompile(Node::Expr(object), Type::String)
                        .wrapping_add(6)
                        .wrapping_add(size),
                    Some(i) => {
                        let (i, o) =
                            self.precompile_pair((i, Type::String), (object, Type::String));
                        i.wrapping_add(9).wrapping_add(o).wrapping_add(size)
                    }
                }
            }
            // `assert` and the internal-name operators are not in the classic grammar (its parser makes none): no
            // code.
            ExprKind::TagDeref(_) | ExprKind::InternalField { .. } | ExprKind::Assert { .. } => 0,
            ExprKind::New(decl) => {
                let size = self.precompile_object(decl).wrapping_add(2);
                size.wrapping_add(u32::from(ty != Type::UInt))
            }
        }
    }

    /// `||` of a case's tests: the first value's test and the `||` of the rest (right-nested, as
    /// `getSwitchOR` builds it), or the only value's test.
    fn case_or_parts(
        string: bool,
        test: &'a Expr,
        values: &'a [Expr],
    ) -> (Node<'a>, Option<Node<'a>>) {
        match values.split_first() {
            Some((first, rest)) => {
                let left = Node::CaseTest {
                    string,
                    test,
                    value: first,
                };
                if rest.is_empty() {
                    (left, None)
                } else {
                    (
                        left,
                        Some(Node::CaseOr {
                            string,
                            test,
                            values: rest,
                        }),
                    )
                }
            }
            None => (Node::Expr(test), None),
        }
    }

    /// The first pass over a node's two operands, in the generation's order (see the module documentation):
    /// their sizes, left then right.
    fn precompile_pair(&mut self, left: (&'a Expr, Type), right: (&'a Expr, Type)) -> (u32, u32) {
        self.precompile_nodes((Node::Expr(left.0), left.1), (Node::Expr(right.0), right.1))
    }

    fn precompile_nodes(&mut self, left: (Node<'a>, Type), right: (Node<'a>, Type)) -> (u32, u32) {
        if self.generation.right_operand_first {
            let r = self.precompile(right.0, right.1);
            let l = self.precompile(left.0, left.1);
            (l, r)
        } else {
            let l = self.precompile(left.0, left.1);
            let r = self.precompile(right.0, right.1);
            (l, r)
        }
    }

    fn precompile_int_binary(
        &mut self,
        op: BinaryOp,
        left: Node<'a>,
        right: Node<'a>,
        ty: Type,
    ) -> u32 {
        let (sub, operand) = int_operand(op);
        let (l, r) = self.precompile_nodes((left, sub), (right, sub));
        let mut size = r.wrapping_add(1).wrapping_add(l);
        if operand == Op::Or || operand == Op::And {
            size = size.wrapping_add(1);
        }
        size.wrapping_add(u32::from(ty != Type::UInt))
    }

    fn precompile_streq(&mut self, left: &'a Expr, right: &'a Expr, equal: bool, ty: Type) -> u32 {
        let (l, r) = self.precompile_pair((left, Type::String), (right, Type::String));
        r.wrapping_add(l)
            .wrapping_add(2)
            .wrapping_add(u32::from(!equal))
            .wrapping_add(u32::from(ty != Type::UInt))
    }

    /// The number of a string or identifier used as a number (`fVal`), and its float-table entry.
    fn precompile_number(&mut self, k: Key, line: u32, text: &[u8], ty: Type) {
        let (fval, warn) = string_to_number(text, self.generation.atof);
        if warn && self.file.is_some() {
            self.warn(line, "string always evaluates to 0.");
        }
        let index = if ty == Type::Float {
            self.floats().add(fval)
        } else {
            0
        };
        self.set_info(
            k,
            Info {
                index,
                fval,
                ..Info::default()
            },
        );
    }

    fn precompile_object(&mut self, decl: &'a ObjectDecl) -> u32 {
        self.precompile_ident(decl.parent.as_deref().unwrap_or(b""));
        let mut args: u32 = 0;
        for a in &decl.args {
            args = args
                .wrapping_add(self.precompile(Node::Expr(a), Type::String))
                .wrapping_add(1);
        }
        args = args
            .wrapping_add(self.precompile(Node::Expr(&decl.class), Type::String))
            .wrapping_add(1);
        let name = self.precompile(Node::Expr(&decl.name), Type::String);
        let mut slots: u32 = 0;
        for f in &decl.fields {
            slots = slots.wrapping_add(self.precompile(Node::Expr(f), Type::None));
        }
        let mut subs: u32 = 0;
        for c in &decl.children {
            subs = subs.wrapping_add(self.precompile_object(c));
        }
        let fail = 10u32
            .wrapping_add(name)
            .wrapping_add(args)
            .wrapping_add(slots)
            .wrapping_add(subs);
        self.set_info(
            key(decl),
            Info {
                a: fail,
                ..Info::default()
            },
        );
        fail
    }

    // ---------------------------------------------------------------- compile

    fn emit(&mut self, ip: &mut u32, word: u32) {
        match self.code.get_mut(*ip as usize) {
            Some(slot) => *slot = word,
            None => self.overflow = self.overflow.saturating_add(1),
        }
        *ip = ip.wrapping_add(1);
    }

    fn emit_op(&mut self, ip: &mut u32, op: Op) {
        self.emit(ip, self.dialect.opcodes.code(op));
    }

    /// `STEtoU32` in its DSO-writing form: an identifier slot holds 0 and the identifier table records it;
    /// the name's offset is the first global-table entry equal to it ignoring case.
    fn emit_ident(&mut self, ip: &mut u32, name: Option<&[u8]>) {
        if let Some(name) = name {
            let offset = self.global_strings.add(name, false, false);
            self.idents.add(offset, *ip);
        }
        self.emit(ip, 0);
    }

    pub(crate) fn compile_block(&mut self, stmts: &'a [Stmt], ip: u32, cont: u32, brk: u32) -> u32 {
        let mut ip = ip;
        for s in stmts {
            ip = self.compile_stmt(StmtRef::Stmt(s), ip, cont, brk);
        }
        ip
    }

    fn compile_stmt(&mut self, stmt: StmtRef<'a>, ip: u32, cont: u32, brk: u32) -> u32 {
        if !self.visit() {
            return ip;
        }
        let s = match stmt {
            StmtRef::Stmt(s) => s,
            StmtRef::Case { .. } => return self.compile_if(stmt, ip, cont, brk),
        };
        let mut ip = ip;
        match &s.kind {
            StmtKind::Break | StmtKind::Continue => {
                let target = if matches!(s.kind, StmtKind::Break) {
                    brk
                } else {
                    cont
                };
                if target != 0 {
                    self.add_break_line(s.line, ip);
                    self.emit_op(&mut ip, Op::Jmp);
                    self.emit(&mut ip, target);
                }
                ip
            }
            StmtKind::Expr(e) => {
                self.add_break_line(s.line, ip);
                self.compile(Node::Expr(e), ip, Type::None)
            }
            StmtKind::Return(e) => {
                self.add_break_line(s.line, ip);
                if let Some(e) = e {
                    ip = self.compile(Node::Expr(e), ip, Type::String);
                }
                self.emit_op(&mut ip, Op::Return);
                ip
            }
            StmtKind::If { .. } | StmtKind::Switch { .. } => self.compile_if(stmt, ip, cont, brk),
            StmtKind::Loop {
                init,
                test,
                step,
                body,
                post_test,
            } => {
                let info = self.info(key(s));
                self.add_break_line(s.line, ip);
                let start = ip;
                if let Some(init) = init {
                    ip = self.compile(Node::Expr(init), ip, Type::None);
                }
                let ty = if info.integer {
                    Type::UInt
                } else {
                    Type::Float
                };
                if !*post_test {
                    ip = self.compile(Node::Expr(test), ip, ty);
                    self.emit_op(
                        &mut ip,
                        if info.integer {
                            Op::JmpIfNot
                        } else {
                            Op::JmpIfFNot
                        },
                    );
                    self.emit(&mut ip, start.wrapping_add(info.c));
                }
                ip = self.compile_block(
                    body,
                    ip,
                    start.wrapping_add(info.b),
                    start.wrapping_add(info.c),
                );
                if let Some(step) = step {
                    ip = self.compile(Node::Expr(step), ip, Type::None);
                }
                ip = self.compile(Node::Expr(test), ip, ty);
                self.emit_op(&mut ip, if info.integer { Op::JmpIf } else { Op::JmpIfF });
                self.emit(&mut ip, start.wrapping_add(info.a));
                ip
            }
            StmtKind::Foreach { .. } => ip,
            StmtKind::Function(f) => self.compile_function(f, None, ip),
            StmtKind::Package { name, functions } => {
                for f in functions {
                    ip = self.compile_function(f, Some(name), ip);
                }
                ip
            }
        }
    }

    fn compile_if_block(&mut self, block: IfBlock<'a>, ip: u32, cont: u32, brk: u32) -> u32 {
        match block {
            IfBlock::Stmts(stmts) => self.compile_block(stmts, ip, cont, brk),
            IfBlock::Case(case) => self.compile_stmt(case, ip, cont, brk),
        }
    }

    fn compile_if(&mut self, stmt: StmtRef<'a>, ip: u32, cont: u32, brk: u32) -> u32 {
        let Some((test, then, otherwise, k)) = Self::if_parts(stmt) else {
            return ip;
        };
        let info = self.info(k);
        let line = match stmt {
            StmtRef::Stmt(s) => match &s.kind {
                StmtKind::Switch { cases, .. } => cases.first().map_or(s.line, |c| c.line),
                _ => s.line,
            },
            StmtRef::Case { cases, .. } => cases.first().map_or(0, |c| c.line),
        };
        let start = ip;
        let mut ip = ip;
        self.add_break_line(line, ip);
        ip = self.compile(
            test,
            ip,
            if info.integer {
                Type::UInt
            } else {
                Type::Float
            },
        );
        self.emit_op(
            &mut ip,
            if info.integer {
                Op::JmpIfNot
            } else {
                Op::JmpIfFNot
            },
        );
        match otherwise {
            Some(block) => {
                self.emit(&mut ip, start.wrapping_add(info.a));
                ip = self.compile_if_block(then, ip, cont, brk);
                self.emit_op(&mut ip, Op::Jmp);
                self.emit(&mut ip, start.wrapping_add(info.b));
                ip = self.compile_if_block(block, ip, cont, brk);
            }
            None => {
                self.emit(&mut ip, start.wrapping_add(info.b));
                ip = self.compile_if_block(then, ip, cont, brk);
            }
        }
        ip
    }

    fn compile_function(&mut self, f: &'a Function, package: Option<&'a [u8]>, ip: u32) -> u32 {
        let info = self.info(key(f));
        let start = ip;
        let mut ip = ip;
        self.emit_op(&mut ip, Op::FuncDecl);
        self.emit_ident(&mut ip, Some(&f.name));
        self.emit_ident(&mut ip, f.namespace.as_deref());
        self.emit_ident(&mut ip, package);
        self.emit(&mut ip, u32::from(f.body.is_some()));
        self.emit(&mut ip, start.wrapping_add(info.a));
        self.emit(&mut ip, u32::try_from(f.params.len()).unwrap_or(u32::MAX));
        for p in &f.params {
            self.emit_ident(&mut ip, Some(&p.name));
        }
        self.in_function = true;
        if let Some(body) = f.body.as_deref() {
            ip = self.compile_block(body, ip, 0, 0);
        }
        self.emit_op(&mut ip, Op::Return);
        self.in_function = false;
        ip
    }

    fn compile(&mut self, node: Node<'a>, ip: u32, ty: Type) -> u32 {
        if !self.visit() {
            return ip;
        }
        let e = match node {
            Node::Expr(e) => e,
            Node::CaseTest {
                string,
                test,
                value,
            } => {
                return if string {
                    self.compile_streq(test, value, true, ip, ty)
                } else {
                    self.compile_int_binary(
                        BinaryOp::Eq,
                        Node::Expr(test),
                        Node::Expr(value),
                        ip,
                        ty,
                    )
                };
            }
            Node::CaseOr {
                string,
                test,
                values,
            } => {
                let (left, right) = Self::case_or_parts(string, test, values);
                return match right {
                    None => self.compile(left, ip, ty),
                    Some(right) => self.compile_int_binary(BinaryOp::Or, left, right, ip, ty),
                };
            }
        };
        let k = key(e);
        let mut ip = ip;
        match &e.kind {
            ExprKind::Binary { op, left, right } if is_float_op(*op) => {
                ip = self.compile(Node::Expr(right), ip, Type::Float);
                ip = self.compile(Node::Expr(left), ip, Type::Float);
                self.emit_op(&mut ip, int_operand(*op).1);
                if ty != Type::Float {
                    self.emit_op(&mut ip, conversion(Type::Float, ty));
                }
            }
            ExprKind::Binary { op, left, right } => {
                ip = self.compile_int_binary(*op, Node::Expr(left), Node::Expr(right), ip, ty);
            }
            ExprKind::StrEq { equal, left, right } => {
                ip = self.compile_streq(left, right, *equal, ip, ty)
            }
            ExprKind::Concat { kind, left, right } => {
                ip = self.compile(Node::Expr(left), ip, Type::String);
                match kind {
                    Concat::Plain => self.emit_op(&mut ip, Op::AdvanceStr),
                    _ => {
                        self.emit_op(&mut ip, Op::AdvanceStrAppendChar);
                        let ch = match kind {
                            Concat::Space => b' ',
                            Concat::Tab => b'\t',
                            _ => b'\n',
                        };
                        self.emit(&mut ip, u32::from(ch));
                    }
                }
                ip = self.compile(Node::Expr(right), ip, Type::String);
                self.emit_op(&mut ip, Op::RewindStr);
                match ty {
                    Type::UInt => self.emit_op(&mut ip, Op::StrToUint),
                    Type::Float => self.emit_op(&mut ip, Op::StrToFlt),
                    Type::None | Type::String => {}
                }
            }
            ExprKind::CommaCat { left, right } => {
                ip = self.compile(Node::Expr(left), ip, Type::String);
                self.emit_op(&mut ip, Op::AdvanceStrComma);
                ip = self.compile(Node::Expr(right), ip, Type::String);
                self.emit_op(&mut ip, Op::RewindStr);
                if ty == Type::UInt || ty == Type::Float {
                    self.warn(
                        e.line,
                        "converting comma string to a number... probably wrong.",
                    );
                }
                match ty {
                    Type::UInt => self.emit_op(&mut ip, Op::StrToUint),
                    Type::Float => self.emit_op(&mut ip, Op::StrToFlt),
                    Type::None | Type::String => {}
                }
            }
            ExprKind::Unary {
                op: UnaryOp::Neg,
                expr,
            } => {
                ip = self.compile(Node::Expr(expr), ip, Type::Float);
                self.emit_op(&mut ip, Op::Neg);
                if ty != Type::Float {
                    self.emit_op(&mut ip, conversion(Type::Float, ty));
                }
            }
            ExprKind::Unary { op, expr } => {
                let integer = self.info(k).integer;
                ip = self.compile(
                    Node::Expr(expr),
                    ip,
                    if integer { Type::UInt } else { Type::Float },
                );
                if *op == UnaryOp::Not {
                    self.emit_op(&mut ip, if integer { Op::Not } else { Op::NotF });
                } else {
                    self.emit_op(&mut ip, Op::OnesComplement);
                }
                if ty != Type::UInt {
                    self.emit_op(&mut ip, conversion(Type::UInt, ty));
                }
            }
            ExprKind::Ternary {
                test,
                then,
                otherwise,
            } => {
                let integer = self.info(k).integer;
                ip = self.compile(
                    Node::Expr(test),
                    ip,
                    if integer { Type::UInt } else { Type::Float },
                );
                self.emit_op(&mut ip, if integer { Op::JmpIfNot } else { Op::JmpIfFNot });
                let jump_else = ip;
                self.emit(&mut ip, 0);
                ip = self.compile(Node::Expr(then), ip, ty);
                self.emit_op(&mut ip, Op::Jmp);
                let jump_end = ip;
                self.emit(&mut ip, 0);
                self.patch(jump_else, ip);
                ip = self.compile(Node::Expr(otherwise), ip, ty);
                self.patch(jump_end, ip);
            }
            ExprKind::Var { name, index } => {
                if ty == Type::None {
                    return ip;
                }
                self.emit_op(
                    &mut ip,
                    if index.is_some() {
                        Op::LoadImmedIdent
                    } else {
                        Op::SetCurVar
                    },
                );
                self.emit_ident(&mut ip, Some(name));
                if let Some(i) = index {
                    self.emit_op(&mut ip, Op::AdvanceStr);
                    ip = self.compile(Node::Expr(i), ip, Type::String);
                    self.emit_op(&mut ip, Op::RewindStr);
                    self.emit_op(&mut ip, Op::SetCurVarArray);
                }
                match ty {
                    Type::UInt => self.emit_op(&mut ip, Op::LoadVarUint),
                    Type::Float => self.emit_op(&mut ip, Op::LoadVarFlt),
                    Type::String => self.emit_op(&mut ip, Op::LoadVarStr),
                    Type::None => {}
                }
            }
            ExprKind::Int(v) => {
                let info = self.info(k);
                match ty {
                    Type::UInt => {
                        self.emit_op(&mut ip, Op::LoadImmedUint);
                        self.emit(&mut ip, *v as u32);
                    }
                    Type::String => {
                        self.emit_op(&mut ip, Op::LoadImmedStr);
                        self.emit(&mut ip, info.index);
                    }
                    Type::Float => {
                        self.emit_op(&mut ip, Op::LoadImmedFlt);
                        self.emit(&mut ip, info.index);
                    }
                    Type::None => {}
                }
            }
            ExprKind::Float(v) => {
                let info = self.info(k);
                match ty {
                    Type::UInt => {
                        self.emit_op(&mut ip, Op::LoadImmedUint);
                        self.emit(&mut ip, f64_to_u32(*v));
                    }
                    Type::String => {
                        self.emit_op(&mut ip, Op::LoadImmedStr);
                        self.emit(&mut ip, info.index);
                    }
                    Type::Float => {
                        self.emit_op(&mut ip, Op::LoadImmedFlt);
                        self.emit(&mut ip, info.index);
                    }
                    Type::None => {}
                }
            }
            ExprKind::Str { tagged, .. } => {
                let info = self.info(k);
                match ty {
                    Type::String => {
                        self.emit_op(
                            &mut ip,
                            if *tagged {
                                Op::TagToStr
                            } else {
                                Op::LoadImmedStr
                            },
                        );
                        self.emit(&mut ip, info.index);
                    }
                    Type::UInt => {
                        self.emit_op(&mut ip, Op::LoadImmedUint);
                        self.emit(&mut ip, f64_to_u32(info.fval));
                    }
                    Type::Float => {
                        self.emit_op(&mut ip, Op::LoadImmedFlt);
                        self.emit(&mut ip, info.index);
                    }
                    Type::None => {}
                }
            }
            ExprKind::Ident(name) => {
                let info = self.info(k);
                match ty {
                    Type::String => {
                        self.emit_op(&mut ip, Op::LoadImmedIdent);
                        self.emit_ident(&mut ip, Some(name));
                    }
                    Type::UInt => {
                        self.emit_op(&mut ip, Op::LoadImmedUint);
                        self.emit(&mut ip, f64_to_u32(info.fval));
                    }
                    Type::Float => {
                        self.emit_op(&mut ip, Op::LoadImmedFlt);
                        self.emit(&mut ip, info.index);
                    }
                    Type::None => {}
                }
            }
            ExprKind::Assign { name, index, value } => {
                let sub = match self.info(k).index {
                    1 => Type::UInt,
                    2 => Type::Float,
                    _ => Type::String,
                };
                ip = self.compile(Node::Expr(value), ip, sub);
                match index {
                    Some(i) => {
                        if sub == Type::String {
                            self.emit_op(&mut ip, Op::AdvanceStr);
                        }
                        self.emit_op(&mut ip, Op::LoadImmedIdent);
                        self.emit_ident(&mut ip, Some(name));
                        self.emit_op(&mut ip, Op::AdvanceStr);
                        ip = self.compile(Node::Expr(i), ip, Type::String);
                        self.emit_op(&mut ip, Op::RewindStr);
                        self.emit_op(&mut ip, Op::SetCurVarArrayCreate);
                        if sub == Type::String {
                            self.emit_op(&mut ip, Op::TerminateRewindStr);
                        }
                    }
                    None => {
                        self.emit_op(&mut ip, Op::SetCurVarCreate);
                        self.emit_ident(&mut ip, Some(name));
                    }
                }
                self.emit_op(
                    &mut ip,
                    match sub {
                        Type::UInt => Op::SaveVarUint,
                        Type::Float => Op::SaveVarFlt,
                        _ => Op::SaveVarStr,
                    },
                );
                if ty != sub {
                    self.emit_op(&mut ip, conversion(sub, ty));
                }
            }
            ExprKind::AssignOp {
                name,
                index,
                op,
                value,
            } => {
                let (sub, operand) = assign_operand(*op);
                ip = self.compile(Node::Expr(value), ip, sub);
                match index {
                    None => {
                        self.emit_op(&mut ip, Op::SetCurVarCreate);
                        self.emit_ident(&mut ip, Some(name));
                    }
                    Some(i) => {
                        self.emit_op(&mut ip, Op::LoadImmedIdent);
                        self.emit_ident(&mut ip, Some(name));
                        self.emit_op(&mut ip, Op::AdvanceStr);
                        ip = self.compile(Node::Expr(i), ip, Type::String);
                        self.emit_op(&mut ip, Op::RewindStr);
                        self.emit_op(&mut ip, Op::SetCurVarArrayCreate);
                    }
                }
                let float = sub == Type::Float;
                self.emit_op(
                    &mut ip,
                    if float {
                        Op::LoadVarFlt
                    } else {
                        Op::LoadVarUint
                    },
                );
                self.emit_op(&mut ip, operand);
                self.emit_op(
                    &mut ip,
                    if float {
                        Op::SaveVarFlt
                    } else {
                        Op::SaveVarUint
                    },
                );
                if sub != ty {
                    self.emit_op(&mut ip, conversion(sub, ty));
                }
            }
            ExprKind::Call {
                namespace,
                name,
                args,
            } => {
                let parent = namespace
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(b"Parent"));
                ip = self.compile_call(
                    args.iter(),
                    name,
                    namespace.as_deref(),
                    if parent { 2 } else { 0 },
                    ip,
                    ty,
                );
            }
            ExprKind::MethodCall { object, name, args } => {
                ip = self.compile_call(
                    std::iter::once(&**object).chain(args),
                    name,
                    None,
                    1,
                    ip,
                    ty,
                );
            }
            ExprKind::Field {
                object,
                name,
                index,
            } => {
                if ty == Type::None {
                    return ip;
                }
                if let Some(i) = index {
                    ip = self.compile(Node::Expr(i), ip, Type::String);
                    self.emit_op(&mut ip, Op::AdvanceStr);
                }
                ip = self.compile(Node::Expr(object), ip, Type::String);
                self.emit_op(&mut ip, Op::SetCurObject);
                self.emit_op(&mut ip, Op::SetCurField);
                self.emit_ident(&mut ip, Some(name));
                if index.is_some() {
                    self.emit_op(&mut ip, Op::TerminateRewindStr);
                    self.emit_op(&mut ip, Op::SetCurFieldArray);
                }
                match ty {
                    Type::UInt => self.emit_op(&mut ip, Op::LoadFieldUint),
                    Type::Float => self.emit_op(&mut ip, Op::LoadFieldFlt),
                    Type::String => self.emit_op(&mut ip, Op::LoadFieldStr),
                    Type::None => {}
                }
            }
            ExprKind::FieldAssign {
                object,
                name,
                index,
                value,
            } => {
                ip = self.compile(Node::Expr(value), ip, Type::String);
                self.emit_op(&mut ip, Op::AdvanceStr);
                if let Some(i) = index {
                    ip = self.compile(Node::Expr(i), ip, Type::String);
                    self.emit_op(&mut ip, Op::AdvanceStr);
                }
                match object {
                    Some(o) => {
                        ip = self.compile(Node::Expr(o), ip, Type::String);
                        self.emit_op(&mut ip, Op::SetCurObject);
                    }
                    None => self.emit_op(&mut ip, Op::SetCurObjectNew),
                }
                self.emit_op(&mut ip, Op::SetCurField);
                self.emit_ident(&mut ip, Some(name));
                if index.is_some() {
                    self.emit_op(&mut ip, Op::TerminateRewindStr);
                    self.emit_op(&mut ip, Op::SetCurFieldArray);
                }
                self.emit_op(&mut ip, Op::TerminateRewindStr);
                self.emit_op(&mut ip, Op::SaveFieldStr);
                if ty != Type::String {
                    self.emit_op(&mut ip, conversion(Type::String, ty));
                }
            }
            ExprKind::FieldAssignOp {
                object,
                name,
                index,
                op,
                value,
            } => {
                let (sub, operand) = assign_operand(*op);
                ip = self.compile(Node::Expr(value), ip, sub);
                if let Some(i) = index {
                    ip = self.compile(Node::Expr(i), ip, Type::String);
                    self.emit_op(&mut ip, Op::AdvanceStr);
                }
                ip = self.compile(Node::Expr(object), ip, Type::String);
                self.emit_op(&mut ip, Op::SetCurObject);
                self.emit_op(&mut ip, Op::SetCurField);
                self.emit_ident(&mut ip, Some(name));
                if index.is_some() {
                    self.emit_op(&mut ip, Op::TerminateRewindStr);
                    self.emit_op(&mut ip, Op::SetCurFieldArray);
                }
                let float = sub == Type::Float;
                self.emit_op(
                    &mut ip,
                    if float {
                        Op::LoadFieldFlt
                    } else {
                        Op::LoadFieldUint
                    },
                );
                self.emit_op(&mut ip, operand);
                self.emit_op(
                    &mut ip,
                    if float {
                        Op::SaveFieldFlt
                    } else {
                        Op::SaveFieldUint
                    },
                );
                if sub != ty {
                    self.emit_op(&mut ip, conversion(sub, ty));
                }
            }
            ExprKind::TagDeref(_) | ExprKind::InternalField { .. } | ExprKind::Assert { .. } => {}
            ExprKind::New(decl) => {
                self.emit_op(&mut ip, Op::LoadImmedUint);
                self.emit(&mut ip, 0);
                ip = self.compile_object(decl, ip, true);
                if ty != Type::UInt {
                    self.emit_op(&mut ip, conversion(Type::UInt, ty));
                }
            }
        }
        ip
    }

    fn patch(&mut self, at: u32, value: u32) {
        if let Some(slot) = self.code.get_mut(at as usize) {
            *slot = value;
        }
    }

    fn compile_call(
        &mut self,
        args: impl Iterator<Item = &'a Expr>,
        name: &'a [u8],
        namespace: Option<&'a [u8]>,
        call_type: u32,
        ip: u32,
        ty: Type,
    ) -> u32 {
        let mut ip = ip;
        self.emit_op(&mut ip, Op::PushFrame);
        for a in args {
            ip = self.compile(Node::Expr(a), ip, Type::String);
            self.emit_op(&mut ip, Op::Push);
        }
        self.emit_op(
            &mut ip,
            if call_type == 0 {
                Op::CallFuncResolve
            } else {
                Op::CallFunc
            },
        );
        self.emit_ident(&mut ip, Some(name));
        self.emit_ident(&mut ip, namespace);
        self.emit(&mut ip, call_type);
        if ty != Type::String {
            self.emit_op(&mut ip, conversion(Type::String, ty));
        }
        ip
    }

    fn compile_int_binary(
        &mut self,
        op: BinaryOp,
        left: Node<'a>,
        right: Node<'a>,
        ip: u32,
        ty: Type,
    ) -> u32 {
        let (sub, operand) = int_operand(op);
        let mut ip = ip;
        if operand == Op::Or || operand == Op::And {
            ip = self.compile(left, ip, sub);
            self.emit_op(
                &mut ip,
                if operand == Op::Or {
                    Op::JmpIfNp
                } else {
                    Op::JmpIfNotNp
                },
            );
            let jump = ip;
            self.emit(&mut ip, 0);
            ip = self.compile(right, ip, sub);
            self.patch(jump, ip);
        } else {
            ip = self.compile(right, ip, sub);
            ip = self.compile(left, ip, sub);
            self.emit_op(&mut ip, operand);
        }
        if ty != Type::UInt {
            self.emit_op(&mut ip, conversion(Type::UInt, ty));
        }
        ip
    }

    fn compile_streq(
        &mut self,
        left: &'a Expr,
        right: &'a Expr,
        equal: bool,
        ip: u32,
        ty: Type,
    ) -> u32 {
        let mut ip = self.compile(Node::Expr(left), ip, Type::String);
        self.emit_op(&mut ip, Op::AdvanceStrNul);
        ip = self.compile(Node::Expr(right), ip, Type::String);
        self.emit_op(&mut ip, Op::CompareStr);
        if !equal {
            self.emit_op(&mut ip, Op::Not);
        }
        if ty != Type::UInt {
            self.emit_op(&mut ip, conversion(Type::UInt, ty));
        }
        ip
    }

    fn compile_object(&mut self, decl: &'a ObjectDecl, ip: u32, root: bool) -> u32 {
        let fail = self.info(key(decl)).a;
        let start = ip;
        let mut ip = ip;
        self.emit_op(&mut ip, Op::PushFrame);
        ip = self.compile(Node::Expr(&decl.class), ip, Type::String);
        self.emit_op(&mut ip, Op::Push);
        ip = self.compile(Node::Expr(&decl.name), ip, Type::String);
        self.emit_op(&mut ip, Op::Push);
        for a in &decl.args {
            ip = self.compile(Node::Expr(a), ip, Type::String);
            self.emit_op(&mut ip, Op::Push);
        }
        self.emit_op(&mut ip, Op::CreateObject);
        self.emit_ident(&mut ip, Some(decl.parent.as_deref().unwrap_or(b"")));
        self.emit(&mut ip, u32::from(decl.datablock));
        self.emit(&mut ip, start.wrapping_add(fail));
        for f in &decl.fields {
            ip = self.compile(Node::Expr(f), ip, Type::None);
        }
        self.emit_op(&mut ip, Op::AddObject);
        self.emit(&mut ip, u32::from(root));
        for c in &decl.children {
            ip = self.compile_object(c, ip, false);
        }
        self.emit_op(&mut ip, Op::EndObject);
        self.emit(&mut ip, u32::from(root || decl.datablock));
        ip
    }

    // ---------------------------------------------------------------- the file

    /// Both passes over a file's statements and the DSO they make (`CodeBlock::compile`): the code is the
    /// precompiled size plus the final `OP_RETURN`; the line-break table holds what the second pass recorded.
    /// `Err` (with the messages) when the file is beyond [`MAX_VISITS`] or [`MAX_CODE_WORDS`].
    pub(crate) fn generate(
        mut self,
        script: &'a Script,
    ) -> (Result<DsoFile, ()>, Vec<Diagnostic>, u64, usize) {
        let size = self.precompile_block(&script.stmts, 0).wrapping_add(1);
        if self.visits > MAX_VISITS || size > MAX_CODE_WORDS {
            self.diagnostics.push(Diagnostic {
                level: Level::Error,
                text: b"the script is too large to compile".to_vec(),
            });
            return (Err(()), self.diagnostics, self.visits, 0);
        }
        self.code = vec![gap_word(self.dialect); size as usize];
        let last = self.compile_block(&script.stmts, 0, 0, 0);
        let mut ip = last;
        self.emit_op(&mut ip, Op::Return);
        if self.visits > MAX_VISITS {
            self.diagnostics.push(Diagnostic {
                level: Level::Error,
                text: b"the script is too large to compile".to_vec(),
            });
            return (Err(()), self.diagnostics, self.visits, 0);
        }
        if ip != size || self.overflow > 0 {
            self.diagnostics.push(Diagnostic {
                level: Level::Warning,
                text: RUN_MISMATCH.to_vec(),
            });
        }
        let encode = self.dialect.strings.encode;
        let dso = DsoFile {
            version: self.dialect.dso_version,
            global_strings: encode(self.global_strings.bytes()),
            global_floats: self.global_floats.values().to_vec(),
            function_strings: encode(self.function_strings.bytes()),
            function_floats: self.function_floats.values().to_vec(),
            local_variables: Vec::new(),
            code: self.code,
            line_breaks: self.line_breaks,
            identifiers: self.idents.entries(),
        };
        // Where the written code ends: the words from there to the end of the code are [`gap_word`]s of a size
        // mismatch (none when the second pass wrote the precompiled size or more).
        let code_end = (ip as usize).min(dso.code.len());
        (Ok(dso), self.diagnostics, self.visits, code_end)
    }
}

#[derive(Clone, Copy)]
enum IfBlock<'a> {
    Stmts(&'a [Stmt]),
    Case(StmtRef<'a>),
}

#[cfg(test)]
#[path = "codegen_tests.rs"]
mod tests;
