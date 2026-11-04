//! The code generator: one pass over the tree, node by node, writing Torque3D's instruction set (the
//! `compile`/`compileStmt` methods of `astNodes.cpp`; see [`torque_ts_bytecode::dialect`] for the Torque3D
//! version this follows). The code words come from the dialect's [`OpcodeTable`].
//!
//! * The string and float tables fill in the order the nodes are compiled; a binary arithmetic or comparison
//!   node compiles its right operand first, a string comparison or concatenation its left one.
//! * Names (functions, namespaces, global variables, fields, classes) go into the global string table
//!   case-sensitively when a node first uses them, and every slot that names one takes two code words (the
//!   loader stores a pointer there) and an entry in the identifier table, found again ignoring case.
//! * Local variables (`%x` without an index) live in registers: each function numbers its locals in the order
//!   it first meets them (its parameters first), and the file's local-variable table lists each function's
//!   names by register. A local's type is the type of what was assigned to it, or none when assignments
//!   disagree; reading one never assigned warns and makes it a string register. Locals at the top level use
//!   the file's own registers (with a warning, in a file).
//! * Every statement records its line and first code word in the line-break table.
//!
//! Two places differ from Torque3D's compiler on purpose, where its code would be wrong: `case a or b`
//! compares the switch value with every listed value (Torque3D's compares it with the first one each time),
//! and the jump of `&&`/`||` always lands after the whole right operand (Torque3D's lands inside an assignment
//! used as the right operand).
//!
//! Follows Torque3D's `astNodes.cpp`, `optimizer.cpp` and `compiledEval.cpp` (MIT); the credits are in the crate documentation.

use std::collections::HashMap;

use torque_ts_bytecode::opcodes::{Op, OpcodeTable};
use torque_ts_bytecode::{Dialect, DsoFile, LocalVariables};
use torque_ts_text::{atof, f64_to_u32, format_g, to_f32, to_f64};

use crate::ast::{
    AssignOp, BinaryOp, Case, Concat, Expr, ExprKind, Function, ObjectDecl, Script, Stmt, StmtKind,
    UnaryOp,
};
use crate::tables::{FloatTable, IdentTable, StringTable};
use crate::{Diagnostic, Level};

/// `TypeReq`: what a node's value is wanted as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Type {
    None,
    UInt,
    Float,
    String,
}

/// The `callType` operand of `OP_CALLFUNC`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
enum CallType {
    Function = 0,
    Static = 1,
    Method = 2,
    Parent = 3,
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
        AssignOp::Increment | AssignOp::Add => (Type::Float, Op::Add),
        AssignOp::Decrement | AssignOp::Sub => (Type::Float, Op::Sub),
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

/// `consoleStringToNumber` (as used by `StrConstNode` and `ConstantNode`): `dAtof` (`strtof`), widened;
/// `true`/`false` (any case) when that is 0; else 0 and a warning when the node has a file.
fn string_to_number(text: &[u8]) -> (f64, bool) {
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

/// A variable is a global-style one (`$x`, or any variable with an index) or a local in a register.
fn is_register_variable(name: &[u8], index: Option<&Expr>) -> bool {
    index.is_none() && name.first() != Some(&b'$')
}

/// An expression node, or one of the nodes the generator makes: the tests of a `case`, and the mask of a
/// remainder made a bitwise and.
#[derive(Clone, Copy)]
enum Node<'s> {
    Expr(&'s Expr),
    /// `test == value` or `test $= value` for one case value.
    CaseTest {
        string: bool,
        test: &'s Expr,
        value: &'s Expr,
    },
    /// The `||` of a case's tests: of `values[..n - 1]`, then of the last value.
    CaseOr {
        string: bool,
        test: &'s Expr,
        values: &'s [Expr],
    },
    Int(i32),
}

/// A statement node: a statement, or the `if` a `case` becomes.
#[derive(Clone, Copy)]
enum StmtRef<'s> {
    Stmt(&'s Stmt),
    /// `cases[0]` of a switch, its `else` the rest of the chain or the default.
    Case {
        string: bool,
        test: &'s Expr,
        cases: &'s [Case],
        default: Option<&'s [Stmt]>,
    },
}

#[derive(Clone, Copy)]
enum Block<'s> {
    Stmts(&'s [Stmt]),
    Case(StmtRef<'s>),
}

/// What a loop's placeholder words are patched with when the loop ends (`CodeStream::FixType`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fix {
    LoopBlockStart,
    Break,
    Continue,
}

/// One function's (or the top level's) local variables: name, ignoring case -> register and type.
#[derive(Default)]
struct Locals {
    registers: HashMap<Vec<u8>, (u32, Type)>,
    /// By register.
    names: Vec<Vec<u8>>,
}

impl Locals {
    /// `FuncVars::assign`: the variable's register (a new one the first time). A type that disagrees with
    /// an earlier one makes the variable's type none.
    fn assign(&mut self, name: &[u8], ty: Type) -> u32 {
        let key = name.to_ascii_lowercase();
        if let Some((reg, current)) = self.registers.get_mut(&key) {
            if ty != *current && *current != Type::None {
                *current = Type::None;
            }
            return *reg;
        }
        let reg = u32::try_from(self.names.len()).unwrap_or(u32::MAX);
        self.registers.insert(key, (reg, ty));
        self.names.push(name.to_vec());
        reg
    }

    fn find(&self, name: &[u8]) -> Option<(u32, Type)> {
        self.registers.get(&name.to_ascii_lowercase()).copied()
    }

    fn count(&self) -> u32 {
        u32::try_from(self.names.len()).unwrap_or(u32::MAX)
    }
}

/// The most node visits one file may cost: about 64 million, a code of several hundred MiB.
pub const MAX_VISITS: u64 = 1 << 26;
/// The largest code one file may have, in words.
pub const MAX_CODE_WORDS: u32 = 1 << 24;

pub(crate) struct Gen<'a> {
    file: Option<&'a [u8]>,
    source: &'a [u8],
    opcodes: &'a OpcodeTable,
    global_strings: StringTable,
    function_strings: StringTable,
    global_floats: FloatTable,
    function_floats: FloatTable,
    /// The current tables are the function tables (inside a function declaration).
    function_tables: bool,
    idents: IdentTable,
    code: Vec<u32>,
    line_breaks: Vec<u32>,
    /// The fixes of each loop being compiled, innermost last.
    loops: Vec<Vec<(u32, Fix)>>,
    /// The function being compiled has its own locals; else the top level's are used.
    function_locals: Option<Locals>,
    top_locals: Locals,
    /// The local-variable table, and each function key's entry in it.
    local_tables: Vec<LocalVariables>,
    local_table_index: HashMap<Vec<u8>, usize>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    /// Code words beyond [`MAX_CODE_WORDS`]: not stored.
    overflow: bool,
    /// Nodes visited: a `switch` compiles its tested expression once per case value, so a small file can make
    /// a large program; past [`MAX_VISITS`] the generator gives up.
    visits: u64,
    /// The string-table offsets of numbers converted to text, by (function tables, float, value bits).
    number_strings: HashMap<(bool, bool, u64), u32>,
}

impl<'a> Gen<'a> {
    pub(crate) fn new(file: Option<&'a [u8]>, source: &'a [u8], dialect: &'a Dialect) -> Gen<'a> {
        Gen {
            file,
            source,
            opcodes: &dialect.opcodes,
            global_strings: StringTable::default(),
            function_strings: StringTable::default(),
            global_floats: FloatTable::default(),
            function_floats: FloatTable::default(),
            function_tables: false,
            idents: IdentTable::default(),
            code: Vec::new(),
            line_breaks: Vec::new(),
            loops: Vec::new(),
            function_locals: None,
            top_locals: Locals::default(),
            local_tables: Vec::new(),
            local_table_index: HashMap::new(),
            diagnostics: Vec::new(),
            overflow: false,
            visits: 0,
            number_strings: HashMap::new(),
        }
    }

    fn file_name(&self) -> String {
        // Text without a file (`eval`'d) is named "(null)", as `printf("%s", NULL)` prints it.
        self.file.map_or_else(
            || "(null)".to_string(),
            |f| String::from_utf8_lossy(f).into_owned(),
        )
    }

    fn warn(&mut self, line: u32, text: &str) {
        let file = self.file_name();
        self.diagnostics.push(Diagnostic {
            level: Level::Warning,
            text: format!("{file} ({line}): {text}").into_bytes(),
        });
    }

    /// The text of a source line (1-based), for the local-variable warnings.
    fn line_text(&self, line: u32) -> String {
        let text = (line as usize)
            .checked_sub(1)
            .and_then(|i| self.source.split(|&b| b == b'\n').nth(i))
            .unwrap_or_default();
        let text = text.strip_suffix(b"\r").unwrap_or(text);
        String::from_utf8_lossy(text).into_owned()
    }

    fn script_warning(&mut self, line: u32, what: &str) {
        let text = format!(
            "{what} File: {}\nLine Num: {line}\nLine: \"{}\"",
            self.file_name(),
            self.line_text(line)
        );
        self.diagnostics.push(Diagnostic {
            level: Level::Warning,
            text: text.into_bytes(),
        });
    }

    /// `getFuncVars`: the locals of the current function, or the top level's (with a warning in a file).
    fn locals(&mut self, line: u32) -> &mut Locals {
        if self.function_locals.is_none() && self.file.is_some() {
            self.script_warning(line, "Attemping to use local variable in global scope.");
        }
        match &mut self.function_locals {
            Some(locals) => locals,
            None => &mut self.top_locals,
        }
    }

    /// `FuncVars::lookup`: a local's register; one never assigned warns and becomes a string register.
    fn lookup(&mut self, name: &[u8], line: u32) -> u32 {
        if let Some((reg, _)) = self.locals(line).find(name) {
            return reg;
        }
        self.warn_unassigned(name, line);
        self.locals_quiet().assign(name, Type::String)
    }

    /// `FuncVars::lookupType`.
    fn lookup_type(&mut self, name: &[u8], line: u32) -> Type {
        if let Some((_, ty)) = self.locals(line).find(name) {
            return ty;
        }
        self.warn_unassigned(name, line);
        self.locals_quiet().assign(name, Type::String);
        Type::String
    }

    fn assign_local(&mut self, name: &[u8], ty: Type, line: u32) -> u32 {
        self.locals(line).assign(name, ty)
    }

    fn locals_quiet(&mut self) -> &mut Locals {
        match &mut self.function_locals {
            Some(locals) => locals,
            None => &mut self.top_locals,
        }
    }

    fn warn_unassigned(&mut self, name: &[u8], line: u32) {
        let what = format!(
            "Script Warning: Variable {} referenced before used when compiling script.",
            String::from_utf8_lossy(name)
        );
        self.script_warning(line, &what);
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

    /// `precompileIdent`: the name goes into the global table, case-sensitively.
    fn precompile_ident(&mut self, name: &[u8]) {
        self.global_strings.add(name, true, false);
    }

    /// Counts a visit; false once the budget is spent (the pass then stops descending).
    fn visit(&mut self) -> bool {
        self.visits = self.visits.saturating_add(1);
        self.visits <= MAX_VISITS
    }

    // ---------------------------------------------------------------- emitting

    fn tell(&self) -> u32 {
        self.code.len() as u32
    }

    /// Writes a word; returns its address.
    fn emit(&mut self, word: u32) -> u32 {
        let at = self.tell();
        if self.code.len() < MAX_CODE_WORDS as usize {
            self.code.push(word);
        } else {
            self.overflow = true;
        }
        at
    }

    fn emit_op(&mut self, op: Op) -> u32 {
        let code = self.opcodes.code(op);
        self.emit(code)
    }

    fn patch(&mut self, at: u32, word: u32) {
        if let Some(slot) = self.code.get_mut(at as usize) {
            *slot = word;
        }
    }

    /// `emitSTE` in its DSO-writing form: two zero words, and the name in the identifier table at the first
    /// one (its offset is the first global-table entry equal to it ignoring case). No name: no entry.
    fn emit_ident(&mut self, name: Option<&[u8]>) {
        let at = self.tell();
        if let Some(name) = name {
            let offset = self.global_strings.add(name, false, false);
            self.idents.add(offset, at);
        }
        self.emit(0);
        self.emit(0);
    }

    /// A placeholder word patched when the current loop ends.
    fn emit_fix(&mut self, fix: Fix) {
        let at = self.emit(0);
        if let Some(scope) = self.loops.last_mut() {
            scope.push((at, fix));
        }
    }

    fn add_break_line(&mut self, line: u32) {
        let at = self.tell();
        self.line_breaks.push(line);
        self.line_breaks.push(at);
    }

    // ---------------------------------------------------------------- statements

    fn compile_block(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            self.compile_stmt(StmtRef::Stmt(s));
        }
    }

    fn compile_stmt(&mut self, stmt: StmtRef<'_>) {
        if !self.visit() {
            return;
        }
        let s = match stmt {
            StmtRef::Stmt(s) => s,
            StmtRef::Case { .. } => return self.compile_if(stmt),
        };
        match &s.kind {
            StmtKind::Break | StmtKind::Continue => {
                let brk = matches!(s.kind, StmtKind::Break);
                if self.loops.is_empty() {
                    let what = if brk { "break" } else { "continue" };
                    self.warn(s.line, &format!("{what} outside of loop... ignoring."));
                    return;
                }
                self.add_break_line(s.line);
                self.emit_op(Op::Jmp);
                self.emit_fix(if brk { Fix::Break } else { Fix::Continue });
            }
            StmtKind::Expr(e) => {
                self.add_break_line(s.line);
                self.compile(Node::Expr(e), Type::None);
            }
            StmtKind::Return(e) => {
                self.add_break_line(s.line);
                let Some(e) = e else {
                    self.emit_op(Op::ReturnVoid);
                    return;
                };
                let mut ty = self.preferred(Node::Expr(e));
                if ty == Type::None {
                    ty = Type::String;
                }
                self.compile(Node::Expr(e), ty);
                self.emit_op(match ty {
                    Type::UInt => Op::ReturnUint,
                    Type::Float => Op::ReturnFlt,
                    _ => Op::Return,
                });
            }
            StmtKind::If { .. } | StmtKind::Switch { .. } => self.compile_if(stmt),
            StmtKind::Loop {
                init,
                test,
                step,
                body,
                post_test,
            } => self.compile_loop(
                s.line,
                init.as_deref(),
                test,
                step.as_deref(),
                body,
                *post_test,
            ),
            StmtKind::Foreach {
                string,
                variable,
                container,
                body,
            } => self.compile_foreach(s.line, *string, variable, container, body),
            StmtKind::Function(f) => self.compile_function(f, None),
            StmtKind::Package { name, functions } => {
                for f in functions {
                    self.compile_function(f, Some(name));
                }
            }
        }
    }

    /// The parts of an `if` (a real one, or the one a `case` becomes): its line, test, block and else.
    fn if_parts<'s>(stmt: StmtRef<'s>) -> Option<(u32, Node<'s>, Block<'s>, Option<Block<'s>>)> {
        let case_if =
            |string: bool, test: &'s Expr, cases: &'s [Case], default: Option<&'s [Stmt]>| {
                let (first, rest) = cases.split_first()?;
                let otherwise = if rest.is_empty() {
                    default.map(Block::Stmts)
                } else {
                    Some(Block::Case(StmtRef::Case {
                        string,
                        test,
                        cases: rest,
                        default,
                    }))
                };
                Some((
                    first.line,
                    Node::CaseOr {
                        string,
                        test,
                        values: &first.values,
                    },
                    Block::Stmts(&first.body),
                    otherwise,
                ))
            };
        match stmt {
            StmtRef::Stmt(s) => match &s.kind {
                StmtKind::If {
                    test,
                    then,
                    otherwise,
                } => Some((
                    s.line,
                    Node::Expr(test),
                    Block::Stmts(then),
                    otherwise.as_deref().map(Block::Stmts),
                )),
                StmtKind::Switch {
                    string,
                    test,
                    cases,
                    default,
                } => case_if(*string, test, cases, default.as_deref()),
                _ => None,
            },
            StmtRef::Case {
                string,
                test,
                cases,
                default,
            } => case_if(string, test, cases, default),
        }
    }

    fn compile_branch(&mut self, block: Block<'_>) {
        match block {
            Block::Stmts(stmts) => self.compile_block(stmts),
            Block::Case(case) => self.compile_stmt(case),
        }
    }

    fn compile_if(&mut self, stmt: StmtRef<'_>) {
        let Some((line, test, then, otherwise)) = Self::if_parts(stmt) else {
            return;
        };
        self.add_break_line(line);
        let ty = self.preferred(test);
        if ty == Type::String || ty == Type::None {
            self.compile(test, Type::String);
            self.emit_op(Op::JmpNotString);
        } else {
            let integer = ty == Type::UInt;
            self.compile(test, if integer { Type::UInt } else { Type::Float });
            self.emit_op(if integer { Op::JmpIfNot } else { Op::JmpIfFNot });
        }
        match otherwise {
            Some(otherwise) => {
                let else_ip = self.emit(0);
                self.compile_branch(then);
                let else_target = self.tell() + 2;
                self.emit_op(Op::Jmp);
                let end_ip = self.emit(0);
                self.compile_branch(otherwise);
                let end = self.tell();
                self.patch(end_ip, end);
                self.patch(else_ip, else_target);
            }
            None => {
                let end_ip = self.emit(0);
                self.compile_branch(then);
                let end = self.tell();
                self.patch(end_ip, end);
            }
        }
    }

    fn compile_loop(
        &mut self,
        line: u32,
        init: Option<&Expr>,
        test: &Expr,
        step: Option<&Expr>,
        body: &[Stmt],
        post_test: bool,
    ) {
        let integer = self.preferred(Node::Expr(test)) == Type::UInt;
        let test_type = if integer { Type::UInt } else { Type::Float };
        self.add_break_line(line);
        self.loops.push(Vec::new());
        if let Some(init) = init {
            self.compile(Node::Expr(init), Type::None);
        }
        if !post_test {
            self.compile(Node::Expr(test), test_type);
            self.emit_op(if integer { Op::JmpIfNot } else { Op::JmpIfFNot });
            self.emit_fix(Fix::Break);
        }
        let start = self.tell();
        self.compile_block(body);
        let cont = self.tell();
        if let Some(step) = step {
            self.compile(Node::Expr(step), Type::None);
        }
        self.compile(Node::Expr(test), test_type);
        self.emit_op(if integer { Op::JmpIf } else { Op::JmpIfF });
        self.emit_fix(Fix::LoopBlockStart);
        let brk = self.tell();
        for (at, fix) in self.loops.pop().unwrap_or_default() {
            let target = match fix {
                Fix::LoopBlockStart => start,
                Fix::Break => brk,
                Fix::Continue => cont,
            };
            self.patch(at, target);
        }
    }

    /// `IterStmtNode::compileStmt`: the container, `OP_ITER_BEGIN` (or `_STR`) with the variable and the address
    /// past the loop, then `OP_ITER` (the `continue` target, which leaves for the `break` target when the
    /// iteration is over), the body, a jump back to `OP_ITER`, and `OP_ITER_END` (the `break` target).
    fn compile_foreach(
        &mut self,
        line: u32,
        string: bool,
        variable: &[u8],
        container: &Expr,
        body: &[Stmt],
    ) {
        self.add_break_line(line);
        self.loops.push(Vec::new());
        let global = variable.first() == Some(&b'$');
        self.compile(Node::Expr(container), Type::String);
        self.emit_op(if string {
            Op::IterBeginStr
        } else {
            Op::IterBegin
        });
        self.emit(u32::from(global));
        if global {
            self.emit_ident(Some(variable));
        } else {
            let ty = if string { Type::String } else { Type::UInt };
            let reg = self.assign_local(variable, ty, line);
            self.emit(reg);
        }
        let final_slot = self.emit(0);
        let cont = self.emit_op(Op::Iter);
        self.emit_fix(Fix::Break);
        let start = self.tell();
        self.compile_block(body);
        let brk = self.tell() + 2;
        self.emit_op(Op::Jmp);
        self.emit_fix(Fix::Continue);
        self.emit_op(Op::IterEnd);
        self.patch(final_slot, brk + 1);
        for (at, fix) in self.loops.pop().unwrap_or_default() {
            let target = match fix {
                Fix::LoopBlockStart => start,
                Fix::Break => brk,
                Fix::Continue => cont,
            };
            self.patch(at, target);
        }
    }

    /// `FunctionDeclStmtNode::compileStmt`: the declaration (its fixed operands, then each argument's register,
    /// flags and default-value start), the body, `OP_RETURN_VOID`, then the code of each default value (ended by
    /// `OP_DEFAULT_END`).
    fn compile_function(&mut self, f: &Function, package: Option<&[u8]>) {
        self.function_tables = true;
        let outer = self.function_locals.replace(Locals::default());
        for p in &f.params {
            self.precompile_ident(&p.name);
            self.assign_local(&p.name, Type::None, f.line);
        }
        self.precompile_ident(&f.name);
        if let Some(ns) = &f.namespace {
            self.precompile_ident(ns);
        }
        if let Some(pkg) = package {
            self.precompile_ident(pkg);
        }
        let argc = u32::try_from(f.params.len()).unwrap_or(u32::MAX);
        self.emit_op(Op::FuncDecl);
        self.emit_ident(Some(&f.name));
        self.emit_ident(f.namespace.as_deref());
        self.emit_ident(package);
        self.emit(u32::from(f.body.is_some()) | f.line.wrapping_shl(1));
        let end_slot = self.emit(0);
        self.emit(argc);
        let registers_slot = self.emit(0);
        for p in &f.params {
            let reg = self.lookup(&p.name, f.line);
            self.emit(reg);
        }
        for p in &f.params {
            self.emit(u32::from(p.default.is_some()));
        }
        let default_slots: Vec<u32> = f.params.iter().map(|_| self.emit(0)).collect();
        if let Some(body) = f.body.as_deref() {
            self.compile_block(body);
        }
        self.add_break_line(f.line);
        self.emit_op(Op::ReturnVoid);
        let count = self.function_locals.as_ref().map_or(0, Locals::count);
        self.patch(registers_slot, count);
        for (p, slot) in f.params.iter().zip(default_slots) {
            let Some(default) = &p.default else {
                continue;
            };
            let start = self.tell();
            self.patch(slot, start);
            let ty = self.passed_type(Node::Expr(default));
            self.compile(Node::Expr(default), ty);
            self.emit_op(Op::DefaultEnd);
        }
        let locals = self.function_locals.take().unwrap_or_default();
        self.function_locals = outer;
        let end = self.tell();
        self.patch(end_slot, end);
        self.function_tables = false;

        let namespace = f.namespace.as_deref().unwrap_or(b"(null)");
        let mut key = namespace.to_vec();
        key.extend_from_slice(b"::");
        key.extend_from_slice(&f.name);
        let folded = key.to_ascii_lowercase();
        let entry = match self.local_table_index.get(&folded) {
            Some(&i) => i,
            None => {
                self.local_table_index
                    .insert(folded, self.local_tables.len());
                self.local_tables.push(LocalVariables {
                    function: key,
                    names: Vec::new(),
                });
                self.local_tables.len() - 1
            }
        };
        if let Some(table) = self.local_tables.get_mut(entry) {
            table.names.extend(locals.names);
        }
    }

    // ---------------------------------------------------------------- expressions

    /// `getPreferredType`. Asking about a local variable can make it (as reading it does).
    fn preferred(&mut self, node: Node<'_>) -> Type {
        let e = match node {
            Node::Expr(e) => e,
            Node::CaseTest { .. } | Node::CaseOr { .. } | Node::Int(_) => return Type::UInt,
        };
        match &e.kind {
            ExprKind::Int(_)
            | ExprKind::StrEq { .. }
            | ExprKind::New(_)
            | ExprKind::InternalField { .. } => Type::UInt,
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
            ExprKind::Var { name, index } => {
                if is_register_variable(name, index.as_deref()) {
                    self.lookup_type(name, e.line)
                } else {
                    Type::None
                }
            }
            ExprKind::Field { .. } | ExprKind::TagDeref(_) | ExprKind::Assert { .. } => Type::None,
            ExprKind::Ternary {
                then, otherwise, ..
            } => {
                let t = self.preferred(Node::Expr(then));
                let f = self.preferred(Node::Expr(otherwise));
                if t == f && t != Type::None {
                    t
                } else {
                    Type::String
                }
            }
            ExprKind::Assign { value, .. } => self.preferred(Node::Expr(value)),
            ExprKind::AssignOp { op, .. } | ExprKind::FieldAssignOp { op, .. } => {
                assign_operand(*op).0
            }
        }
    }

    /// The preferred type of a value passed on (an argument, a returned value): a string when it has none.
    fn passed_type(&mut self, node: Node<'_>) -> Type {
        match self.preferred(node) {
            Type::None => Type::String,
            ty => ty,
        }
    }

    fn compile_int(&mut self, value: i32, ty: Type) {
        match ty {
            Type::UInt => {
                self.emit_op(Op::LoadImmedUint);
                self.emit(value as u32);
            }
            Type::String => {
                let index =
                    self.number_string(false, u64::from(value as u32), || value.to_string());
                self.emit_op(Op::LoadImmedStr);
                self.emit(index);
            }
            Type::Float => {
                let index = self.floats().add(f64::from(value));
                self.emit_op(Op::LoadImmedFlt);
                self.emit(index);
            }
            Type::None => {}
        }
    }

    fn compile_float(&mut self, value: f64, ty: Type) {
        match ty {
            Type::UInt => {
                self.emit_op(Op::LoadImmedUint);
                self.emit(f64_to_u32(value));
            }
            Type::String => {
                let index = self.number_string(true, value.to_bits(), || format_g(value));
                self.emit_op(Op::LoadImmedStr);
                self.emit(index);
            }
            Type::Float => {
                let index = self.floats().add(value);
                self.emit_op(Op::LoadImmedFlt);
                self.emit(index);
            }
            Type::None => {}
        }
    }

    /// A string or a name used as a number.
    fn compile_text_number(&mut self, text: &[u8], line: u32, ty: Type) {
        let (value, warn) = string_to_number(text);
        if warn && self.file.is_some() {
            self.warn(line, "string always evaluates to 0.");
        }
        match ty {
            Type::UInt => {
                self.emit_op(Op::LoadImmedUint);
                self.emit(f64_to_u32(value));
            }
            Type::Float => {
                let index = self.floats().add(value);
                self.emit_op(Op::LoadImmedFlt);
                self.emit(index);
            }
            Type::None | Type::String => {}
        }
    }

    /// `test == value` (or `$=`) for one case value, or the `||` of several.
    fn compile_case(&mut self, node: Node<'_>, ty: Type) {
        match node {
            Node::CaseTest {
                string,
                test,
                value,
            } => {
                if string {
                    self.compile_streq(test, value, true);
                } else {
                    self.compile_int_binary(BinaryOp::Eq, Node::Expr(test), Node::Expr(value));
                }
            }
            Node::CaseOr {
                string,
                test,
                values,
            } => match values.split_last() {
                Some((last, rest)) if !rest.is_empty() => self.compile_int_binary(
                    BinaryOp::Or,
                    Node::CaseOr {
                        string,
                        test,
                        values: rest,
                    },
                    Node::CaseTest {
                        string,
                        test,
                        value: last,
                    },
                ),
                Some((only, _)) => self.compile_case(
                    Node::CaseTest {
                        string,
                        test,
                        value: only,
                    },
                    ty,
                ),
                None => self.compile(Node::Expr(test), ty),
            },
            _ => {}
        }
    }

    fn compile(&mut self, node: Node<'_>, ty: Type) {
        if !self.visit() {
            return;
        }
        let e = match node {
            Node::Expr(e) => e,
            Node::Int(v) => return self.compile_int(v, ty),
            Node::CaseTest { .. } | Node::CaseOr { .. } => return self.compile_case(node, ty),
        };
        match &e.kind {
            ExprKind::Int(v) => self.compile_int(*v, ty),
            ExprKind::Float(v) => self.compile_float(*v, ty),
            ExprKind::Binary { op, left, right } if is_float_op(*op) => {
                // Two number constants fold into one (a division by zero into 0).
                let number = |x: &Expr| match x.kind {
                    ExprKind::Int(v) => Some(f64::from(v)),
                    ExprKind::Float(v) => Some(v),
                    _ => None,
                };
                if let (Some(l), Some(r)) = (number(left), number(right)) {
                    let value = match op {
                        BinaryOp::Add => l + r,
                        BinaryOp::Sub => l - r,
                        BinaryOp::Mul => l * r,
                        _ if r != 0.0 => l / r,
                        _ => 0.0,
                    };
                    return self.compile_float(value, ty);
                }
                self.compile(Node::Expr(right), Type::Float);
                self.compile(Node::Expr(left), Type::Float);
                self.emit_op(int_operand(*op).1);
            }
            ExprKind::Binary { op, left, right } => {
                self.compile_int_binary(*op, Node::Expr(left), Node::Expr(right));
            }
            ExprKind::StrEq { equal, left, right } => self.compile_streq(left, right, *equal),
            ExprKind::Concat { kind, left, right } => {
                self.compile(Node::Expr(left), Type::String);
                let ch = match kind {
                    Concat::Plain => None,
                    Concat::Space => Some(b' '),
                    Concat::Tab => Some(b'\t'),
                    Concat::NewLine => Some(b'\n'),
                };
                if let Some(ch) = ch {
                    self.emit_op(Op::AdvanceStrAppendChar);
                    self.emit(u32::from(ch));
                }
                self.compile(Node::Expr(right), Type::String);
                self.emit_op(Op::RewindStr);
            }
            ExprKind::CommaCat { left, right } => {
                self.compile(Node::Expr(left), Type::String);
                self.emit_op(Op::AdvanceStrAppendChar);
                self.emit(u32::from(b'_'));
                self.compile(Node::Expr(right), Type::String);
                self.emit_op(Op::RewindStr);
                if ty == Type::UInt || ty == Type::Float {
                    self.warn(
                        e.line,
                        "converting comma string to a number... probably wrong.",
                    );
                }
            }
            ExprKind::Unary {
                op: UnaryOp::Neg,
                expr,
            } => {
                self.compile(Node::Expr(expr), Type::Float);
                self.emit_op(Op::Neg);
            }
            ExprKind::Unary { op, expr } => {
                let pref = self.preferred(Node::Expr(expr));
                let integer =
                    !(*op == UnaryOp::Not && (pref == Type::Float || pref == Type::String));
                self.compile(
                    Node::Expr(expr),
                    if integer { Type::UInt } else { Type::Float },
                );
                self.emit_op(match (op, integer) {
                    (UnaryOp::Not, true) => Op::Not,
                    (UnaryOp::Not, false) => Op::NotF,
                    _ => Op::OnesComplement,
                });
            }
            ExprKind::Ternary {
                test,
                then,
                otherwise,
            } => {
                let integer = self.preferred(Node::Expr(test)) == Type::UInt;
                self.compile(
                    Node::Expr(test),
                    if integer { Type::UInt } else { Type::Float },
                );
                self.emit_op(if integer { Op::JmpIfNot } else { Op::JmpIfFNot });
                let jump_else = self.emit(0);
                self.compile(Node::Expr(then), ty);
                self.emit_op(Op::Jmp);
                let jump_end = self.emit(0);
                let else_at = self.tell();
                self.patch(jump_else, else_at);
                self.compile(Node::Expr(otherwise), ty);
                let end = self.tell();
                self.patch(jump_end, end);
            }
            ExprKind::Var { name, index } => self.compile_var(name, index.as_deref(), e.line, ty),
            ExprKind::Str { text, tagged } => match ty {
                Type::String => {
                    let index = self.strings().add(text, true, *tagged);
                    self.emit_op(if *tagged {
                        Op::TagToStr
                    } else {
                        Op::LoadImmedStr
                    });
                    self.emit(index);
                }
                Type::UInt | Type::Float => self.compile_text_number(text, e.line, ty),
                Type::None => {}
            },
            ExprKind::Ident(name) => match ty {
                Type::String => {
                    self.precompile_ident(name);
                    self.emit_op(Op::LoadImmedIdent);
                    self.emit_ident(Some(name));
                }
                Type::UInt | Type::Float => self.compile_text_number(name, e.line, ty),
                Type::None => {}
            },
            ExprKind::Assign { name, index, value } => {
                self.compile_assign(name, index.as_deref(), value, e.line, ty);
            }
            ExprKind::AssignOp {
                name,
                index,
                op,
                value,
            } => self.compile_assign_op(name, index.as_deref(), *op, value, e.line, ty),
            ExprKind::Call {
                namespace,
                name,
                args,
            } => {
                self.precompile_ident(name);
                if let Some(ns) = namespace {
                    self.precompile_ident(ns);
                }
                let call = match namespace {
                    None => CallType::Function,
                    Some(ns) if ns.eq_ignore_ascii_case(b"Parent") => CallType::Parent,
                    Some(_) => CallType::Static,
                };
                self.compile_call(name, namespace.as_deref(), None, args, call, ty);
            }
            ExprKind::MethodCall { object, name, args } => {
                self.precompile_ident(name);
                self.compile_call(name, None, Some(object), args, CallType::Method, ty);
            }
            ExprKind::Field {
                object,
                name,
                index,
            } => {
                if ty == Type::None {
                    return;
                }
                self.precompile_ident(name);
                if let Some(index) = index {
                    self.compile(Node::Expr(index), Type::String);
                }
                self.compile(Node::Expr(object), Type::String);
                self.emit_op(Op::SetCurObject);
                self.emit_op(Op::SetCurField);
                self.emit_ident(Some(name));
                self.emit_op(Op::PopStk);
                if index.is_some() {
                    self.emit_op(Op::SetCurFieldArray);
                    self.emit_op(Op::PopStk);
                }
                self.emit_op(match ty {
                    Type::UInt => Op::LoadFieldUint,
                    Type::Float => Op::LoadFieldFlt,
                    _ => Op::LoadFieldStr,
                });
            }
            ExprKind::FieldAssign {
                object,
                name,
                index,
                value,
            } => {
                self.precompile_ident(name);
                self.compile(Node::Expr(value), Type::String);
                if let Some(index) = index {
                    self.compile(Node::Expr(index), Type::String);
                }
                match object {
                    Some(object) => {
                        self.compile(Node::Expr(object), Type::String);
                        self.emit_op(Op::SetCurObject);
                    }
                    None => {
                        self.emit_op(Op::SetCurObjectNew);
                    }
                }
                self.emit_op(Op::SetCurField);
                self.emit_ident(Some(name));
                // A field of an object being made keeps the object on the stack.
                if object.is_some() {
                    self.emit_op(Op::PopStk);
                }
                if index.is_some() {
                    self.emit_op(Op::SetCurFieldArray);
                    self.emit_op(Op::PopStk);
                }
                self.emit_op(Op::SaveFieldStr);
                if ty == Type::None {
                    self.emit_op(Op::PopStk);
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
                self.precompile_ident(name);
                self.compile(Node::Expr(value), sub);
                if let Some(index) = index {
                    self.compile(Node::Expr(index), Type::String);
                }
                self.compile(Node::Expr(object), Type::String);
                self.emit_op(Op::SetCurObject);
                self.emit_op(Op::SetCurField);
                self.emit_ident(Some(name));
                self.emit_op(Op::PopStk);
                if index.is_some() {
                    self.emit_op(Op::SetCurFieldArray);
                    if ty == Type::None {
                        self.emit_op(Op::PopStk);
                    }
                }
                let float = sub == Type::Float;
                self.emit_op(if float {
                    Op::LoadFieldFlt
                } else {
                    Op::LoadFieldUint
                });
                self.emit_op(operand);
                self.emit_op(if float {
                    Op::SaveFieldFlt
                } else {
                    Op::SaveFieldUint
                });
                if ty == Type::None {
                    self.emit_op(Op::PopStk);
                }
            }
            ExprKind::InternalField {
                object,
                name,
                recursive,
            } => {
                if ty == Type::None {
                    return;
                }
                self.compile(Node::Expr(object), Type::String);
                self.emit_op(Op::SetCurObject);
                // The internal object replaces the current one.
                self.emit_op(Op::PopStk);
                self.compile(Node::Expr(name), Type::String);
                self.emit_op(Op::SetCurObjectInternal);
                self.emit(u32::from(*recursive));
            }
            ExprKind::Assert { test, message } => {
                let index = self.strings().add(message, true, false);
                self.compile(Node::Expr(test), Type::UInt);
                self.emit_op(Op::Assert);
                self.emit(index);
            }
            ExprKind::TagDeref(_) => {}
            ExprKind::New(decl) => {
                self.emit_op(Op::LoadImmedUint);
                self.emit(0);
                self.compile_object(decl, true);
                if ty == Type::None {
                    self.emit_op(Op::PopStk);
                }
            }
        }
    }

    fn compile_var(&mut self, name: &[u8], index: Option<&Expr>, line: u32, ty: Type) {
        if ty == Type::None {
            return;
        }
        self.precompile_ident(name);
        if is_register_variable(name, index) {
            self.emit_op(match ty {
                Type::UInt => Op::LoadLocalVarUint,
                Type::Float => Op::LoadLocalVarFlt,
                _ => Op::LoadLocalVarStr,
            });
            let reg = self.lookup(name, line);
            self.emit(reg);
            return;
        }
        self.emit_op(if index.is_some() {
            Op::LoadImmedIdent
        } else {
            Op::SetCurVar
        });
        self.emit_ident(Some(name));
        if let Some(index) = index {
            self.compile(Node::Expr(index), Type::String);
            self.emit_op(Op::RewindStr);
            self.emit_op(Op::SetCurVarArray);
            self.emit_op(Op::PopStk);
        }
        self.emit_op(match ty {
            Type::UInt => Op::LoadVarUint,
            Type::Float => Op::LoadVarFlt,
            _ => Op::LoadVarStr,
        });
    }

    /// The global variable (with its index) an assignment writes: `OP_SETCURVAR_CREATE`, or the name, the
    /// index and `OP_SETCURVAR_ARRAY_CREATE`.
    fn select_global(&mut self, name: &[u8], index: Option<&Expr>, ty: Type) {
        match index {
            None => {
                self.emit_op(Op::SetCurVarCreate);
                self.emit_ident(Some(name));
            }
            Some(index) => {
                self.emit_op(Op::LoadImmedIdent);
                self.emit_ident(Some(name));
                self.compile(Node::Expr(index), Type::String);
                self.emit_op(Op::RewindStr);
                self.emit_op(Op::SetCurVarArrayCreate);
                if ty == Type::None {
                    self.emit_op(Op::PopStk);
                }
            }
        }
    }

    fn compile_assign(
        &mut self,
        name: &[u8],
        index: Option<&Expr>,
        value: &Expr,
        line: u32,
        ty: Type,
    ) {
        let mut sub = self.preferred(Node::Expr(value));
        if sub == Type::None {
            sub = ty;
        }
        if sub == Type::None {
            sub = Type::String;
        }
        self.precompile_ident(name);
        self.compile(Node::Expr(value), sub);
        if is_register_variable(name, index) {
            self.emit_op(match sub {
                Type::UInt => Op::SaveLocalVarUint,
                Type::Float => Op::SaveLocalVarFlt,
                _ => Op::SaveLocalVarStr,
            });
            let reg = self.assign_local(name, sub, line);
            self.emit(reg);
        } else {
            self.select_global(name, index, ty);
            self.emit_op(match sub {
                Type::UInt => Op::SaveVarUint,
                Type::Float => Op::SaveVarFlt,
                _ => Op::SaveVarStr,
            });
        }
        if ty == Type::None {
            self.emit_op(Op::PopStk);
        }
    }

    fn compile_assign_op(
        &mut self,
        name: &[u8],
        index: Option<&Expr>,
        op: AssignOp,
        value: &Expr,
        line: u32,
        ty: Type,
    ) {
        let (sub, operand) = assign_operand(op);
        self.precompile_ident(name);
        let register = is_register_variable(name, index);
        if op == AssignOp::Increment && register && ty == Type::None {
            let reg = self.assign_local(name, Type::Float, line);
            self.emit_op(Op::Inc);
            self.emit(reg);
            return;
        }
        self.compile(Node::Expr(value), sub);
        let float = sub == Type::Float;
        if register {
            let reg = self.assign_local(name, sub, line);
            self.emit_op(if float {
                Op::LoadLocalVarFlt
            } else {
                Op::LoadLocalVarUint
            });
            self.emit(reg);
            self.emit_op(operand);
            self.emit_op(if float {
                Op::SaveLocalVarFlt
            } else {
                Op::SaveLocalVarUint
            });
            self.emit(reg);
        } else {
            self.select_global(name, index, ty);
            self.emit_op(if float {
                Op::LoadVarFlt
            } else {
                Op::LoadVarUint
            });
            self.emit_op(operand);
            self.emit_op(if float {
                Op::SaveVarFlt
            } else {
                Op::SaveVarUint
            });
        }
        if ty == Type::None {
            self.emit_op(Op::PopStk);
        }
    }

    fn compile_call(
        &mut self,
        name: &[u8],
        namespace: Option<&[u8]>,
        object: Option<&Expr>,
        args: &[Expr],
        call: CallType,
        ty: Type,
    ) {
        let count = args.len() + usize::from(object.is_some());
        self.emit_op(Op::PushFrame);
        self.emit(u32::try_from(count).unwrap_or(u32::MAX));
        for a in object.into_iter().chain(args) {
            let arg_type = self.passed_type(Node::Expr(a));
            self.compile(Node::Expr(a), arg_type);
            self.emit_op(Op::Push);
        }
        self.emit_op(Op::CallFunc);
        self.emit_ident(Some(name));
        self.emit_ident(namespace);
        self.emit(call as u32);
        if ty == Type::None {
            self.emit_op(Op::PopStk);
        }
    }

    /// An integer binary node. `x % 2`, `% 4` and `% 8` of a variable become `& 1`, `& 3` and `& 7`.
    fn compile_int_binary(&mut self, op: BinaryOp, left: Node<'_>, right: Node<'_>) {
        let mut op = op;
        let mut right = right;
        if op == BinaryOp::Mod
            && let Node::Expr(l) = left
            && matches!(l.kind, ExprKind::Var { .. })
            && let Node::Expr(r) = right
        {
            let divisor = match r.kind {
                ExprKind::Int(v) => Some(v),
                ExprKind::Float(v) => Some(v as i32),
                _ => None,
            };
            if let Some(mask) = match divisor {
                Some(2) => Some(1),
                Some(4) => Some(3),
                Some(8) => Some(7),
                _ => None,
            } {
                op = BinaryOp::BitAnd;
                right = Node::Int(mask);
            }
        }
        let (sub, operand) = int_operand(op);
        if operand == Op::Or || operand == Op::And {
            self.compile(left, sub);
            self.emit_op(if operand == Op::Or {
                Op::JmpIfNp
            } else {
                Op::JmpIfNotNp
            });
            let jump = self.emit(0);
            self.compile(right, sub);
            let end = self.tell();
            self.patch(jump, end);
        } else {
            self.compile(right, sub);
            self.compile(left, sub);
            self.emit_op(operand);
        }
    }

    fn compile_streq(&mut self, left: &Expr, right: &Expr, equal: bool) {
        self.compile(Node::Expr(left), Type::String);
        self.compile(Node::Expr(right), Type::String);
        self.emit_op(Op::CompareStr);
        if !equal {
            self.emit_op(Op::Not);
        }
    }

    /// `ObjectDeclNode::compileSubObject`.
    fn compile_object(&mut self, decl: &ObjectDecl, root: bool) {
        self.emit_op(Op::PushFrame);
        self.emit(u32::try_from(decl.args.len() + 2).unwrap_or(u32::MAX));
        self.compile(Node::Expr(&decl.class), Type::String);
        self.emit_op(Op::Push);
        self.compile(Node::Expr(&decl.name), Type::String);
        self.emit_op(Op::Push);
        for a in &decl.args {
            let arg_type = self.passed_type(Node::Expr(a));
            self.compile(Node::Expr(a), arg_type);
            self.emit_op(Op::Push);
        }
        self.emit_op(Op::CreateObject);
        self.emit_ident(decl.parent.as_deref());
        self.emit(u32::from(decl.datablock));
        self.emit(u32::from(decl.internal_name));
        self.emit(u32::from(decl.singleton));
        self.emit(decl.line);
        let fail_slot = self.emit(0);
        for f in &decl.fields {
            self.compile(Node::Expr(f), Type::None);
        }
        self.emit_op(Op::AddObject);
        self.emit(u32::from(root));
        for c in &decl.children {
            self.compile_object(c, false);
        }
        self.emit_op(Op::EndObject);
        self.emit(u32::from(root || decl.datablock));
        let fail = self.emit_op(Op::FinishObject);
        self.patch(fail_slot, fail);
    }

    // ---------------------------------------------------------------- the file

    /// The DSO of a file's statements (`CodeBlock::compile`): their code and a final `OP_RETURN_VOID`.
    /// `Err` (with the messages) when the file is beyond [`MAX_VISITS`] or [`MAX_CODE_WORDS`].
    pub(crate) fn generate(
        mut self,
        script: &Script,
        dialect: &Dialect,
    ) -> (Result<DsoFile, ()>, Vec<Diagnostic>, u64) {
        self.compile_block(&script.stmts);
        self.emit_op(Op::ReturnVoid);
        if self.visits > MAX_VISITS || self.overflow {
            self.diagnostics.push(Diagnostic {
                level: Level::Error,
                text: b"the script is too large to compile".to_vec(),
            });
            return (Err(()), self.diagnostics, self.visits);
        }
        let dso = DsoFile {
            version: dialect.dso_version,
            global_strings: self.global_strings.bytes().to_vec(),
            global_floats: self.global_floats.values().to_vec(),
            function_strings: self.function_strings.bytes().to_vec(),
            function_floats: self.function_floats.values().to_vec(),
            local_variables: if dialect.layout.local_variable_table {
                self.local_tables
            } else {
                Vec::new()
            },
            code: self.code,
            line_breaks: self.line_breaks,
            identifiers: self.idents.entries(),
        };
        (Ok(dso), self.diagnostics, self.visits)
    }
}

#[cfg(test)]
#[path = "codegen_tests.rs"]
mod tests;
