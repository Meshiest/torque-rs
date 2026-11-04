//! The grammar and parser (`CMDparse`, a yacc parser). The grammar is Torque3D's `CMDgram.y`, with the same rules
//! and `%left`/`%right` declarations: TGE 1.4's (which already has `do`-`while`, the eight `for` forms and `/* */`
//! comments) with `foreach`/`foreach$`, `assert`, `singleton`, internal names (`new C([name])`, `->`, `-->`),
//! parameter defaults and a datablock whose class and name are expressions. [`crate::lalr`] builds the LALR(1)
//! tables from it; the driver below reads a lookahead token only in a state
//! that has a shift or several reductions, as yacc's does, so a node made by a reduction gets the scanner's
//! line counter at that moment (`dbgLineNumber`). The driver stops at the first syntax error (no recovery:
//! the report, then the statements already reduced are what the file compiles to), and a state stack of
//! [`MAX_PARSE_DEPTH`] entries overflows with the same report.
//!
//! The rules for the tokens the scanner never produces are left out: the "tagged string table" tokens (`TTAG`),
//! the console type names (`TYPEIDENT`, which need the engine's type registry) and doc comments (`DOCBLOCK`).
//! That removes only their own states and their columns of the others, so every input the scanner can produce is
//! parsed the same.
//!
//! The grammar is credited in the crate documentation (Torque3D, MIT).
//!
//! **Dialects.** A [`Syntax`] chooses the grammar: [`Syntax::TORQUE3D`] (the default) or [`Syntax::TGE14`], TGE 1.4's
//! `CMDgram.y` (`engine/console/`, which carries no notice of its own; the tree is Copyright (C) GarageGames.com,
//! Inc.): Torque3D's rules without its additions, with a datablock's class and name plain identifiers and at least one
//! field, in the same order. A syntax can also leave out the `do`-`while` rule and the `for` forms that omit a part, bound the state stack lower, and name the field
//! `datablock = x;` sets. [`parse_scanned`] parses the tokens of any [`Scanner`].

use std::sync::OnceLock;

use crate::ast::{
    AssignOp, BinaryOp, Case, Concat, Expr, ExprKind, Function, Name, ObjectDecl, Param, Script,
    Stmt, StmtKind, UnaryOp,
};
use crate::lalr::{self, Action, Assoc, Rule, Sym, Tables};
use crate::lexer::{Language, Lexed, Lexer, Token, TokenValue, tok};
use crate::{Diagnostic, Level};

/// Nonterminals.
mod nt {
    pub const ACCEPT: u8 = 0;
    pub const START: u8 = 1;
    pub const DECL_LIST: u8 = 2;
    pub const DECL: u8 = 3;
    pub const PACKAGE_DECL: u8 = 4;
    pub const FN_DECL_LIST: u8 = 5;
    pub const STATEMENT_LIST: u8 = 6;
    pub const STMT: u8 = 7;
    pub const FN_DECL_STMT: u8 = 8;
    pub const VAR_LIST_DECL: u8 = 9;
    pub const VAR_LIST: u8 = 10;
    pub const DATABLOCK_DECL: u8 = 11;
    pub const OBJECT_DECL: u8 = 12;
    pub const PARENT_BLOCK: u8 = 13;
    pub const OBJECT_NAME: u8 = 14;
    pub const OBJECT_ARGS: u8 = 15;
    pub const OBJECT_DECLARE_BLOCK: u8 = 16;
    pub const OBJECT_DECL_LIST: u8 = 17;
    pub const STMT_BLOCK: u8 = 18;
    pub const SWITCH_STMT: u8 = 19;
    pub const CASE_BLOCK: u8 = 20;
    pub const CASE_EXPR: u8 = 21;
    pub const IF_STMT: u8 = 22;
    pub const WHILE_STMT: u8 = 23;
    pub const FOR_STMT: u8 = 24;
    pub const EXPRESSION_STMT: u8 = 25;
    pub const EXPR: u8 = 26;
    pub const SLOT_ACC: u8 = 27;
    pub const CLASS_NAME_EXPR: u8 = 28;
    pub const ASSIGN_OP_STRUCT: u8 = 29;
    pub const STMT_EXPR: u8 = 30;
    pub const FUNCALL_EXPR: u8 = 31;
    pub const EXPR_LIST_DECL: u8 = 32;
    pub const EXPR_LIST: u8 = 33;
    pub const SLOT_ASSIGN_LIST: u8 = 34;
    pub const SLOT_ASSIGN: u8 = 35;
    pub const AIDX_EXPR: u8 = 36;
    pub const FOREACH_STMT: u8 = 37;
    pub const PARAM: u8 = 38;
    pub const INTSLOT_ACC: u8 = 39;
    pub const ASSERT_EXPR: u8 = 40;
    pub const SLOT_ASSIGN_LIST_OPT: u8 = 41;
    pub const COUNT: usize = 42;
}

/// What a rule's reduction builds (the semantic actions of `CMDgram.y`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Act {
    Accept,
    Start,
    DeclListEmpty,
    DeclListAppend,
    Pass,
    DeclFn,
    DeclPackage,
    Package,
    FnListOne,
    FnListAppend,
    StmtListEmpty,
    StmtListAppend,
    StmtOfStmt,
    Break,
    Continue,
    ReturnNone,
    Return,
    ExprStmt,
    Function,
    FunctionNs,
    Empty,
    VarListOne,
    VarListAppend,
    /// A parameter: `%a`, `%a ?`, `%a = default` or `%a ? = default`.
    Param {
        optional: bool,
        default: bool,
    },
    Datablock,
    /// `new` or `singleton`; with a block of fields and children or not; with the name in brackets (an internal
    /// name) or not.
    Object {
        block: bool,
        internal: bool,
        singleton: bool,
    },
    ParentNone,
    ParentName,
    ObjectNameEmpty,
    ObjectArgs,
    BlockEmpty,
    BlockSlots,
    BlockDecls,
    BlockBoth,
    ObjListOne,
    ObjListAppend,
    StmtBlockBraces,
    StmtBlockOne,
    Switch,
    SwitchStr,
    CaseOne,
    CaseDefault,
    CaseMore,
    CaseExprOne,
    CaseExprOr,
    If,
    IfElse,
    While,
    DoWhile,
    /// `foreach` (false) or `foreach$` (true).
    Foreach(bool),
    /// A `for`; the flags say which of its three parts it has.
    For {
        init: bool,
        test: bool,
        step: bool,
    },
    Paren,
    IntBinary(BinaryOp),
    FloatBinary(BinaryOp),
    Neg,
    TagDeref,
    Ternary,
    StrEq(bool),
    Concat,
    Not,
    BitNot,
    TagAtom,
    Float,
    Int,
    BreakConst,
    SlotAccExpr,
    NewObject,
    IdentConst,
    StrAtom,
    Var,
    VarIndex,
    SlotAcc,
    SlotAccIndex,
    /// `->` (false) or `-->` (true).
    InternalSlot(bool),
    /// `assert(test)` or `assert(test, "message")`.
    Assert {
        message: bool,
    },
    ClassIdent,
    ClassParen,
    AssignInc,
    AssignDec,
    AssignOpOf(AssignOp),
    Assign,
    AssignIndex,
    AssignOpVar,
    AssignOpIndex,
    SlotAssignOp,
    SlotAssign,
    SlotAssignList,
    Call,
    CallNs,
    MethodCall,
    ListEmpty,
    ListOne,
    ListAppend,
    SlotListOne,
    SlotListAppend,
    SlotInit,
    SlotInitDatablock,
    SlotInitIndex,
    AidxOne,
    AidxComma,
    SlotListOptEmpty,
}

/// The grammar a parse uses (see the module documentation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Syntax {
    /// The language: its tokens (when the parser scans the source itself) and its rules.
    pub language: Language,
    /// `do stmt_block while (expr)`.
    pub do_while: bool,
    /// The seven `for` forms that leave out the initialiser, the test or the step (the full form is always there).
    pub for_omitted_parts: bool,
    /// The entries the state stack holds (the start state included); a push past it is a syntax error.
    pub max_parse_depth: usize,
    /// The field a `datablock = x;` line of an object block sets.
    pub datablock_slot: &'static [u8],
}

impl Syntax {
    /// Torque3D's grammar.
    pub const TORQUE3D: Syntax = Syntax {
        language: Language::Torque3d,
        do_while: true,
        for_omitted_parts: true,
        max_parse_depth: MAX_PARSE_DEPTH,
        datablock_slot: b"datablock",
    };

    /// TGE 1.4's grammar.
    pub const TGE14: Syntax = Syntax {
        language: Language::Tge14,
        ..Syntax::TORQUE3D
    };

    /// Which of the parser's table sets this syntax uses.
    fn table_index(&self) -> usize {
        usize::from(self.language == Language::Tge14) * 4
            + usize::from(self.do_while) * 2
            + usize::from(self.for_omitted_parts)
    }
}

impl Default for Syntax {
    fn default() -> Syntax {
        Syntax::TORQUE3D
    }
}

type RuleDef = (u8, Vec<Sym>, Option<u8>, Act);

fn t(x: u8) -> Sym {
    Sym::T(x)
}

fn n(x: u8) -> Sym {
    Sym::N(x)
}

/// The grammar of `syntax`, rule by rule in `CMDgram.y`'s order (only reduce/reduce conflicts would depend on it,
/// and there are none).
fn rules(syntax: &Syntax) -> Vec<RuleDef> {
    let mut r = torque3d_rules();
    if syntax.language == Language::Tge14 {
        r = tge14_rules(r);
    }
    if !syntax.do_while {
        r.retain(|rule| rule.3 != Act::DoWhile);
    }
    if !syntax.for_omitted_parts {
        r.retain(
            |rule| !matches!(rule.3, Act::For { init, test, step } if !(init && test && step)),
        );
    }
    r
}

/// TGE 1.4's rules from Torque3D's: its additions left out (`foreach`, `assert`, `singleton`, internal names,
/// parameters with defaults, optional datablock fields), a parameter a plain variable and a datablock's class and
/// name identifiers. The order of the rest is the same in both files.
fn tge14_rules(torque3d: Vec<RuleDef>) -> Vec<RuleDef> {
    use nt::*;
    use tok::*;
    let c = |ch: u8| t(char_token(ch));
    let added = [
        FOREACH_STMT,
        PARAM,
        INTSLOT_ACC,
        ASSERT_EXPR,
        SLOT_ASSIGN_LIST_OPT,
    ];
    torque3d
        .into_iter()
        .filter_map(|(lhs, rhs, prec, act)| {
            match act {
                Act::Object { internal: true, .. }
                | Act::Object {
                    singleton: true, ..
                } => return None,
                Act::Datablock => {
                    let rhs = vec![
                        t(DATABLOCK),
                        t(IDENT),
                        c(b'('),
                        t(IDENT),
                        n(PARENT_BLOCK),
                        c(b')'),
                        c(b'{'),
                        n(SLOT_ASSIGN_LIST),
                        c(b'}'),
                        c(b';'),
                    ];
                    return Some((lhs, rhs, prec, act));
                }
                _ => {}
            }
            if added.contains(&lhs) {
                return None;
            }
            let uses_added = rhs
                .iter()
                .any(|s| matches!(s, Sym::N(x) if added.contains(x)));
            if !uses_added {
                return Some((lhs, rhs, prec, act));
            }
            // A parameter list is a list of variables.
            (lhs == VAR_LIST).then(|| {
                let rhs = rhs
                    .into_iter()
                    .map(|s| if s == n(PARAM) { t(VAR) } else { s })
                    .collect();
                (lhs, rhs, prec, act)
            })
        })
        .collect()
}

/// Torque3D's grammar (see [`rules`]).
fn torque3d_rules() -> Vec<RuleDef> {
    use nt::*;
    use tok::*;
    let c = |ch: u8| t(char_token(ch));
    let mut r: Vec<RuleDef> = vec![
        (ACCEPT, vec![n(START)], None, Act::Accept),
        (START, vec![n(DECL_LIST)], None, Act::Start),
        (DECL_LIST, vec![], None, Act::DeclListEmpty),
        (
            DECL_LIST,
            vec![n(DECL_LIST), n(DECL)],
            None,
            Act::DeclListAppend,
        ),
        (DECL, vec![n(STMT)], None, Act::Pass),
        (DECL, vec![n(FN_DECL_STMT)], None, Act::DeclFn),
        (DECL, vec![n(PACKAGE_DECL)], None, Act::DeclPackage),
        (
            PACKAGE_DECL,
            vec![
                t(PACKAGE),
                t(IDENT),
                c(b'{'),
                n(FN_DECL_LIST),
                c(b'}'),
                c(b';'),
            ],
            None,
            Act::Package,
        ),
        (FN_DECL_LIST, vec![n(FN_DECL_STMT)], None, Act::FnListOne),
        (
            FN_DECL_LIST,
            vec![n(FN_DECL_LIST), n(FN_DECL_STMT)],
            None,
            Act::FnListAppend,
        ),
        (STATEMENT_LIST, vec![], None, Act::StmtListEmpty),
        (
            STATEMENT_LIST,
            vec![n(STATEMENT_LIST), n(STMT)],
            None,
            Act::StmtListAppend,
        ),
        (STMT, vec![n(IF_STMT)], None, Act::StmtOfStmt),
        (STMT, vec![n(WHILE_STMT)], None, Act::StmtOfStmt),
        (STMT, vec![n(FOR_STMT)], None, Act::StmtOfStmt),
        (STMT, vec![n(FOREACH_STMT)], None, Act::StmtOfStmt),
        (STMT, vec![n(DATABLOCK_DECL)], None, Act::StmtOfStmt),
        (STMT, vec![n(SWITCH_STMT)], None, Act::StmtOfStmt),
        (STMT, vec![t(BREAK), c(b';')], None, Act::Break),
        (STMT, vec![t(CONTINUE), c(b';')], None, Act::Continue),
        (STMT, vec![t(RETURN), c(b';')], None, Act::ReturnNone),
        (STMT, vec![t(RETURN), n(EXPR), c(b';')], None, Act::Return),
        (STMT, vec![n(EXPRESSION_STMT), c(b';')], None, Act::ExprStmt),
        (
            FN_DECL_STMT,
            vec![
                t(FUNCTION),
                t(IDENT),
                c(b'('),
                n(VAR_LIST_DECL),
                c(b')'),
                c(b'{'),
                n(STATEMENT_LIST),
                c(b'}'),
            ],
            None,
            Act::Function,
        ),
        (
            FN_DECL_STMT,
            vec![
                t(FUNCTION),
                t(IDENT),
                t(COLONCOLON),
                t(IDENT),
                c(b'('),
                n(VAR_LIST_DECL),
                c(b')'),
                c(b'{'),
                n(STATEMENT_LIST),
                c(b'}'),
            ],
            None,
            Act::FunctionNs,
        ),
        (VAR_LIST_DECL, vec![], None, Act::Empty),
        (VAR_LIST_DECL, vec![n(VAR_LIST)], None, Act::Pass),
        (VAR_LIST, vec![n(PARAM)], None, Act::VarListOne),
        (
            VAR_LIST,
            vec![n(VAR_LIST), c(b','), n(PARAM)],
            None,
            Act::VarListAppend,
        ),
        (
            PARAM,
            vec![t(VAR)],
            None,
            Act::Param {
                optional: false,
                default: false,
            },
        ),
        (
            PARAM,
            vec![t(VAR), c(b'?')],
            None,
            Act::Param {
                optional: true,
                default: false,
            },
        ),
        (
            PARAM,
            vec![t(VAR), c(b'='), n(EXPR)],
            None,
            Act::Param {
                optional: false,
                default: true,
            },
        ),
        (
            PARAM,
            vec![t(VAR), c(b'?'), c(b'='), n(EXPR)],
            None,
            Act::Param {
                optional: true,
                default: true,
            },
        ),
        (
            DATABLOCK_DECL,
            vec![
                t(DATABLOCK),
                n(CLASS_NAME_EXPR),
                c(b'('),
                n(EXPR),
                n(PARENT_BLOCK),
                c(b')'),
                c(b'{'),
                n(SLOT_ASSIGN_LIST_OPT),
                c(b'}'),
                c(b';'),
            ],
            None,
            Act::Datablock,
        ),
    ];
    // `new` with and without a block, the same with the name in brackets, then `singleton` with and without a
    // block.
    for (keyword, internal, block) in [
        (NEW, false, true),
        (NEW, false, false),
        (NEW, true, true),
        (NEW, true, false),
        (SINGLETON, false, true),
        (SINGLETON, false, false),
    ] {
        let mut rhs = vec![t(keyword), n(CLASS_NAME_EXPR), c(b'(')];
        if internal {
            rhs.extend([c(b'['), n(OBJECT_NAME), c(b']')]);
        } else {
            rhs.push(n(OBJECT_NAME));
        }
        rhs.extend([n(PARENT_BLOCK), n(OBJECT_ARGS), c(b')')]);
        if block {
            rhs.extend([c(b'{'), n(OBJECT_DECLARE_BLOCK), c(b'}')]);
        }
        let act = Act::Object {
            block,
            internal,
            singleton: keyword == SINGLETON,
        };
        r.push((OBJECT_DECL, rhs, None, act));
    }
    r.extend([
        (PARENT_BLOCK, vec![], None, Act::ParentNone),
        (PARENT_BLOCK, vec![c(b':'), t(IDENT)], None, Act::ParentName),
        (OBJECT_NAME, vec![], None, Act::ObjectNameEmpty),
        (OBJECT_NAME, vec![n(EXPR)], None, Act::Pass),
        (OBJECT_ARGS, vec![], None, Act::Empty),
        (
            OBJECT_ARGS,
            vec![c(b','), n(EXPR_LIST)],
            None,
            Act::ObjectArgs,
        ),
        (OBJECT_DECLARE_BLOCK, vec![], None, Act::BlockEmpty),
        (
            OBJECT_DECLARE_BLOCK,
            vec![n(SLOT_ASSIGN_LIST)],
            None,
            Act::BlockSlots,
        ),
        (
            OBJECT_DECLARE_BLOCK,
            vec![n(OBJECT_DECL_LIST)],
            None,
            Act::BlockDecls,
        ),
        (
            OBJECT_DECLARE_BLOCK,
            vec![n(SLOT_ASSIGN_LIST), n(OBJECT_DECL_LIST)],
            None,
            Act::BlockBoth,
        ),
        (
            OBJECT_DECL_LIST,
            vec![n(OBJECT_DECL), c(b';')],
            None,
            Act::ObjListOne,
        ),
        (
            OBJECT_DECL_LIST,
            vec![n(OBJECT_DECL_LIST), n(OBJECT_DECL), c(b';')],
            None,
            Act::ObjListAppend,
        ),
        (
            STMT_BLOCK,
            vec![c(b'{'), n(STATEMENT_LIST), c(b'}')],
            None,
            Act::StmtBlockBraces,
        ),
        (STMT_BLOCK, vec![n(STMT)], None, Act::StmtBlockOne),
        (
            SWITCH_STMT,
            vec![
                t(SWITCH),
                c(b'('),
                n(EXPR),
                c(b')'),
                c(b'{'),
                n(CASE_BLOCK),
                c(b'}'),
            ],
            None,
            Act::Switch,
        ),
        (
            SWITCH_STMT,
            vec![
                t(SWITCHSTR),
                c(b'('),
                n(EXPR),
                c(b')'),
                c(b'{'),
                n(CASE_BLOCK),
                c(b'}'),
            ],
            None,
            Act::SwitchStr,
        ),
        (
            CASE_BLOCK,
            vec![t(CASE), n(CASE_EXPR), c(b':'), n(STATEMENT_LIST)],
            None,
            Act::CaseOne,
        ),
        (
            CASE_BLOCK,
            vec![
                t(CASE),
                n(CASE_EXPR),
                c(b':'),
                n(STATEMENT_LIST),
                t(DEFAULT),
                c(b':'),
                n(STATEMENT_LIST),
            ],
            None,
            Act::CaseDefault,
        ),
        (
            CASE_BLOCK,
            vec![
                t(CASE),
                n(CASE_EXPR),
                c(b':'),
                n(STATEMENT_LIST),
                n(CASE_BLOCK),
            ],
            None,
            Act::CaseMore,
        ),
        (CASE_EXPR, vec![n(EXPR)], None, Act::CaseExprOne),
        (
            CASE_EXPR,
            vec![n(CASE_EXPR), t(CASEOR), n(EXPR)],
            None,
            Act::CaseExprOr,
        ),
        (
            IF_STMT,
            vec![t(IF), c(b'('), n(EXPR), c(b')'), n(STMT_BLOCK)],
            None,
            Act::If,
        ),
        (
            IF_STMT,
            vec![
                t(IF),
                c(b'('),
                n(EXPR),
                c(b')'),
                n(STMT_BLOCK),
                t(ELSE),
                n(STMT_BLOCK),
            ],
            None,
            Act::IfElse,
        ),
        (
            WHILE_STMT,
            vec![t(WHILE), c(b'('), n(EXPR), c(b')'), n(STMT_BLOCK)],
            None,
            Act::While,
        ),
        (
            WHILE_STMT,
            vec![t(DO), n(STMT_BLOCK), t(WHILE), c(b'('), n(EXPR), c(b')')],
            None,
            Act::DoWhile,
        ),
    ]);
    // The full form first, then the others in `CMDgram.y`'s order.
    for mask in [7u8, 3, 5, 1, 6, 2, 4, 0] {
        let (init, test, step) = (mask & 4 != 0, mask & 2 != 0, mask & 1 != 0);
        let mut rhs = vec![t(FOR), c(b'(')];
        if init {
            rhs.push(n(EXPR));
        }
        rhs.push(c(b';'));
        if test {
            rhs.push(n(EXPR));
        }
        rhs.push(c(b';'));
        if step {
            rhs.push(n(EXPR));
        }
        rhs.extend([c(b')'), n(STMT_BLOCK)]);
        r.push((FOR_STMT, rhs, None, Act::For { init, test, step }));
    }
    for (keyword, string) in [(FOREACH, false), (FOREACHSTR, true)] {
        r.push((
            FOREACH_STMT,
            vec![
                t(keyword),
                c(b'('),
                t(VAR),
                t(IN),
                n(EXPR),
                c(b')'),
                n(STMT_BLOCK),
            ],
            None,
            Act::Foreach(string),
        ));
    }
    r.extend([
        (EXPRESSION_STMT, vec![n(STMT_EXPR)], None, Act::Pass),
        (EXPR, vec![n(STMT_EXPR)], None, Act::Pass),
        (EXPR, vec![c(b'('), n(EXPR), c(b')')], None, Act::Paren),
    ]);
    let binary: [(u8, Act); 22] = [
        (char_token(b'^'), Act::IntBinary(BinaryOp::BitXor)),
        (char_token(b'%'), Act::IntBinary(BinaryOp::Mod)),
        (char_token(b'&'), Act::IntBinary(BinaryOp::BitAnd)),
        (char_token(b'|'), Act::IntBinary(BinaryOp::BitOr)),
        (char_token(b'+'), Act::FloatBinary(BinaryOp::Add)),
        (char_token(b'-'), Act::FloatBinary(BinaryOp::Sub)),
        (char_token(b'*'), Act::FloatBinary(BinaryOp::Mul)),
        (char_token(b'/'), Act::FloatBinary(BinaryOp::Div)),
        (char_token(b'<'), Act::IntBinary(BinaryOp::Lt)),
        (char_token(b'>'), Act::IntBinary(BinaryOp::Gt)),
        (GE, Act::IntBinary(BinaryOp::Ge)),
        (LE, Act::IntBinary(BinaryOp::Le)),
        (EQ, Act::IntBinary(BinaryOp::Eq)),
        (NE, Act::IntBinary(BinaryOp::Ne)),
        (OROR, Act::IntBinary(BinaryOp::Or)),
        (SHL, Act::IntBinary(BinaryOp::Shl)),
        (SHR, Act::IntBinary(BinaryOp::Shr)),
        (ANDAND, Act::IntBinary(BinaryOp::And)),
        (STREQ, Act::StrEq(true)),
        (STRNE, Act::StrEq(false)),
        (char_token(b'@'), Act::Concat),
        // placeholder replaced below: the ternary has its own rule
        (char_token(b'?'), Act::Ternary),
    ];
    // In CMDgram.y's order: ^ % & | + - * / , then unary -, *, the ternary, then < > >= <= == != || << >> &&
    // $= !$= @, then ! ~ and the atoms.
    for &(op, act) in binary.iter().take(8) {
        r.push((EXPR, vec![n(EXPR), t(op), n(EXPR)], None, act));
    }
    r.push((EXPR, vec![c(b'-'), n(EXPR)], Some(UNARY), Act::Neg));
    r.push((EXPR, vec![c(b'*'), n(EXPR)], Some(UNARY), Act::TagDeref));
    r.push((
        EXPR,
        vec![n(EXPR), c(b'?'), n(EXPR), c(b':'), n(EXPR)],
        None,
        Act::Ternary,
    ));
    for &(op, act) in binary.iter().skip(8).take(13) {
        r.push((EXPR, vec![n(EXPR), t(op), n(EXPR)], None, act));
    }
    r.extend([
        (EXPR, vec![c(b'!'), n(EXPR)], None, Act::Not),
        (EXPR, vec![c(b'~'), n(EXPR)], None, Act::BitNot),
        (EXPR, vec![t(TAGATOM)], None, Act::TagAtom),
        (EXPR, vec![t(FLTCONST)], None, Act::Float),
        (EXPR, vec![t(INTCONST)], None, Act::Int),
        (EXPR, vec![t(BREAK)], None, Act::BreakConst),
        (EXPR, vec![n(SLOT_ACC)], None, Act::SlotAccExpr),
        (EXPR, vec![n(INTSLOT_ACC)], None, Act::Pass),
        (EXPR, vec![t(IDENT)], None, Act::IdentConst),
        (EXPR, vec![t(STRATOM)], None, Act::StrAtom),
        (EXPR, vec![t(VAR)], None, Act::Var),
        (
            EXPR,
            vec![t(VAR), c(b'['), n(AIDX_EXPR), c(b']')],
            None,
            Act::VarIndex,
        ),
        (
            SLOT_ACC,
            vec![n(EXPR), c(b'.'), t(IDENT)],
            None,
            Act::SlotAcc,
        ),
        (
            SLOT_ACC,
            vec![n(EXPR), c(b'.'), t(IDENT), c(b'['), n(AIDX_EXPR), c(b']')],
            None,
            Act::SlotAccIndex,
        ),
        (
            INTSLOT_ACC,
            vec![n(EXPR), t(INTNAME), n(CLASS_NAME_EXPR)],
            None,
            Act::InternalSlot(false),
        ),
        (
            INTSLOT_ACC,
            vec![n(EXPR), t(INTNAMER), n(CLASS_NAME_EXPR)],
            None,
            Act::InternalSlot(true),
        ),
        (CLASS_NAME_EXPR, vec![t(IDENT)], None, Act::ClassIdent),
        (
            CLASS_NAME_EXPR,
            vec![c(b'('), n(EXPR), c(b')')],
            None,
            Act::ClassParen,
        ),
        (ASSIGN_OP_STRUCT, vec![t(PLUSPLUS)], None, Act::AssignInc),
        (ASSIGN_OP_STRUCT, vec![t(MINUSMINUS)], None, Act::AssignDec),
    ]);
    for (tok, op) in [
        (PLASN, AssignOp::Add),
        (MIASN, AssignOp::Sub),
        (MLASN, AssignOp::Mul),
        (DVASN, AssignOp::Div),
        (MODASN, AssignOp::Mod),
        (ANDASN, AssignOp::BitAnd),
        (XORASN, AssignOp::BitXor),
        (ORASN, AssignOp::BitOr),
        (SLASN, AssignOp::Shl),
        (SRASN, AssignOp::Shr),
    ] {
        r.push((
            ASSIGN_OP_STRUCT,
            vec![t(tok), n(EXPR)],
            None,
            Act::AssignOpOf(op),
        ));
    }
    r.extend([
        (STMT_EXPR, vec![n(FUNCALL_EXPR)], None, Act::Pass),
        (STMT_EXPR, vec![n(ASSERT_EXPR)], None, Act::Pass),
        (STMT_EXPR, vec![n(OBJECT_DECL)], None, Act::NewObject),
        (STMT_EXPR, vec![t(VAR), c(b'='), n(EXPR)], None, Act::Assign),
        (
            STMT_EXPR,
            vec![t(VAR), c(b'['), n(AIDX_EXPR), c(b']'), c(b'='), n(EXPR)],
            None,
            Act::AssignIndex,
        ),
        (
            STMT_EXPR,
            vec![t(VAR), n(ASSIGN_OP_STRUCT)],
            None,
            Act::AssignOpVar,
        ),
        (
            STMT_EXPR,
            vec![t(VAR), c(b'['), n(AIDX_EXPR), c(b']'), n(ASSIGN_OP_STRUCT)],
            None,
            Act::AssignOpIndex,
        ),
        (
            STMT_EXPR,
            vec![n(SLOT_ACC), n(ASSIGN_OP_STRUCT)],
            None,
            Act::SlotAssignOp,
        ),
        (
            STMT_EXPR,
            vec![n(SLOT_ACC), c(b'='), n(EXPR)],
            None,
            Act::SlotAssign,
        ),
        (
            STMT_EXPR,
            vec![n(SLOT_ACC), c(b'='), c(b'{'), n(EXPR_LIST), c(b'}')],
            None,
            Act::SlotAssignList,
        ),
        (
            FUNCALL_EXPR,
            vec![t(IDENT), c(b'('), n(EXPR_LIST_DECL), c(b')')],
            None,
            Act::Call,
        ),
        (
            FUNCALL_EXPR,
            vec![
                t(IDENT),
                t(COLONCOLON),
                t(IDENT),
                c(b'('),
                n(EXPR_LIST_DECL),
                c(b')'),
            ],
            None,
            Act::CallNs,
        ),
        (
            FUNCALL_EXPR,
            vec![
                n(EXPR),
                c(b'.'),
                t(IDENT),
                c(b'('),
                n(EXPR_LIST_DECL),
                c(b')'),
            ],
            None,
            Act::MethodCall,
        ),
        (
            ASSERT_EXPR,
            vec![t(ASSERT), c(b'('), n(EXPR), c(b')')],
            None,
            Act::Assert { message: false },
        ),
        (
            ASSERT_EXPR,
            vec![t(ASSERT), c(b'('), n(EXPR), c(b','), t(STRATOM), c(b')')],
            None,
            Act::Assert { message: true },
        ),
        (EXPR_LIST_DECL, vec![], None, Act::ListEmpty),
        (EXPR_LIST_DECL, vec![n(EXPR_LIST)], None, Act::Pass),
        (EXPR_LIST, vec![n(EXPR)], None, Act::ListOne),
        (
            EXPR_LIST,
            vec![n(EXPR_LIST), c(b','), n(EXPR)],
            None,
            Act::ListAppend,
        ),
        (SLOT_ASSIGN_LIST_OPT, vec![], None, Act::SlotListOptEmpty),
        (
            SLOT_ASSIGN_LIST_OPT,
            vec![n(SLOT_ASSIGN_LIST)],
            None,
            Act::Pass,
        ),
        (
            SLOT_ASSIGN_LIST,
            vec![n(SLOT_ASSIGN)],
            None,
            Act::SlotListOne,
        ),
        (
            SLOT_ASSIGN_LIST,
            vec![n(SLOT_ASSIGN_LIST), n(SLOT_ASSIGN)],
            None,
            Act::SlotListAppend,
        ),
        (
            SLOT_ASSIGN,
            vec![t(IDENT), c(b'='), n(EXPR), c(b';')],
            None,
            Act::SlotInit,
        ),
        (
            SLOT_ASSIGN,
            vec![t(DATABLOCK), c(b'='), n(EXPR), c(b';')],
            None,
            Act::SlotInitDatablock,
        ),
        (
            SLOT_ASSIGN,
            vec![
                t(IDENT),
                c(b'['),
                n(AIDX_EXPR),
                c(b']'),
                c(b'='),
                n(EXPR),
                c(b';'),
            ],
            None,
            Act::SlotInitIndex,
        ),
        (AIDX_EXPR, vec![n(EXPR)], None, Act::AidxOne),
        (
            AIDX_EXPR,
            vec![n(AIDX_EXPR), c(b','), n(EXPR)],
            None,
            Act::AidxComma,
        ),
    ]);
    r
}

/// The precedence declarations of `CMDgram.y`, lowest first (TGE 1.4's have no internal-name operators).
fn precedence(language: Language) -> Vec<Option<(u8, Assoc)>> {
    use tok::*;
    let mut prec = vec![None; COUNT];
    let levels: [(Assoc, Vec<u8>); 17] = [
        (Assoc::Left, vec![char_token(b'[')]),
        (
            Assoc::Right,
            vec![
                MODASN,
                ANDASN,
                XORASN,
                PLASN,
                MIASN,
                MLASN,
                DVASN,
                ORASN,
                SLASN,
                SRASN,
                char_token(b'='),
            ],
        ),
        (Assoc::Left, vec![char_token(b'?'), char_token(b':')]),
        (Assoc::Left, vec![OROR]),
        (Assoc::Left, vec![ANDAND]),
        (Assoc::Left, vec![char_token(b'|')]),
        (Assoc::Left, vec![char_token(b'^')]),
        (Assoc::Left, vec![char_token(b'&')]),
        (Assoc::Left, vec![EQ, NE]),
        (
            Assoc::Left,
            vec![char_token(b'<'), LE, char_token(b'>'), GE],
        ),
        (Assoc::Left, vec![char_token(b'@'), STREQ, STRNE]),
        (Assoc::Left, vec![SHL, SHR]),
        (Assoc::Left, vec![char_token(b'+'), char_token(b'-')]),
        (
            Assoc::Left,
            vec![char_token(b'*'), char_token(b'/'), char_token(b'%')],
        ),
        (
            Assoc::Right,
            vec![
                char_token(b'!'),
                char_token(b'~'),
                PLUSPLUS,
                MINUSMINUS,
                UNARY,
            ],
        ),
        (Assoc::Left, vec![char_token(b'.')]),
        (Assoc::Left, vec![INTNAME, INTNAMER]),
    ];
    let levels = levels
        .iter()
        .filter(|(_, toks)| language == Language::Torque3d || !toks.contains(&INTNAME));
    for (level, (assoc, toks)) in levels.enumerate() {
        for &t in toks {
            if let Some(slot) = prec.get_mut(usize::from(t)) {
                *slot = Some((level as u8 + 1, *assoc));
            }
        }
    }
    prec
}

struct Parser {
    tables: Tables,
    acts: Vec<Act>,
}

/// The parser of a syntax's rules (each set built once).
fn parser(syntax: &Syntax) -> Option<&'static Parser> {
    static PARSERS: [OnceLock<Option<Parser>>; 8] = [const { OnceLock::new() }; 8];
    PARSERS
        .get(syntax.table_index())?
        .get_or_init(|| {
            let list = rules(syntax);
            let grammar = lalr::Grammar {
                terminals: tok::COUNT,
                nonterminals: nt::COUNT,
                rules: list
                    .iter()
                    .map(|(lhs, rhs, prec, _)| Rule {
                        lhs: *lhs,
                        rhs: rhs.clone(),
                        prec: *prec,
                    })
                    .collect(),
                prec: precedence(syntax.language),
            };
            let tables = lalr::build(&grammar).ok()?;
            Some(Parser {
                tables,
                acts: list.iter().map(|r| r.3).collect(),
            })
        })
        .as_ref()
}

/// The grammar's tables, for the tests (state count, conflicts).
pub fn tables() -> Option<&'static Tables> {
    tables_for(&Syntax::TORQUE3D)
}

/// The tables of a syntax's grammar.
pub fn tables_for(syntax: &Syntax) -> Option<&'static Tables> {
    parser(syntax).map(|p| &p.tables)
}

pub use tok::char_token;

/// A value on the parser's stack.
enum V {
    None,
    Tok(Token),
    Expr(Expr),
    Exprs(Vec<Expr>),
    Stmt(Stmt),
    Stmts(Vec<Stmt>),
    Func(Function),
    Funcs(Vec<Function>),
    Param(Param),
    Params(Vec<Param>),
    Parent(Option<Name>),
    Slot(Expr, Name, Option<Expr>),
    AssignOp(AssignOp, Expr),
    Cases(Vec<Case>, Option<Vec<Stmt>>),
    Obj(ObjectDecl),
    Objs(Vec<ObjectDecl>),
    Block(Vec<Expr>, Vec<ObjectDecl>),
}

/// The result of parsing a file.
pub struct Parsed {
    /// The statements reduced before the end of the input or before the first syntax error.
    pub script: Script,
    pub diagnostics: Vec<Diagnostic>,
    pub syntax_error: bool,
    /// Work units: source bytes scanned plus parser steps.
    pub work: u64,
}

fn name_of(v: V) -> Name {
    match v {
        V::Tok(Token {
            value: TokenValue::Name(n),
            ..
        }) => n,
        _ => Name::from(&b""[..]),
    }
}

fn line_of(v: &V) -> u32 {
    match v {
        V::Tok(Token {
            value: TokenValue::Line(l),
            ..
        }) => *l,
        V::Tok(t) => t.line,
        _ => 0,
    }
}

/// A parameter: TGE 1.4's are plain variables (a token), Torque3D's a `param` value.
fn param_of(v: V) -> Param {
    match v {
        V::Param(p) => p,
        other => Param {
            name: name_of(other),
            default: None,
        },
    }
}

/// A datablock's class or name: an identifier (TGE 1.4) is a name expression at the declaration's line.
fn name_expr(v: V, line: u32) -> Expr {
    match v {
        V::Tok(_) => Expr {
            line,
            kind: ExprKind::Ident(name_of(v)),
        },
        other => expr_of(other),
    }
}

fn expr_of(v: V) -> Expr {
    match v {
        V::Expr(e) => e,
        _ => Expr {
            line: 0,
            kind: ExprKind::Int(0),
        },
    }
}

fn stmts_of(v: V) -> Vec<Stmt> {
    match v {
        V::Stmts(s) => s,
        V::Stmt(s) => vec![s],
        _ => Vec::new(),
    }
}

/// One reduction, for the tests: the rule (its index in [`rule_names`]), its left-hand side and length, and how
/// many tokens the parser had read (the lookahead included) when it reduced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reduction {
    pub rule: u16,
    pub lhs: &'static str,
    pub len: u16,
    pub tokens_read: u32,
}

/// A name for every rule of the grammar, by index (the left-hand side, the length and the semantic action),
/// for the tests' grammar coverage.
pub fn rule_names() -> Vec<String> {
    rule_names_for(&Syntax::TORQUE3D)
}

/// [`rule_names`] of a syntax's grammar.
pub fn rule_names_for(syntax: &Syntax) -> Vec<String> {
    rules(syntax)
        .iter()
        .map(|(lhs, rhs, _, act)| {
            format!(
                "{} <- {} symbols ({act:?})",
                NONTERMINAL_NAMES
                    .get(usize::from(*lhs))
                    .copied()
                    .unwrap_or("?"),
                rhs.len()
            )
        })
        .collect()
}

const NONTERMINAL_NAMES: [&str; nt::COUNT] = [
    "$accept",
    "start",
    "decl_list",
    "decl",
    "package_decl",
    "fn_decl_list",
    "statement_list",
    "stmt",
    "fn_decl_stmt",
    "var_list_decl",
    "var_list",
    "datablock_decl",
    "object_decl",
    "parent_block",
    "object_name",
    "object_args",
    "object_declare_block",
    "object_decl_list",
    "stmt_block",
    "switch_stmt",
    "case_block",
    "case_expr",
    "if_stmt",
    "while_stmt",
    "for_stmt",
    "expression_stmt",
    "expr",
    "slot_acc",
    "class_name_expr",
    "assign_op_struct",
    "stmt_expr",
    "funcall_expr",
    "expr_list_decl",
    "expr_list",
    "slot_assign_list",
    "slot_assign",
    "aidx_expr",
    "foreach_stmt",
    "param",
    "intslot_acc",
    "assert_expr",
    "slot_assign_list_opt",
];

/// The reductions of a parse, in order (the parse is the same as [`parse`]'s), and a last entry with the
/// left-hand side "ERROR" when it stopped at a syntax error.
pub fn reductions(source: &[u8]) -> Vec<Reduction> {
    reductions_with(&mut Lexer::new(source), &Syntax::TORQUE3D)
}

/// [`reductions`] of the tokens of `scanner` under `syntax`.
pub fn reductions_with(scanner: &mut (impl Scanner + ?Sized), syntax: &Syntax) -> Vec<Reduction> {
    let mut trace = Vec::new();
    parse_traced(scanner, None, syntax, Some(&mut trace));
    trace
}

/// Parses `source` (already cut at its first NUL). `file` names the file in the syntax error report.
pub fn parse(source: &[u8], file: Option<&[u8]>) -> Parsed {
    parse_with(source, file, &Syntax::TORQUE3D)
}

/// Parses `source` under `syntax`, scanning it with the syntax's language.
pub fn parse_with(source: &[u8], file: Option<&[u8]>, syntax: &Syntax) -> Parsed {
    parse_scanned(
        &mut Lexer::with_language(source, syntax.language),
        file,
        syntax,
    )
}

/// Parses the tokens of `scanner` under `syntax`: a dialect with a scanner of its own gives it here.
pub fn parse_scanned(
    scanner: &mut (impl Scanner + ?Sized),
    file: Option<&[u8]>,
    syntax: &Syntax,
) -> Parsed {
    parse_traced(scanner, file, syntax, None)
}

/// Where the parser's tokens come from: [`Lexer`], another dialect's scanner, or a list of token kinds
/// ([`parse_tokens`]).
pub trait Scanner {
    /// The next token, the end, or why the scan stopped.
    fn next_token(&mut self) -> Lexed;
    /// Tokens handed out so far (the end included).
    fn tokens(&self) -> u32;
    /// The line counter (the line a node made now gets).
    fn line(&self) -> u32;
    /// The token the parser sees at the end of the input.
    fn end_token(&self) -> Token;
    /// The text scanned (for the syntax error report).
    fn source(&self) -> &[u8];
    /// Where the scanner's reading had got to in [`source`](Scanner::source) (the report marks it).
    fn scan_index(&self) -> usize;
}

impl Scanner for Lexer<'_> {
    fn next_token(&mut self) -> Lexed {
        Lexer::next_token(self)
    }
    fn tokens(&self) -> u32 {
        Lexer::tokens(self)
    }
    fn line(&self) -> u32 {
        Lexer::line(self)
    }
    fn end_token(&self) -> Token {
        Lexer::end_token(self)
    }
    fn source(&self) -> &[u8] {
        Lexer::source(self)
    }
    fn scan_index(&self) -> usize {
        Lexer::scan_index(self)
    }
}

/// A list of token kinds, then the end of the input (as many times as it is asked for).
struct Kinds<'a> {
    kinds: &'a [u8],
    calls: u32,
}

impl Scanner for Kinds<'_> {
    fn next_token(&mut self) -> Lexed {
        let at = self.calls as usize;
        self.calls = self.calls.saturating_add(1);
        match self.kinds.get(at) {
            Some(&kind) => Lexed::Token(Token {
                kind,
                value: TokenValue::None,
                line: 1,
            }),
            None => Lexed::End,
        }
    }
    fn tokens(&self) -> u32 {
        self.calls
    }
    fn line(&self) -> u32 {
        1
    }
    fn end_token(&self) -> Token {
        Token {
            kind: tok::END,
            value: TokenValue::None,
            line: 1,
        }
    }
    fn source(&self) -> &[u8] {
        b""
    }
    fn scan_index(&self) -> usize {
        0
    }
}

/// For the tests: parses a list of token kinds (the [`tok`] numbers; a kind with no terminal is a token no
/// rule takes) followed by the end of the input, and returns the reductions as [`reductions`] does (the tree
/// is built from placeholder values and dropped). This lets a test drive the parser with token streams of its
/// own, to compare with a reference driver.
#[doc(hidden)]
pub fn parse_tokens(kinds: &[u8]) -> Vec<Reduction> {
    parse_tokens_with(kinds, &Syntax::TORQUE3D)
}

/// [`parse_tokens`] under `syntax`.
#[doc(hidden)]
pub fn parse_tokens_with(kinds: &[u8], syntax: &Syntax) -> Vec<Reduction> {
    let mut trace = Vec::new();
    let mut out = Parsed {
        script: Script::default(),
        diagnostics: Vec::new(),
        syntax_error: false,
        work: 0,
    };
    let Some(p) = parser(syntax) else {
        return trace;
    };
    let mut source = Kinds { kinds, calls: 0 };
    let stop = run(p, syntax, &mut source, &mut out, &mut Some(&mut trace));
    if stop.is_some() {
        trace.push(Reduction {
            rule: u16::MAX,
            lhs: "ERROR",
            len: 0,
            tokens_read: source.tokens(),
        });
    }
    trace
}

/// Why a parse stopped before the end.
enum Stop {
    /// A token no rule can take, or a full state stack ([`MAX_PARSE_DEPTH`]).
    Syntax,
    /// A `/*` comment without its end.
    UnterminatedComment,
    /// The scanner stopped with this message.
    Scanner(String),
    /// The compiler's own bounds: [`MAX_TREE_DEPTH`] and [`MAX_TOKENS`].
    TooBig(&'static str),
}

/// The parser's state stack holds this many entries (the start state included): bison's default stack bound
/// (`YYMAXDEPTH`). A push past it is a syntax error.
pub const MAX_PARSE_DEPTH: usize = 10_000;
/// The deepest tree the parser builds: the code generator recurses once per level, so this bounds its stack
/// (dropping a tree does not recurse).
pub const MAX_TREE_DEPTH: u32 = 4096;
/// The most tokens one file may have (about 16 MiB of dense source).
pub const MAX_TOKENS: u32 = 1 << 22;

/// How deep the value of a reduction nests: lists keep their deepest element's depth (they are walked, not
/// recursed), a `case a or b` chain and every other node add one level.
fn depth_of(act: Act, depths: &[u32]) -> u32 {
    let deepest = depths.iter().copied().max().unwrap_or(0);
    match act {
        Act::DeclListAppend
        | Act::FnListAppend
        | Act::StmtListAppend
        | Act::VarListAppend
        | Act::ObjListAppend
        | Act::ListAppend
        | Act::SlotListAppend
        | Act::Pass
        | Act::StmtOfStmt
        | Act::Paren
        | Act::ObjectArgs
        | Act::StmtBlockOne
        | Act::StmtBlockBraces
        | Act::AidxOne
        | Act::ListOne
        | Act::SlotListOne
        | Act::CaseExprOne
        | Act::SlotAcc
        | Act::SlotAccIndex
        | Act::AssignOpOf(_) => deepest,
        _ => deepest.saturating_add(1),
    }
}

fn parse_traced(
    lexer: &mut (impl Scanner + ?Sized),
    file: Option<&[u8]>,
    syntax: &Syntax,
    mut trace: Option<&mut Vec<Reduction>>,
) -> Parsed {
    let mut out = Parsed {
        script: Script::default(),
        diagnostics: Vec::new(),
        syntax_error: false,
        work: lexer.source().len() as u64,
    };
    let Some(p) = parser(syntax) else {
        out.syntax_error = true;
        out.diagnostics.push(Diagnostic {
            level: Level::Error,
            text: b"internal error: the parse tables could not be built".to_vec(),
        });
        return out;
    };
    let stop = run(p, syntax, lexer, &mut out, &mut trace);
    if let Some(stop) = stop {
        if let Some(trace) = trace {
            trace.push(Reduction {
                rule: u16::MAX,
                lhs: "ERROR",
                len: 0,
                tokens_read: lexer.tokens(),
            });
        }
        out.syntax_error = true;
        out.diagnostics.extend(syntax_error_report(
            lexer.source(),
            lexer.scan_index(),
            lexer.line(),
            file,
        ));
        let note = match stop {
            Stop::Syntax => None,
            Stop::UnterminatedComment => {
                Some("unexpected end of file inside block comment".to_string())
            }
            Stop::Scanner(why) => Some(why),
            Stop::TooBig(why) => Some(why.to_string()),
        };
        if let Some(note) = note {
            out.diagnostics.push(Diagnostic {
                level: Level::Error,
                text: note.into_bytes(),
            });
        }
    }
    out
}

/// The parser's loop: None when the input was accepted, else why it stopped.
fn run(
    p: &Parser,
    syntax: &Syntax,
    lexer: &mut (impl Scanner + ?Sized),
    out: &mut Parsed,
    trace: &mut Option<&mut Vec<Reduction>>,
) -> Option<Stop> {
    let max_depth = syntax.max_parse_depth;
    let mut stack: Vec<(u16, V, u32)> = vec![(0, V::None, 0)];
    let mut lookahead: Option<Token> = None;
    loop {
        out.work = out.work.saturating_add(1);
        let state = stack.last().map_or(0, |s| s.0);
        let rule = match p
            .tables
            .default_reduce
            .get(usize::from(state))
            .copied()
            .flatten()
        {
            Some(rule) => rule,
            None => {
                let token = match lookahead.take() {
                    Some(t) => t,
                    None => {
                        if lexer.tokens() >= MAX_TOKENS {
                            return Some(Stop::TooBig("the file has more than 4194304 tokens"));
                        }
                        match lexer.next_token() {
                            Lexed::Token(t) => t,
                            // A malformed escape: the scanner returns -1, which the parser reads as the end.
                            Lexed::End | Lexed::BadString => lexer.end_token(),
                            Lexed::UnterminatedComment => {
                                return Some(Stop::UnterminatedComment);
                            }
                            Lexed::Error(why) => return Some(Stop::Scanner(why)),
                        }
                    }
                };
                match p.tables.action(state, token.kind) {
                    Action::Shift(to) => {
                        if stack.len() + 1 > max_depth {
                            return Some(Stop::Syntax);
                        }
                        stack.push((to, V::Tok(token), 0));
                        continue;
                    }
                    Action::Reduce(rule) => {
                        lookahead = Some(token);
                        rule
                    }
                    Action::Accept => return None,
                    Action::Error => return Some(Stop::Syntax),
                }
            }
        };
        let len = usize::from(
            p.tables
                .rule_len
                .get(usize::from(rule))
                .copied()
                .unwrap_or(0),
        );
        let lhs = p
            .tables
            .rule_lhs
            .get(usize::from(rule))
            .copied()
            .unwrap_or(0);
        let act = p.acts.get(usize::from(rule)).copied().unwrap_or(Act::Pass);
        if let Some(trace) = trace.as_deref_mut() {
            trace.push(Reduction {
                rule,
                lhs: NONTERMINAL_NAMES
                    .get(usize::from(lhs))
                    .copied()
                    .unwrap_or("?"),
                len: len as u16,
                tokens_read: lexer.tokens(),
            });
        }
        let at = stack.len().saturating_sub(len);
        let mut depths = Vec::with_capacity(len);
        let values: Vec<V> = stack
            .drain(at..)
            .map(|(_, v, d)| {
                depths.push(d);
                v
            })
            .collect();
        let depth = depth_of(act, &depths);
        if depth > MAX_TREE_DEPTH {
            return Some(Stop::TooBig(
                "an expression or statement nested more than 4096 levels deep",
            ));
        }
        let value = reduce(act, values, lexer.line(), syntax, &mut out.script);
        let top = stack.last().map_or(0, |s| s.0);
        let to = p.tables.goto(top, lhs);
        if stack.len() + 1 > max_depth {
            return Some(Stop::Syntax);
        }
        stack.push((to, value, depth));
    }
}

/// The syntax error report: `<file> Line: <n> - Syntax error.`, then the text around the read position
/// with `##` on both sides of the byte the scanner had reached (at most 256 bytes before it unless a line break
/// comes first, at most 512 after it, a 1016-byte budget), a lone CR shown as LF. Text without a file name
/// (`eval`) only prints `Syntax error in input.`.
pub fn syntax_error_report(
    source: &[u8],
    scan_index: usize,
    line: u32,
    file: Option<&[u8]>,
) -> Vec<Diagnostic> {
    let Some(file) = file else {
        return vec![Diagnostic {
            level: Level::Error,
            text: b"Syntax error in input.".to_vec(),
        }];
    };
    let mut head = format!(
        "{} Line: {line} - Syntax error.",
        String::from_utf8_lossy(file)
    )
    .into_bytes();
    head.truncate(4095);
    let at = |i: usize| source.get(i).copied().unwrap_or(0);
    let mut back = 1usize;
    let mut fwd = 0usize;
    let mut n = 1usize;
    while n < 0x3f8 && back < scan_index {
        let c = at(scan_index - back);
        if (c == b'\r' || c == b'\n') && back > 0x100 {
            break;
        }
        n += 1;
        back += 1;
    }
    while n < 0x3f8 && fwd < 0x200 {
        let c = at(scan_index + fwd);
        if c == 0 {
            break;
        }
        if (c == b'\r' || c == b'\n') && fwd > 0x100 {
            break;
        }
        n += 1;
        fwd += 1;
    }
    back = back.saturating_sub(1);
    fwd = fwd.saturating_sub(1);
    let mut text = Vec::with_capacity(back + fwd + 5);
    text.extend((0..back).map(|k| at(scan_index - back + k)));
    text.extend_from_slice(b"##");
    text.push(at(scan_index));
    text.extend_from_slice(b"##");
    text.extend((0..fwd).map(|k| at(scan_index + 1 + k)));
    // The report is a C string: it ends at the first NUL (the halt byte at the end of the text is one).
    if let Some(nul) = text.iter().position(|&b| b == 0) {
        text.truncate(nul);
    }
    let mut i = 0;
    while let Some(&c) = text.get(i) {
        if c == b'\r'
            && text.get(i + 1) != Some(&b'\n')
            && let Some(slot) = text.get_mut(i)
        {
            *slot = b'\n';
        }
        i += 1;
    }
    vec![
        Diagnostic {
            level: Level::Error,
            text: head,
        },
        Diagnostic {
            level: Level::Warning,
            text: b">>> Some error context, with ## on sides of error halt:".to_vec(),
        },
        Diagnostic {
            level: Level::Error,
            text,
        },
        Diagnostic {
            level: Level::Warning,
            text: b">>> Error report complete.\n".to_vec(),
        },
    ]
}

fn binary(left: V, op: BinaryOp, right: V, line: u32) -> V {
    V::Expr(Expr {
        line,
        kind: ExprKind::Binary {
            op,
            left: Box::new(expr_of(left)),
            right: Box::new(expr_of(right)),
        },
    })
}

/// Runs a rule's action on the values of its right-hand side. `line` is the scanner's line counter now.
fn reduce(act: Act, values: Vec<V>, line: u32, syntax: &Syntax, script: &mut Script) -> V {
    let mut it = values.into_iter();
    let mut next = || it.next().unwrap_or(V::None);
    let expr = |kind: ExprKind| V::Expr(Expr { line, kind });
    match act {
        Act::Accept | Act::Start | Act::DeclListEmpty | Act::Empty => V::None,
        Act::DeclListAppend => {
            let _list = next();
            match next() {
                V::Stmt(s) => script.stmts.push(s),
                V::Stmts(s) => script.stmts.extend(s),
                _ => {}
            }
            V::None
        }
        Act::Pass | Act::StmtOfStmt => next(),
        Act::DeclFn => match next() {
            V::Func(f) => V::Stmt(Stmt {
                line: f.line,
                kind: StmtKind::Function(Box::new(f)),
            }),
            other => other,
        },
        Act::DeclPackage => next(),
        Act::Package => {
            let _ = next();
            let name = name_of(next());
            let _ = next();
            let functions = match next() {
                V::Funcs(f) => f,
                _ => Vec::new(),
            };
            V::Stmt(Stmt {
                line,
                kind: StmtKind::Package { name, functions },
            })
        }
        Act::FnListOne => match next() {
            V::Func(f) => V::Funcs(vec![f]),
            _ => V::Funcs(Vec::new()),
        },
        Act::FnListAppend => {
            let mut list = match next() {
                V::Funcs(f) => f,
                _ => Vec::new(),
            };
            if let V::Func(f) = next() {
                list.push(f);
            }
            V::Funcs(list)
        }
        Act::StmtListEmpty => V::Stmts(Vec::new()),
        Act::StmtListAppend => {
            let mut list = stmts_of(next());
            list.extend(stmts_of(next()));
            V::Stmts(list)
        }
        Act::Break => V::Stmt(Stmt {
            line,
            kind: StmtKind::Break,
        }),
        Act::Continue => V::Stmt(Stmt {
            line,
            kind: StmtKind::Continue,
        }),
        Act::ReturnNone => V::Stmt(Stmt {
            line,
            kind: StmtKind::Return(None),
        }),
        Act::Return => {
            let _ = next();
            let e = expr_of(next());
            V::Stmt(Stmt {
                line,
                kind: StmtKind::Return(Some(e)),
            })
        }
        Act::ExprStmt => {
            let e = expr_of(next());
            V::Stmt(Stmt {
                line: e.line,
                kind: StmtKind::Expr(e),
            })
        }
        Act::Function | Act::FunctionNs => {
            let _ = next();
            let first = name_of(next());
            let (namespace, name) = if act == Act::FunctionNs {
                let _ = next();
                (Some(first), name_of(next()))
            } else {
                (None, first)
            };
            let _ = next();
            let params = match next() {
                V::Params(p) => p,
                _ => Vec::new(),
            };
            let _ = next();
            let _ = next();
            let body = stmts_of(next());
            V::Func(Function {
                line,
                name,
                namespace,
                params,
                body: if body.is_empty() { None } else { Some(body) },
            })
        }
        Act::VarListOne => V::Params(vec![param_of(next())]),
        Act::VarListAppend => {
            let mut list = match next() {
                V::Params(p) => p,
                _ => Vec::new(),
            };
            let _ = next();
            list.push(param_of(next()));
            V::Params(list)
        }
        Act::Param { optional, default } => {
            let name = name_of(next());
            if optional {
                let _ = next();
            }
            let default = default.then(|| {
                let _ = next();
                expr_of(next())
            });
            V::Param(Param { name, default })
        }
        Act::Datablock => {
            let _ = next();
            let class = name_expr(next(), line);
            let _ = next();
            let name = name_expr(next(), line);
            let parent = match next() {
                V::Parent(p) => p,
                _ => None,
            };
            let _ = next();
            let _ = next();
            let fields = match next() {
                V::Exprs(f) => f,
                _ => Vec::new(),
            };
            let decl = ObjectDecl {
                line,
                class,
                name,
                parent,
                args: Vec::new(),
                fields,
                children: Vec::new(),
                datablock: true,
                internal_name: false,
                singleton: false,
            };
            V::Stmt(Stmt {
                line,
                kind: StmtKind::Expr(Expr {
                    line,
                    kind: ExprKind::New(Box::new(decl)),
                }),
            })
        }
        Act::Object {
            block,
            internal,
            singleton,
        } => {
            let _ = next();
            let class = expr_of(next());
            let _ = next();
            if internal {
                let _ = next();
            }
            let name = expr_of(next());
            if internal {
                let _ = next();
            }
            let parent = match next() {
                V::Parent(p) => p,
                _ => None,
            };
            let args = match next() {
                V::Exprs(a) => a,
                _ => Vec::new(),
            };
            let _ = next();
            let (fields, children) = if block {
                let _ = next();
                match next() {
                    V::Block(f, c) => (f, c),
                    _ => (Vec::new(), Vec::new()),
                }
            } else {
                (Vec::new(), Vec::new())
            };
            V::Obj(ObjectDecl {
                line,
                class,
                name,
                parent,
                args,
                fields,
                children,
                datablock: false,
                internal_name: internal,
                singleton,
            })
        }
        Act::ParentNone => V::Parent(None),
        Act::ParentName => {
            let _ = next();
            V::Parent(Some(name_of(next())))
        }
        Act::ObjectNameEmpty => expr(ExprKind::Str {
            text: Vec::new(),
            tagged: false,
        }),
        Act::ObjectArgs => {
            let _ = next();
            next()
        }
        Act::BlockEmpty => V::Block(Vec::new(), Vec::new()),
        Act::BlockSlots => match next() {
            V::Exprs(f) => V::Block(f, Vec::new()),
            _ => V::Block(Vec::new(), Vec::new()),
        },
        Act::BlockDecls => match next() {
            V::Objs(c) => V::Block(Vec::new(), c),
            _ => V::Block(Vec::new(), Vec::new()),
        },
        Act::BlockBoth => {
            let f = match next() {
                V::Exprs(f) => f,
                _ => Vec::new(),
            };
            let c = match next() {
                V::Objs(c) => c,
                _ => Vec::new(),
            };
            V::Block(f, c)
        }
        Act::ObjListOne => match next() {
            V::Obj(o) => V::Objs(vec![o]),
            _ => V::Objs(Vec::new()),
        },
        Act::ObjListAppend => {
            let mut list = match next() {
                V::Objs(o) => o,
                _ => Vec::new(),
            };
            if let V::Obj(o) = next() {
                list.push(o);
            }
            V::Objs(list)
        }
        Act::StmtBlockBraces => {
            let _ = next();
            V::Stmts(stmts_of(next()))
        }
        Act::StmtBlockOne => V::Stmts(stmts_of(next())),
        Act::Switch | Act::SwitchStr => {
            let _ = next();
            let _ = next();
            let test = Box::new(expr_of(next()));
            let _ = next();
            let _ = next();
            let (cases, default) = match next() {
                V::Cases(c, d) => (c, d),
                _ => (Vec::new(), None),
            };
            V::Stmt(Stmt {
                line: cases.first().map_or(line, |c| c.line),
                kind: StmtKind::Switch {
                    string: act == Act::SwitchStr,
                    test,
                    cases,
                    default,
                },
            })
        }
        Act::CaseOne | Act::CaseDefault | Act::CaseMore => {
            let case_line = line_of(&next());
            let values = match next() {
                V::Exprs(v) => v,
                _ => Vec::new(),
            };
            let _ = next();
            let body = stmts_of(next());
            let first = Case {
                line: case_line,
                values,
                body,
            };
            match act {
                Act::CaseOne => V::Cases(vec![first], None),
                Act::CaseDefault => {
                    let _ = next();
                    let _ = next();
                    V::Cases(
                        vec![first],
                        Some(stmts_of(next())).filter(|b| !b.is_empty()),
                    )
                }
                _ => match next() {
                    V::Cases(mut rest, default) => {
                        rest.insert(0, first);
                        V::Cases(rest, default)
                    }
                    _ => V::Cases(vec![first], None),
                },
            }
        }
        Act::CaseExprOne => V::Exprs(vec![expr_of(next())]),
        Act::CaseExprOr => {
            let mut list = match next() {
                V::Exprs(l) => l,
                _ => Vec::new(),
            };
            let _ = next();
            list.push(expr_of(next()));
            V::Exprs(list)
        }
        Act::If | Act::IfElse => {
            let if_line = line_of(&next());
            let _ = next();
            let test = Box::new(expr_of(next()));
            let _ = next();
            let then = stmts_of(next());
            let otherwise = if act == Act::IfElse {
                let _ = next();
                Some(stmts_of(next())).filter(|b| !b.is_empty())
            } else {
                None
            };
            V::Stmt(Stmt {
                line: if_line,
                kind: StmtKind::If {
                    test,
                    then,
                    otherwise,
                },
            })
        }
        Act::While => {
            let while_line = line_of(&next());
            let _ = next();
            let test = Box::new(expr_of(next()));
            let _ = next();
            let body = stmts_of(next());
            V::Stmt(Stmt {
                line: while_line,
                kind: StmtKind::Loop {
                    init: None,
                    test,
                    step: None,
                    body,
                    post_test: false,
                },
            })
        }
        Act::DoWhile => {
            let _ = next();
            let body = stmts_of(next());
            let while_line = line_of(&next());
            let _ = next();
            let test = Box::new(expr_of(next()));
            V::Stmt(Stmt {
                line: while_line,
                kind: StmtKind::Loop {
                    init: None,
                    test,
                    step: None,
                    body,
                    post_test: true,
                },
            })
        }
        Act::For { init, test, step } => {
            let for_line = line_of(&next());
            let _ = next();
            let mut part = |present: bool| {
                let value = present.then(|| Box::new(expr_of(next())));
                let _ = next();
                value
            };
            let init = part(init);
            let test = part(test).unwrap_or_else(|| {
                // A missing test is the constant 1, made at the loop's line.
                Box::new(Expr {
                    line: for_line,
                    kind: ExprKind::Int(1),
                })
            });
            let step = part(step);
            let body = stmts_of(next());
            V::Stmt(Stmt {
                line: for_line,
                kind: StmtKind::Loop {
                    init,
                    test,
                    step,
                    body,
                    post_test: false,
                },
            })
        }
        Act::Foreach(string) => {
            let foreach_line = line_of(&next());
            let _ = next();
            let variable = name_of(next());
            let _ = next();
            let container = Box::new(expr_of(next()));
            let _ = next();
            let body = stmts_of(next());
            V::Stmt(Stmt {
                line: foreach_line,
                kind: StmtKind::Foreach {
                    string,
                    variable,
                    container,
                    body,
                },
            })
        }
        Act::Paren => {
            let _ = next();
            next()
        }
        Act::IntBinary(op) | Act::FloatBinary(op) => {
            let l = next();
            let _ = next();
            binary(l, op, next(), line)
        }
        Act::Neg | Act::Not | Act::BitNot | Act::TagDeref => {
            let _ = next();
            let e = Box::new(expr_of(next()));
            expr(match act {
                Act::Neg => ExprKind::Unary {
                    op: UnaryOp::Neg,
                    expr: e,
                },
                Act::Not => ExprKind::Unary {
                    op: UnaryOp::Not,
                    expr: e,
                },
                Act::BitNot => ExprKind::Unary {
                    op: UnaryOp::BitNot,
                    expr: e,
                },
                _ => ExprKind::TagDeref(e),
            })
        }
        Act::Ternary => {
            let test = Box::new(expr_of(next()));
            let _ = next();
            let then = Box::new(expr_of(next()));
            let _ = next();
            let otherwise = Box::new(expr_of(next()));
            expr(ExprKind::Ternary {
                test,
                then,
                otherwise,
            })
        }
        Act::StrEq(equal) => {
            let left = Box::new(expr_of(next()));
            let _ = next();
            let right = Box::new(expr_of(next()));
            expr(ExprKind::StrEq { equal, left, right })
        }
        Act::Concat => {
            let left = Box::new(expr_of(next()));
            let kind = match next() {
                V::Tok(Token {
                    value: TokenValue::Char(c),
                    ..
                }) => match c {
                    b' ' => Concat::Space,
                    b'\t' => Concat::Tab,
                    b'\n' => Concat::NewLine,
                    _ => Concat::Plain,
                },
                _ => Concat::Plain,
            };
            let right = Box::new(expr_of(next()));
            expr(ExprKind::Concat { kind, left, right })
        }
        Act::TagAtom | Act::StrAtom => match next() {
            V::Tok(Token {
                value: TokenValue::Str(text),
                ..
            }) => expr(ExprKind::Str {
                text,
                tagged: act == Act::TagAtom,
            }),
            _ => expr(ExprKind::Str {
                text: Vec::new(),
                tagged: act == Act::TagAtom,
            }),
        },
        Act::Float => match next() {
            V::Tok(Token {
                value: TokenValue::Float(f),
                ..
            }) => expr(ExprKind::Float(f)),
            _ => expr(ExprKind::Float(0.0)),
        },
        Act::Int => match next() {
            V::Tok(Token {
                value: TokenValue::Int(i),
                ..
            }) => expr(ExprKind::Int(i)),
            _ => expr(ExprKind::Int(0)),
        },
        Act::BreakConst => expr(ExprKind::Ident(Name::from(&b"break"[..]))),
        Act::NewObject => match next() {
            // The node is the declaration's (made when `object_decl` was reduced).
            V::Obj(decl) => V::Expr(Expr {
                line: decl.line,
                kind: ExprKind::New(Box::new(decl)),
            }),
            other => other,
        },
        Act::SlotAccExpr => match next() {
            V::Slot(object, name, index) => expr(ExprKind::Field {
                object: Box::new(object),
                name,
                index: index.map(Box::new),
            }),
            _ => V::None,
        },
        Act::IdentConst => expr(ExprKind::Ident(name_of(next()))),
        Act::Var => expr(ExprKind::Var {
            name: name_of(next()),
            index: None,
        }),
        Act::VarIndex => {
            let name = name_of(next());
            let _ = next();
            let index = expr_of(next());
            expr(ExprKind::Var {
                name,
                index: Some(Box::new(index)),
            })
        }
        Act::SlotAcc | Act::SlotAccIndex => {
            let object = expr_of(next());
            let _ = next();
            let name = name_of(next());
            let index = if act == Act::SlotAccIndex {
                let _ = next();
                Some(expr_of(next()))
            } else {
                None
            };
            V::Slot(object, name, index)
        }
        Act::InternalSlot(recursive) => {
            let object = Box::new(expr_of(next()));
            let _ = next();
            let name = Box::new(expr_of(next()));
            expr(ExprKind::InternalField {
                object,
                name,
                recursive,
            })
        }
        Act::Assert { message } => {
            let _ = next();
            let _ = next();
            let test = Box::new(expr_of(next()));
            let message = if message {
                let _ = next();
                match next() {
                    V::Tok(Token {
                        value: TokenValue::Str(text),
                        ..
                    }) => text,
                    _ => Vec::new(),
                }
            } else {
                b"TorqueScript assert!".to_vec()
            };
            expr(ExprKind::Assert { test, message })
        }
        Act::ClassIdent => expr(ExprKind::Ident(name_of(next()))),
        Act::ClassParen => {
            let _ = next();
            next()
        }
        Act::AssignInc | Act::AssignDec => V::AssignOp(
            if act == Act::AssignInc {
                AssignOp::Increment
            } else {
                AssignOp::Decrement
            },
            Expr {
                line,
                kind: ExprKind::Float(1.0),
            },
        ),
        Act::AssignOpOf(op) => {
            let _ = next();
            V::AssignOp(op, expr_of(next()))
        }
        Act::Assign => {
            let name = name_of(next());
            let _ = next();
            let value = Box::new(expr_of(next()));
            expr(ExprKind::Assign {
                name,
                index: None,
                value,
            })
        }
        Act::AssignIndex => {
            let name = name_of(next());
            let _ = next();
            let index = expr_of(next());
            let _ = next();
            let _ = next();
            let value = Box::new(expr_of(next()));
            expr(ExprKind::Assign {
                name,
                index: Some(Box::new(index)),
                value,
            })
        }
        Act::AssignOpVar | Act::AssignOpIndex => {
            let name = name_of(next());
            let index = if act == Act::AssignOpIndex {
                let _ = next();
                let i = expr_of(next());
                let _ = next();
                Some(Box::new(i))
            } else {
                None
            };
            match next() {
                V::AssignOp(op, value) => expr(ExprKind::AssignOp {
                    name,
                    index,
                    op,
                    value: Box::new(value),
                }),
                _ => V::None,
            }
        }
        Act::SlotAssignOp => {
            let slot = next();
            let op = next();
            match (slot, op) {
                (V::Slot(object, name, index), V::AssignOp(op, value)) => {
                    expr(ExprKind::FieldAssignOp {
                        object: Box::new(object),
                        name,
                        index: index.map(Box::new),
                        op,
                        value: Box::new(value),
                    })
                }
                _ => V::None,
            }
        }
        Act::SlotAssign | Act::SlotAssignList => {
            let slot = next();
            let _ = next();
            // `obj.f = { a, b }` keeps only the list's first expression: the node compiles only that one.
            let value = if act == Act::SlotAssignList {
                let _ = next();
                match next() {
                    V::Exprs(list) => list.into_iter().next(),
                    _ => None,
                }
            } else {
                Some(expr_of(next()))
            };
            match slot {
                V::Slot(object, name, index) => expr(ExprKind::FieldAssign {
                    object: Some(Box::new(object)),
                    name,
                    index: index.map(Box::new),
                    value: Box::new(value.unwrap_or(Expr {
                        line,
                        kind: ExprKind::Int(0),
                    })),
                }),
                _ => V::None,
            }
        }
        Act::Call | Act::CallNs => {
            let first = name_of(next());
            let (namespace, name) = if act == Act::CallNs {
                let _ = next();
                (Some(first), name_of(next()))
            } else {
                (None, first)
            };
            let _ = next();
            let args = match next() {
                V::Exprs(a) => a,
                _ => Vec::new(),
            };
            expr(ExprKind::Call {
                namespace,
                name,
                args,
            })
        }
        Act::MethodCall => {
            let object = Box::new(expr_of(next()));
            let _ = next();
            let name = name_of(next());
            let _ = next();
            let args = match next() {
                V::Exprs(a) => a,
                _ => Vec::new(),
            };
            expr(ExprKind::MethodCall { object, name, args })
        }
        Act::ListEmpty | Act::SlotListOptEmpty => V::Exprs(Vec::new()),
        Act::ListOne | Act::SlotListOne => match next() {
            V::Expr(e) => V::Exprs(vec![e]),
            _ => V::Exprs(Vec::new()),
        },
        Act::ListAppend => {
            let mut list = match next() {
                V::Exprs(l) => l,
                _ => Vec::new(),
            };
            let _ = next();
            list.push(expr_of(next()));
            V::Exprs(list)
        }
        Act::SlotListAppend => {
            let mut list = match next() {
                V::Exprs(l) => l,
                _ => Vec::new(),
            };
            list.push(expr_of(next()));
            V::Exprs(list)
        }
        Act::SlotInit | Act::SlotInitDatablock | Act::SlotInitIndex => {
            let name = match act {
                Act::SlotInitDatablock => {
                    let _ = next();
                    Name::from(syntax.datablock_slot)
                }
                _ => name_of(next()),
            };
            let index = if act == Act::SlotInitIndex {
                let _ = next();
                let i = expr_of(next());
                let _ = next();
                Some(Box::new(i))
            } else {
                None
            };
            let _ = next();
            let value = Box::new(expr_of(next()));
            expr(ExprKind::FieldAssign {
                object: None,
                name,
                index,
                value,
            })
        }
        Act::AidxOne => next(),
        Act::AidxComma => {
            let left = Box::new(expr_of(next()));
            let _ = next();
            let right = Box::new(expr_of(next()));
            expr(ExprKind::CommaCat { left, right })
        }
    }
}
