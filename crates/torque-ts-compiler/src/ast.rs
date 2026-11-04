//! The syntax tree of a TorqueScript file, as the parser builds it (one node class per construct of Torque3D's
//! tree). It is the seam a different front end (a DSL) can feed to the code generator. Every node carries
//! the line it was given when it was made: the scanner's line counter at that moment, which is what the
//! line-break table and the compiler's warnings print.
//! Follows Torque3D's `ast.h` and `astAlloc.cpp` (MIT); the credits are in the crate documentation.

use std::rc::Rc;

/// An identifier, a variable name or a field name, spelled as its first occurrence in the file spelled it
/// (they are interned case-insensitively in the global string table).
pub type Name = Rc<[u8]>;

/// A whole file: its top-level statements in order (function declarations and packages included).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Script {
    pub stmts: Vec<Stmt>,
}

/// The large parts of the rarer statements are boxed, so a statement stays small (about the size of an
/// expression): a file can hold a million of them.
#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    /// The statement's line (`StmtNode::dbgLineNumber`). For `if`, `while` and `for` it is the line of the
    /// keyword; for a `switch` the line of its first `case`.
    pub line: u32,
    pub kind: StmtKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    Break,
    Continue,
    Return(Option<Expr>),
    /// An expression used as a statement: a call, an assignment or an object declaration.
    Expr(Expr),
    If {
        test: Box<Expr>,
        then: Vec<Stmt>,
        otherwise: Option<Vec<Stmt>>,
    },
    /// `while`, `for` and `do`-`while` (`init` and `step` are `None` for `while`, and for the parts a `for`
    /// leaves out; a `for` without a test has the constant 1). A `do`-`while` (`post_test`) runs its body
    /// before the first test; its line is the line of its `while`.
    Loop {
        init: Option<Box<Expr>>,
        test: Box<Expr>,
        step: Option<Box<Expr>>,
        body: Vec<Stmt>,
        post_test: bool,
    },
    /// `switch` (`string` for `switch$`). It becomes a chain of `if`s at parse time
    /// (`IfStmtNode::propagateSwitchExpr`); the code generator does the same.
    Switch {
        string: bool,
        test: Box<Expr>,
        cases: Vec<Case>,
        default: Option<Vec<Stmt>>,
    },
    /// `foreach (%v in container) body` and `foreach$` (`string`): the body runs once per object of a set (or per
    /// word of a string), the variable holding it (`IterStmtNode`).
    Foreach {
        string: bool,
        variable: Name,
        container: Box<Expr>,
        body: Vec<Stmt>,
    },
    Function(Box<Function>),
    /// `package Name { functions };`: each function is declared in the package.
    Package {
        name: Name,
        functions: Vec<Function>,
    },
}

/// Statements nest as deep as the parser's stack allows (thousands of levels of `if (1) {`): dropping one takes
/// the statements under it out into a list instead of recursing.
impl Drop for Stmt {
    fn drop(&mut self) {
        let mut pending = Vec::new();
        take_statements(&mut self.kind, &mut pending);
        while let Some(mut s) = pending.pop() {
            take_statements(&mut s.kind, &mut pending);
        }
    }
}

/// Moves the statements directly under `kind` into `out`.
fn take_statements(kind: &mut StmtKind, out: &mut Vec<Stmt>) {
    match kind {
        StmtKind::Break | StmtKind::Continue | StmtKind::Return(_) | StmtKind::Expr(_) => {}
        StmtKind::If {
            then, otherwise, ..
        } => {
            out.append(then);
            if let Some(o) = otherwise {
                out.append(o);
            }
        }
        StmtKind::Loop { body, .. } | StmtKind::Foreach { body, .. } => out.append(body),
        StmtKind::Switch { cases, default, .. } => {
            for c in cases {
                out.append(&mut c.body);
            }
            if let Some(d) = default {
                out.append(d);
            }
        }
        StmtKind::Function(f) => {
            if let Some(b) = &mut f.body {
                out.append(b);
            }
        }
        StmtKind::Package { functions, .. } => {
            for f in functions {
                if let Some(b) = &mut f.body {
                    out.append(b);
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Case {
    /// The line of the `case` keyword.
    pub line: u32,
    /// The values (`case a or b:`).
    pub values: Vec<Expr>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub line: u32,
    pub name: Name,
    /// `Namespace::name`.
    pub namespace: Option<Name>,
    pub params: Vec<Param>,
    /// `None` for an empty body `{}` (no statement list is stored and the declaration is marked as
    /// having no body).
    pub body: Option<Vec<Stmt>>,
}

/// A parameter of a function: `%a`, or `%a = default` (`%a ?` and `%a ? = default` are the same).
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: Name,
    /// Evaluated when a call leaves the argument out.
    pub default: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub line: u32,
    pub kind: ExprKind,
}

/// Expressions are the only nodes that nest thousands of levels deep (`1 + 1 + ...`, `%a.f.g...` grow
/// without growing the parser's stack): dropping one takes its sub-expressions out into a list instead of
/// recursing, so a tree of [`crate::parser::MAX_TREE_DEPTH`] levels can be dropped on any thread.
/// Statements and objects nest only as deep as the parser's stack and [`crate::parser::MAX_TREE_DEPTH`] allow.
impl Drop for Expr {
    fn drop(&mut self) {
        let mut pending = Vec::new();
        take_children(&mut self.kind, &mut pending);
        while let Some(mut e) = pending.pop() {
            take_children(&mut e.kind, &mut pending);
        }
    }
}

/// An expression with nothing under it, left where a sub-expression was taken out.
fn leaf() -> Expr {
    Expr {
        line: 0,
        kind: ExprKind::Int(0),
    }
}

fn take_box(b: &mut Box<Expr>, out: &mut Vec<Expr>) {
    out.push(std::mem::replace(&mut **b, leaf()));
}

fn take_option(b: &mut Option<Box<Expr>>, out: &mut Vec<Expr>) {
    if let Some(b) = b {
        take_box(b, out);
    }
}

fn take_object(d: &mut ObjectDecl, out: &mut Vec<Expr>) {
    out.push(std::mem::replace(&mut d.class, leaf()));
    out.push(std::mem::replace(&mut d.name, leaf()));
    out.append(&mut d.args);
    out.append(&mut d.fields);
    for child in &mut d.children {
        take_object(child, out);
    }
}

/// Moves the sub-expressions of `kind` into `out`.
fn take_children(kind: &mut ExprKind, out: &mut Vec<Expr>) {
    match kind {
        ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Str { .. } | ExprKind::Ident(_) => {}
        ExprKind::Var { index, .. } => take_option(index, out),
        ExprKind::Unary { expr, .. }
        | ExprKind::TagDeref(expr)
        | ExprKind::Assert { test: expr, .. } => take_box(expr, out),
        ExprKind::Binary { left, right, .. }
        | ExprKind::StrEq { left, right, .. }
        | ExprKind::Concat { left, right, .. }
        | ExprKind::CommaCat { left, right }
        | ExprKind::InternalField {
            object: left,
            name: right,
            ..
        } => {
            take_box(left, out);
            take_box(right, out);
        }
        ExprKind::Ternary {
            test,
            then,
            otherwise,
        } => {
            take_box(test, out);
            take_box(then, out);
            take_box(otherwise, out);
        }
        ExprKind::Assign { index, value, .. } | ExprKind::AssignOp { index, value, .. } => {
            take_option(index, out);
            take_box(value, out);
        }
        ExprKind::Call { args, .. } => out.append(args),
        ExprKind::MethodCall { object, args, .. } => {
            take_box(object, out);
            out.append(args);
        }
        ExprKind::Field { object, index, .. } => {
            take_box(object, out);
            take_option(index, out);
        }
        ExprKind::FieldAssign {
            object,
            index,
            value,
            ..
        } => {
            take_option(object, out);
            take_option(index, out);
            take_box(value, out);
        }
        ExprKind::FieldAssignOp {
            object,
            index,
            value,
            ..
        } => {
            take_box(object, out);
            take_option(index, out);
            take_box(value, out);
        }
        ExprKind::New(decl) => take_object(decl, out),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    /// `-x` (a float operation).
    Neg,
    /// `!x`
    Not,
    /// `~x`
    BitNot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    // Float operations (`FloatBinaryExprNode`).
    Add,
    Sub,
    Mul,
    Div,
    // Integer operations and comparisons (`IntBinaryExprNode`).
    Mod,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Lt,
    Gt,
    Le,
    Ge,
    Eq,
    Ne,
    And,
    Or,
}

/// The operators of `op=` and `++`/`--`. `x++` and `x--` add and subtract the float 1 (their value is that
/// constant); they are kept apart from `+=` and `-=` because a code generator may have a shorter form for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    Increment,
    Decrement,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    BitAnd,
    BitXor,
    BitOr,
    Shl,
    Shr,
}

/// `@`, `SPC`, `TAB`, `NL`: the byte inserted between the two strings (none for `@`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Concat {
    Plain,
    Space,
    Tab,
    NewLine,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Int(i32),
    Float(f64),
    /// A string literal (`"..."`, or `'...'` when `tagged`), escapes decoded, cut at a NUL.
    Str {
        text: Vec<u8>,
        tagged: bool,
    },
    /// A bare identifier used as a value (`ConstantNode`): its name is the string. `break` used as a
    /// value is the identifier `break`.
    Ident(Name),
    /// `$x`, `%x`, `$x[i]` (an index of several expressions is a [`ExprKind::CommaCat`] chain).
    Var {
        name: Name,
        index: Option<Box<Expr>>,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    /// `$=` (`equal`) and `!$=`.
    StrEq {
        equal: bool,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Concat {
        kind: Concat,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    /// The `,` of an array index: `$a[1, 2]` joins the parts with the separator `_`.
    CommaCat {
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Ternary {
        test: Box<Expr>,
        then: Box<Expr>,
        otherwise: Box<Expr>,
    },
    Assign {
        name: Name,
        index: Option<Box<Expr>>,
        value: Box<Expr>,
    },
    AssignOp {
        name: Name,
        index: Option<Box<Expr>>,
        op: AssignOp,
        value: Box<Expr>,
    },
    /// `name(args)` and `Namespace::name(args)` (`Parent::name(args)` is a parent call).
    Call {
        namespace: Option<Name>,
        name: Name,
        args: Vec<Expr>,
    },
    /// `object.name(args)`.
    MethodCall {
        object: Box<Expr>,
        name: Name,
        args: Vec<Expr>,
    },
    /// `object.field`, `object.field[index]`.
    Field {
        object: Box<Expr>,
        name: Name,
        index: Option<Box<Expr>>,
    },
    /// `object.field = value`; inside an object declaration's block the object is `None` (the object
    /// being made).
    FieldAssign {
        object: Option<Box<Expr>>,
        name: Name,
        index: Option<Box<Expr>>,
        value: Box<Expr>,
    },
    FieldAssignOp {
        object: Box<Expr>,
        name: Name,
        index: Option<Box<Expr>>,
        op: AssignOp,
        value: Box<Expr>,
    },
    /// `object->name` and `object-->name` (`recursive`): the child of `object` whose internal name is `name`
    /// (`InternalSlotAccessNode`).
    InternalField {
        object: Box<Expr>,
        name: Box<Expr>,
        recursive: bool,
    },
    /// `assert(test)` and `assert(test, "message")` (`AssertCallExprNode`); the message of the first form is
    /// `TorqueScript assert!`.
    Assert {
        test: Box<Expr>,
        message: Vec<u8>,
    },
    /// `*expr` (`TTagDerefNode`): a remnant of the tag syntax; it compiles to nothing.
    TagDeref(Box<Expr>),
    /// `new Class(name, args) { fields; children; }`, `singleton Class(...)` and
    /// `datablock Class(name : parent) { fields };`.
    New(Box<ObjectDecl>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ObjectDecl {
    pub line: u32,
    /// The class: an identifier ([`ExprKind::Ident`]) or a parenthesised expression.
    pub class: Expr,
    /// The object's name; an omitted name is the empty string literal the parser makes for it.
    pub name: Expr,
    /// `: Parent` (an empty name is stored when there is none).
    pub parent: Option<Name>,
    pub args: Vec<Expr>,
    /// The field assignments of the block (each a [`ExprKind::FieldAssign`] with no object).
    pub fields: Vec<Expr>,
    pub children: Vec<ObjectDecl>,
    pub datablock: bool,
    /// `new Class([name])`: the name is the object's internal name.
    pub internal_name: bool,
    /// `singleton Class(name)`: an existing object of that name is used instead of a new one.
    pub singleton: bool,
}
