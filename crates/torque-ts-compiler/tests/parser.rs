//! The parser: the trees it builds (precedence and associativity as the grammar's tables have them), the lines
//! its nodes get, its syntax error report, and its bounds.

mod common;

use std::time::Instant;

use torque_ts_bytecode::Dialect;
use torque_ts_compiler::ast::{
    AssignOp, BinaryOp, Concat, Expr, ExprKind, ObjectDecl, Script, Stmt, StmtKind, UnaryOp,
};
use torque_ts_compiler::parser::{
    MAX_PARSE_DEPTH, MAX_TOKENS, MAX_TREE_DEPTH, parse, syntax_error_report,
};
use torque_ts_compiler::{Level, MAX_SOURCE_BYTES, Purpose, compile};

fn t3d() -> Dialect {
    Dialect::torque3d()
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn binary(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Mod => "%",
        BinaryOp::BitAnd => "&",
        BinaryOp::BitOr => "|",
        BinaryOp::BitXor => "^",
        BinaryOp::Shl => "<<",
        BinaryOp::Shr => ">>",
        BinaryOp::Lt => "<",
        BinaryOp::Gt => ">",
        BinaryOp::Le => "<=",
        BinaryOp::Ge => ">=",
        BinaryOp::Eq => "==",
        BinaryOp::Ne => "!=",
        BinaryOp::And => "&&",
        BinaryOp::Or => "||",
    }
}

fn assign(op: AssignOp) -> &'static str {
    match op {
        AssignOp::Increment => "++",
        AssignOp::Decrement => "--",
        AssignOp::Add => "+=",
        AssignOp::Sub => "-=",
        AssignOp::Mul => "*=",
        AssignOp::Div => "/=",
        AssignOp::Mod => "%=",
        AssignOp::BitAnd => "&=",
        AssignOp::BitXor => "^=",
        AssignOp::BitOr => "|=",
        AssignOp::Shl => "<<=",
        AssignOp::Shr => ">>=",
    }
}

fn index(i: &Option<Box<Expr>>) -> String {
    i.as_ref()
        .map_or_else(String::new, |i| format!("[{}]", e(i)))
}

fn list(items: &[Expr]) -> String {
    items.iter().map(|x| format!(" {}", e(x))).collect()
}

/// An expression as an S-expression (lines left out).
fn e(x: &Expr) -> String {
    match &x.kind {
        ExprKind::Int(i) => i.to_string(),
        ExprKind::Float(f) => format!("{f:?}"),
        ExprKind::Str { text, tagged } => {
            let q = if *tagged { '\'' } else { '"' };
            format!("{q}{}{q}", lossy(text))
        }
        ExprKind::Ident(n) => lossy(n),
        ExprKind::Var { name, index: i } => format!("{}{}", lossy(name), index(i)),
        ExprKind::Unary { op, expr } => {
            let o = match op {
                UnaryOp::Neg => "-",
                UnaryOp::Not => "!",
                UnaryOp::BitNot => "~",
            };
            format!("({o} {})", e(expr))
        }
        ExprKind::Binary { op, left, right } => {
            format!("({} {} {})", binary(*op), e(left), e(right))
        }
        ExprKind::StrEq { equal, left, right } => {
            let o = if *equal { "$=" } else { "!$=" };
            format!("({o} {} {})", e(left), e(right))
        }
        ExprKind::Concat { kind, left, right } => {
            let o = match kind {
                Concat::Plain => "@",
                Concat::Space => "SPC",
                Concat::Tab => "TAB",
                Concat::NewLine => "NL",
            };
            format!("({o} {} {})", e(left), e(right))
        }
        ExprKind::CommaCat { left, right } => format!("(, {} {})", e(left), e(right)),
        ExprKind::Ternary {
            test,
            then,
            otherwise,
        } => format!("(? {} {} {})", e(test), e(then), e(otherwise)),
        ExprKind::Assign {
            name,
            index: i,
            value,
        } => format!("(= {}{} {})", lossy(name), index(i), e(value)),
        ExprKind::AssignOp {
            name,
            index: i,
            op,
            value,
        } => format!("({} {}{} {})", assign(*op), lossy(name), index(i), e(value)),
        ExprKind::Call {
            namespace,
            name,
            args,
        } => {
            let ns = namespace
                .as_ref()
                .map_or_else(String::new, |n| format!("{}::", lossy(n)));
            format!("(call {ns}{}{})", lossy(name), list(args))
        }
        ExprKind::MethodCall { object, name, args } => {
            format!("(.{}() {}{})", lossy(name), e(object), list(args))
        }
        ExprKind::Field {
            object,
            name,
            index: i,
        } => format!("(. {} {}{})", e(object), lossy(name), index(i)),
        ExprKind::FieldAssign {
            object,
            name,
            index: i,
            value,
        } => {
            let o = object.as_ref().map_or_else(|| "new".to_string(), |o| e(o));
            format!("(.= {o} {}{} {})", lossy(name), index(i), e(value))
        }
        ExprKind::FieldAssignOp {
            object,
            name,
            index: i,
            op,
            value,
        } => format!(
            "(.{} {} {}{} {})",
            assign(*op),
            e(object),
            lossy(name),
            index(i),
            e(value)
        ),
        ExprKind::InternalField {
            object,
            name,
            recursive,
        } => format!(
            "({} {} {})",
            if *recursive { "-->" } else { "->" },
            e(object),
            e(name)
        ),
        ExprKind::Assert { test, message } => format!("(assert {} {:?})", e(test), lossy(message)),
        ExprKind::TagDeref(x) => format!("(* {})", e(x)),
        ExprKind::New(decl) => object(decl),
    }
}

fn object(d: &ObjectDecl) -> String {
    let parent = d
        .parent
        .as_ref()
        .map_or_else(String::new, |p| format!(" : {}", lossy(p)));
    let children: String = d
        .children
        .iter()
        .map(|c| format!(" {}", object(c)))
        .collect();
    format!(
        "({} {} {}{parent}{} {{{} }}{children})",
        if d.datablock {
            "datablock"
        } else if d.singleton {
            "singleton"
        } else {
            "new"
        },
        e(&d.class),
        e(&d.name),
        list(&d.args),
        list(&d.fields)
    )
}

fn parsed(src: &str) -> Script {
    let p = parse(src.as_bytes(), Some(b"t.cs"));
    assert!(
        !p.syntax_error,
        "{src:?}: {:?}",
        common::texts(&p.diagnostics)
    );
    p.script
}

/// The expression of a source's only statement.
fn stmt_expr(src: &str) -> String {
    let script = parsed(src);
    assert_eq!(script.stmts.len(), 1, "{src}");
    match &script.stmts[0].kind {
        StmtKind::Expr(x) => e(x),
        other => panic!("{src}: not an expression statement: {other:?}"),
    }
}

/// The value assigned by `$x = <expr>;`.
fn value(expr: &str) -> String {
    let src = format!("$x = {expr};");
    let script = parsed(&src);
    match &script.stmts[0].kind {
        StmtKind::Expr(Expr {
            kind: ExprKind::Assign { value, .. },
            ..
        }) => e(value),
        other => panic!("{src}: {other:?}"),
    }
}

#[test]
fn precedence_and_associativity_follow_the_grammar() {
    for (src, tree) in [
        // `?:` is left-associative in the grammar (`%left '?' ':'`).
        ("%a ? %b : %c ? %d : %e", "(? (? %a %b %c) %d %e)"),
        ("%a ? %b : (%c ? %d : %e)", "(? %a %b (? %c %d %e))"),
        // `.` binds tighter than the unary operators, which bind tighter than the binary ones.
        ("-%a.b", "(- (. %a b))"),
        ("!%a == %b", "(== (! %a) %b)"),
        ("!%a @ %b", "(@ (! %a) %b)"),
        ("~%a.b[1]", "(~ (. %a b[1]))"),
        ("- - 1", "(- (- 1))"),
        ("- -1", "(- (- 1))"),
        ("-%a * %b", "(* (- %a) %b)"),
        ("-%a + %b", "(+ (- %a) %b)"),
        ("!%a && %b", "(&& (! %a) %b)"),
        ("~%a | %b", "(| (~ %a) %b)"),
        // Every binary level is left-associative.
        ("%a * %b / %c", "(/ (* %a %b) %c)"),
        ("%a << 1 << 2", "(<< (<< %a 1) 2)"),
        ("%a >> 1 << 2", "(<< (>> %a 1) 2)"),
        ("%a < %b <= %c", "(<= (< %a %b) %c)"),
        ("%a == %b != %c", "(!= (== %a %b) %c)"),
        ("%a & %b & %c", "(& (& %a %b) %c)"),
        ("%a ^ %b ^ %c", "(^ (^ %a %b) %c)"),
        ("%a | %b | %c", "(| (| %a %b) %c)"),
        ("%a && %b && %c", "(&& (&& %a %b) %c)"),
        ("%a || %b || %c", "(|| (|| %a %b) %c)"),
        ("%a $= %b $= %c", "($= ($= %a %b) %c)"),
        ("*%y", "(* %y)"),
        // `@`, `SPC`, `TAB`, `NL`, `$=` and `!$=` share a level, left-associative, below `<<` `>>` `+` `-`.
        ("%a @ %b SPC %c", "(SPC (@ %a %b) %c)"),
        ("%a TAB %b NL %c", "(NL (TAB %a %b) %c)"),
        ("%a $= %b @ %c", "(@ ($= %a %b) %c)"),
        ("%a @ %b !$= %c", "(!$= (@ %a %b) %c)"),
        ("%a @ %b + %c", "(@ %a (+ %b %c))"),
        ("%a + %b @ %c", "(@ (+ %a %b) %c)"),
        ("%a << 1 @ 2", "(@ (<< %a 1) 2)"),
        ("%a < %b @ %c", "(< %a (@ %b %c))"),
        // The arithmetic and comparison levels.
        ("1 + 2 * 3 - 4", "(- (+ 1 (* 2 3)) 4)"),
        ("1 - 2 - 3", "(- (- 1 2) 3)"),
        ("8 / 4 % 3 * 2", "(* (% (/ 8 4) 3) 2)"),
        ("%a << 1 + 2", "(<< %a (+ 1 2))"),
        ("%a < %b == %c > %d", "(== (< %a %b) (> %c %d))"),
        ("%a & %b == %c", "(& %a (== %b %c))"),
        ("%a | %b ^ %c & %d", "(| %a (^ %b (& %c %d)))"),
        ("%a || %b && %c", "(|| %a (&& %b %c))"),
        ("%a && %b || %c", "(|| (&& %a %b) %c)"),
        ("%a || %b ? 1 : 2", "(? (|| %a %b) 1 2)"),
        // An assignment is an expression: it takes everything to its right.
        ("1 + $y = 2", "(+ 1 (= $y 2))"),
        ("$y = 2 + 1", "(= $y (+ 2 1))"),
        ("%a ? $b = 1 : 2", "(? %a (= $b 1) 2)"),
        ("%a.b = %c.d = 1", "(.= %a b (.= %c d 1))"),
        ("$y += 1 + 2", "(+= $y (+ 1 2))"),
        ("$y[1]++", "(++ $y[1] 1.0)"),
        ("%o.f[2]--", "(.-- %o f[2] 1.0)"),
        // Postfix chains.
        ("%a.b[1].c", "(. (. %a b[1]) c)"),
        ("%a.b.c()", "(.c() (. %a b))"),
        ("%a.b().c", "(. (.b() %a) c)"),
        ("foo().bar(1, 2)", "(.bar() (call foo) 1 2)"),
        ("Ns::foo(%a, 1.5)", "(call Ns::foo %a 1.5)"),
        ("$a[1, 2, 3]", "$a[(, (, 1 2) 3)]"),
        ("%a.b[1, 2]", "(. %a b[(, 1 2)])"),
        ("(1).f", "(. 1 f)"),
        ("1.f", "(. 1 f)"),
        // Atoms.
        ("true + false", "(+ 1 0)"),
        ("0x10 + 1.5e1", "(+ 16 15.0)"),
        (
            "'tag' @ \"s\" @ Foo @ break",
            "(@ (@ (@ 'tag' \"s\") Foo) break)",
        ),
        ("new ScriptObject()", "(new ScriptObject \"\" { })"),
        (
            "new (%c)(%n : P, 1, 2) { a = 1; b[2] = 3; new X(); }",
            "(new %c %n : P 1 2 { (.= new a 1) (.= new b[2] 3) } (new X \"\" { }))",
        ),
    ] {
        assert_eq!(value(src), tree, "{src}");
    }
}

#[test]
fn statements_build_the_right_nodes() {
    for (src, tree) in [
        ("$a = $b = $c;", "(= $a (= $b $c))"),
        ("%a.b.c();", "(.c() (. %a b))"),
        ("%o.f = {1, 2};", "(.= %o f 1)"),
        ("%o.f = {%a @ %b};", "(.= %o f (@ %a %b))"),
        ("$a[1] -= 2;", "(-= $a[1] 2)"),
        ("%o.f <<= 1;", "(.<<= %o f 1)"),
        (
            "new X(N) { datablock = 1; };",
            "(new X N { (.= new datablock 1) })",
        ),
        (
            "datablock D(N : P) { a = 1; };",
            "(datablock D N : P { (.= new a 1) })",
        ),
        ("Parent::f(%this);", "(call Parent::f %this)"),
    ] {
        assert_eq!(stmt_expr(src), tree, "{src}");
    }
}

fn block(stmts: &[Stmt]) -> String {
    let items: String = stmts.iter().map(|s| format!(" {}", st(s))).collect();
    format!("{{{items} }}")
}

fn names(params: &[torque_ts_compiler::ast::Param]) -> String {
    params
        .iter()
        .map(|p| match &p.default {
            None => lossy(&p.name),
            Some(d) => format!("{}={}", lossy(&p.name), e(d)),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn function(f: &torque_ts_compiler::ast::Function) -> String {
    let ns = f
        .namespace
        .as_ref()
        .map_or_else(String::new, |n| format!("{}::", lossy(n)));
    let body = f
        .body
        .as_ref()
        .map_or_else(|| "none".to_string(), |b| block(b));
    format!(
        "(function {ns}{} ({}) {body})",
        lossy(&f.name),
        names(&f.params)
    )
}

/// A statement as an S-expression (lines left out).
fn st(s: &Stmt) -> String {
    match &s.kind {
        StmtKind::Break => "break".to_string(),
        StmtKind::Continue => "continue".to_string(),
        StmtKind::Return(None) => "(return)".to_string(),
        StmtKind::Return(Some(x)) => format!("(return {})", e(x)),
        StmtKind::Expr(x) => e(x),
        StmtKind::If {
            test,
            then,
            otherwise,
        } => match otherwise {
            None => format!("(if {} {})", e(test), block(then)),
            Some(o) => format!("(if {} {} else {})", e(test), block(then), block(o)),
        },
        StmtKind::Loop {
            init,
            test,
            step,
            body,
            post_test,
        } => {
            let part = |x: &Option<Box<Expr>>| x.as_deref().map_or_else(|| "-".to_string(), e);
            match (init, step) {
                _ if *post_test => format!("(do {} while {})", block(body), e(test)),
                (None, None) => format!("(while {} {})", e(test), block(body)),
                _ => format!(
                    "(for {} {} {} {})",
                    part(init),
                    e(test),
                    part(step),
                    block(body)
                ),
            }
        }
        StmtKind::Switch {
            string,
            test,
            cases,
            default,
        } => {
            let cases: String = cases
                .iter()
                .map(|c| format!(" (case{} : {})", list(&c.values), block(&c.body)))
                .collect();
            let default = default
                .as_ref()
                .map_or_else(String::new, |d| format!(" (default {})", block(d)));
            format!(
                "(switch{} {}{cases}{default})",
                if *string { "$" } else { "" },
                e(test)
            )
        }
        StmtKind::Foreach {
            string,
            variable,
            container,
            body,
        } => format!(
            "(foreach{} {} {} {})",
            if *string { "$" } else { "" },
            lossy(variable),
            e(container),
            block(body)
        ),
        StmtKind::Function(f) => function(f),
        StmtKind::Package { name, functions } => {
            let fs: String = functions
                .iter()
                .map(|f| format!(" {}", function(f)))
                .collect();
            format!("(package {}{fs})", lossy(name))
        }
    }
}

fn file_tree(src: &str) -> String {
    block(&parsed(src).stmts)
}

#[test]
fn declarations_and_control_flow_build_the_right_nodes() {
    for (src, tree) in [
        ("function f() {}", "{ (function f () none) }"),
        (
            "function f(%a, %b) { return %a; }",
            "{ (function f (%a %b) { (return %a) }) }",
        ),
        (
            "function Ns::f(%this) { $x = 1; }",
            "{ (function Ns::f (%this) { (= $x 1) }) }",
        ),
        (
            "package P { function f() {} function Ns::g(%a) { return; } };",
            "{ (package P (function f () none) (function Ns::g (%a) { (return) })) }",
        ),
        (
            "switch$ (%a) { case \"a\" or \"b\": $x = 1; case \"c\": default: $y = 2; }",
            "{ (switch$ %a (case \"a\" \"b\" : { (= $x 1) }) (case \"c\" : { }) (default { (= $y 2) })) }",
        ),
        (
            "switch (1) { case 1: $x = 1; case 2: }",
            "{ (switch 1 (case 1 : { (= $x 1) }) (case 2 : { })) }",
        ),
        (
            "if (%a) $x = 1; else if (%b) $x = 2; else $x = 3;",
            "{ (if %a { (= $x 1) } else { (if %b { (= $x 2) } else { (= $x 3) }) }) }",
        ),
        // The dangling `else` binds to the inner `if`.
        (
            "if (1) if (2) $a = 1; else $b = 2;",
            "{ (if 1 { (if 2 { (= $a 1) } else { (= $b 2) }) }) }",
        ),
        (
            "while (%i < 3) { $x++; }",
            "{ (while (< %i 3) { (++ $x 1.0) }) }",
        ),
        (
            "for (%i = 0; %i < 3; %i++) $x = 1;",
            "{ (for (= %i 0) (< %i 3) (++ %i 1.0) { (= $x 1) }) }",
        ),
        (
            "return; return 1 + 2; break; continue;",
            "{ (return) (return (+ 1 2)) break continue }",
        ),
        (
            "$a[1] = 2; %a = 1; $a[1, 2] = 3; $a[%i] += 1;",
            "{ (= $a[1] 2) (= %a 1) (= $a[(, 1 2)] 3) (+= $a[%i] 1) }",
        ),
        (
            "$x = 1; function f() {} $y = 2;",
            "{ (= $x 1) (function f () none) (= $y 2) }",
        ),
    ] {
        assert_eq!(file_tree(src), tree, "{src}");
    }
}

#[test]
fn the_grammar_has_one_conflict_the_dangling_else() {
    let t = torque_ts_compiler::parser::tables().expect("the tables build");
    assert_eq!(t.states(), STATES);
    assert_eq!(t.conflicts.len(), 1, "{:?}", t.conflicts);
    assert!(t.conflicts[0].contains("shift/reduce"), "{:?}", t.conflicts);
}

/// The states of the grammar's tables.
const STATES: usize = 365;

#[test]
fn do_while_and_for_without_parts_build_loops() {
    for (src, tree) in [
        (
            "do $x++; while ($x < 3)",
            "{ (do { (++ $x 1.0) } while (< $x 3)) }",
        ),
        ("do { } while (1)", "{ (do { } while 1) }"),
        ("for (;;) {}", "{ (while 1 { }) }"),
        ("for ($i = 0;;) {}", "{ (for (= $i 0) 1 - { }) }"),
        ("for (; $i;) {}", "{ (while $i { }) }"),
        ("for (;; $i++) {}", "{ (for - 1 (++ $i 1.0) { }) }"),
        ("for (; $i; $i++) {}", "{ (for - $i (++ $i 1.0) { }) }"),
        ("for ($i = 0; $i;) {}", "{ (for (= $i 0) $i - { }) }"),
        (
            "for ($i = 0;; $i++) {}",
            "{ (for (= $i 0) 1 (++ $i 1.0) { }) }",
        ),
    ] {
        assert_eq!(file_tree(src), tree, "{src}");
    }
    // `do`-`while` has no `;` after it: one more is a syntax error.
    assert!(parse(b"do {} while (1);", Some(b"t.cs")).syntax_error);
    // A `do`-`while` takes the line of its `while`, the constant 1 of a `for` the line of its `for`.
    let stmt = first(
        "do {
} while
(1)",
    );
    assert_eq!(stmt.line, 2);
    let stmt = first(
        "
for (;;) {}",
    );
    let StmtKind::Loop { test, .. } = &stmt.kind else {
        panic!()
    };
    assert_eq!((stmt.line, test.line), (2, 2));
    // `do` is a keyword: it is no function name.
    assert!(parse(b"do(1);", Some(b"t.cs")).syntax_error);
}

#[test]
fn torque3d_statements_and_expressions_build_their_nodes() {
    for (src, tree) in [
        (
            "foreach (%o in %set) %o.f();",
            "{ (foreach %o %set { (.f() %o) }) }",
        ),
        (
            "foreach$ ($w in \"a b\") { echo($w); break; }",
            "{ (foreach$ $w \"a b\" { (call echo $w) break }) }",
        ),
        ("assert(%x);", "{ (assert %x \"TorqueScript assert!\") }"),
        (
            "assert(%x > 1, \"too small\");",
            "{ (assert (> %x 1) \"too small\") }",
        ),
        ("%c = %o->child;", "{ (= %c (-> %o child)) }"),
        ("%c = %o-->(%n).f;", "{ (= %c (. (--> %o %n) f)) }"),
        ("%c = %a->b->c;", "{ (= %c (-> (-> %a b) c)) }"),
        (
            "singleton Material(M) { a = 1; };",
            "{ (singleton Material M { (.= new a 1) }) }",
        ),
        (
            "new ScriptObject([inner]);",
            "{ (new ScriptObject inner { }) }",
        ),
        (
            "function f(%a, %b ?, %c = 2, %d ? = %a @ 1) {}",
            "{ (function f (%a %b %c=2 %d=(@ %a 1)) none) }",
        ),
        (
            "datablock (%cls)(%name : Base) { };",
            "{ (datablock %cls %name : Base { }) }",
        ),
        (
            "datablock C(\"n\" @ 1) { datablock = x; };",
            "{ (datablock C (@ \"n\" 1) { (.= new datablock x) }) }",
        ),
    ] {
        assert_eq!(file_tree(src), tree, "{src}");
    }
    let decl = |src: &str| match first(src).kind {
        StmtKind::Expr(Expr {
            kind: ExprKind::New(ref d),
            ..
        }) => (d.internal_name, d.singleton),
        ref other => panic!("{other:?}"),
    };
    assert_eq!(decl("new C([n]);"), (true, false));
    assert_eq!(decl("singleton C(n);"), (false, true));
    assert_eq!(decl("new C(n);"), (false, false));
    // `in` and the new keywords are no names; `foreach` takes a variable, not an expression.
    for src in [
        &b"in = 1;"[..],
        b"function assert() {}",
        b"foreach (%a + 1 in %s) {}",
        b"foreach ($a[1] in %s) {}",
        b"singleton C([n]);",
    ] {
        assert!(parse(src, Some(b"t.cs")).syntax_error, "{:?}", lossy(src));
    }
}

#[test]
fn else_with_an_empty_block_and_an_empty_default_add_nothing() {
    let script = parsed("if (1) $x = 1; else {} if (1) $x = 1; else $y = 2;");
    let elses: Vec<bool> = script
        .stmts
        .iter()
        .map(|s| match &s.kind {
            StmtKind::If { otherwise, .. } => otherwise.is_some(),
            _ => panic!("not an if"),
        })
        .collect();
    assert_eq!(elses, [false, true]);
    let script = parsed(
        "switch (1) { case 1: $x = 1; default: } switch (1) { case 1: default: $y = 2; } switch (1) { case 1: }",
    );
    let defaults: Vec<bool> = script
        .stmts
        .iter()
        .map(|s| match &s.kind {
            StmtKind::Switch { default, .. } => default.is_some(),
            _ => panic!("not a switch"),
        })
        .collect();
    assert_eq!(defaults, [false, true, false]);
}

/// Runs `f` on a thread with the 1 MiB stack the VM documents for its callers: the parser must not need more.
fn on_a_one_mebibyte_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(f)
        .expect("a thread")
        .join()
        .expect("the parse did not overflow the stack or panic")
}

/// Left-nested chains, one shape per node kind that nests to the left: `start` and `op` n times, then `;`.
const CHAINS: [(&str, &str); 8] = [
    ("$x = 1", " + 1"),
    ("$x = 1", " @ 1"),
    ("$x = 1", " ? 1 : 1"),
    ("$x = 1", " $= 1"),
    ("$x = %a", ".f"),
    ("$x = %a", ".f[1]"),
    ("$x = %a", ".f()"),
    ("$x = %a", ".f(1, 2)"),
];

#[test]
fn deep_trees_parse_and_drop_on_a_one_mebibyte_stack() {
    assert_eq!(torque_ts_compiler::parser::MAX_TREE_DEPTH, 4096);
    let note = "an expression or statement nested more than 4096 levels deep";
    on_a_one_mebibyte_stack(move || {
        // Left-nested chains grow the tree, not the parser's stack; the tree is bounded at 4096 levels.
        for (start, op) in CHAINS {
            let chain = |n: usize| format!("{start}{};", op.repeat(n)).into_bytes();
            let p = parse(&chain(4000), Some(b"t.cs"));
            assert!(!p.syntax_error, "{op}: 4000 levels parse");
            drop(p);
            let p = parse(&chain(5000), Some(b"t.cs"));
            assert!(p.syntax_error, "{op}: 5000 levels are refused");
            assert_eq!(
                common::texts(&p.diagnostics).last().map(String::as_str),
                Some(note),
                "{op}"
            );
            drop(p);
        }
        // The boundary itself: n times ` + 1` after `$x = 1`.
        let chain = |n: usize| format!("$x = 1{};", " + 1".repeat(n)).into_bytes();
        assert!(!parse(&chain(4092), Some(b"t.cs")).syntax_error);
        assert!(parse(&chain(4093), Some(b"t.cs")).syntax_error);
        // The state stack (10000 entries) bounds the right-nested shapes, however deep the source nests; a
        // chain of unary operators reaches the tree's bound first. 1665 nested `if` blocks parse and drop here.
        for n in [10usize, 300, 4092, 4093, 9994, 9995, 100_000] {
            let src = format!("$x = {}1{};", "(".repeat(n), ")".repeat(n));
            assert_eq!(parse(src.as_bytes(), Some(b"t.cs")).syntax_error, n > 9994);
            let src = format!("$x = {}1;", "- ".repeat(n));
            assert_eq!(parse(src.as_bytes(), Some(b"t.cs")).syntax_error, n > 4092);
            let src = format!("{}1; {}", "new X() { a = ".repeat(n), "}".repeat(n));
            assert!(parse(src.as_bytes(), Some(b"t.cs")).syntax_error);
        }
        for n in [10usize, 1665, 1666, 5000] {
            let src = format!("{}$x = 1;{}", "if (1) {".repeat(n), "}".repeat(n));
            assert_eq!(parse(src.as_bytes(), Some(b"t.cs")).syntax_error, n > 1665);
        }
    });
}

/// Dropping a 4000-level tree by the compiler-generated glue needs about 800 KiB of stack in a debug build,
/// so the 1 MiB test above does not see a drop that recurses; this one gives the whole parse and
/// drop 192 KiB (the parse alone needs less than 128).
#[test]
fn dropping_a_deep_tree_does_not_recurse() {
    for (start, op) in CHAINS {
        let src = format!("{start}{};", op.repeat(4000)).into_bytes();
        let (error, stmts) = std::thread::Builder::new()
            .stack_size(192 << 10)
            .spawn(move || {
                let p = parse(&src, Some(b"t.cs"));
                (p.syntax_error, p.script.stmts.len())
            })
            .expect("a thread")
            .join()
            .expect("the parse and the drop of the tree fit 192 KiB");
        assert_eq!((error, stmts), (false, 1), "{op}");
    }
}

#[test]
fn hostile_megabyte_sources_end_in_a_report_or_a_parse_without_hanging() {
    const MIB: usize = 1 << 20;
    let repeat = |unit: &str| unit.repeat(MIB / unit.len()).into_bytes();
    let shapes: Vec<(&str, Vec<u8>, bool)> = vec![
        // (name, source, whether it is a syntax error)
        ("statements", repeat("$x = 1;\n"), false),
        ("calls", repeat("f(1, 2);"), false),
        ("empty functions", repeat("function f(%a) {}\n"), false),
        ("object fields", repeat("new X() { a = 1; };"), false),
        ("operators", repeat("1+"), true),
        ("opening parentheses", repeat("("), true),
        ("closing parentheses", repeat(")"), true),
        ("semicolons", repeat(";"), true),
        ("opening braces", repeat("{"), true),
        ("quotes", repeat("\""), true),
        (
            "unterminated strings",
            repeat("\"\\\"\\\"\\\"\\\"\\\"\\\"\\\"\\\"\n"),
            true,
        ),
        ("tags", repeat("'"), true),
        ("one identifier", vec![b'a'; MIB], true),
        (
            "one comment",
            [b"//".as_slice(), &vec![b'c'; MIB]].concat(),
            false,
        ),
        ("block comment openers", repeat("/*"), true),
        ("illegal bytes", repeat("#"), true),
        ("non-ASCII bytes", vec![0xE9; MIB], true),
        ("backslashes", repeat("\\"), true),
        ("variables", repeat("$a::b::c "), true),
        ("numbers", repeat("1.5e+3 0x1F "), true),
        // A NUL ends the source; a 0xFF byte is an illegal token.
        ("nul", vec![0; MIB], false),
        ("0xff", vec![0xFF; MIB], true),
    ];
    for (name, src, error) in shapes {
        let start = std::time::Instant::now();
        let (syntax_error, messages) = on_a_one_mebibyte_stack(move || {
            let p = parse(&src, Some(b"t.cs"));
            (p.syntax_error, p.diagnostics.len())
        });
        let secs = start.elapsed().as_secs_f64();
        assert_eq!(syntax_error, error, "{name}");
        // The four lines of the report, and a note when a bound of the compiler stopped the parse.
        assert_eq!(
            (4..=5).contains(&messages),
            error,
            "{name}: {messages} messages"
        );
        assert!(error || messages == 0, "{name}");
        assert!(secs < 60.0, "{name}: {secs} s");
    }
}

#[test]
fn statements_reduced_before_a_syntax_error_stay_in_the_script() {
    // yacc's default reductions run before an error is found, so `if (1) $x = 1;` and a function are statements
    // of the file when the `)` or `%x` after them is rejected; a statement the bad token interrupts is not.
    for src in [
        &b"$a = 1; if (1) $x = 1; )"[..],
        b"$a = 1; if (1) $x = 1; else $y = 2; )",
        b"$a = 1; function f() {} )",
        b"$a = 1; while (1) {} %x",
        b"$a = 1; for (%i = 0; %i < 1; %i++) {} )",
        b"$a = 1; switch (1) { case 1: } )",
        b"$a = 1; datablock D(N) { a = 1; }; )",
        b"$a = 1; package P { function f() {} }; )",
    ] {
        let p = parse(src, Some(b"t.cs"));
        assert!(p.syntax_error, "{:?}", lossy(src));
        assert_eq!(p.script.stmts.len(), 2, "{:?}", lossy(src));
    }
    let p = parse(b"$a = 1; $b = 2 )", Some(b"t.cs"));
    assert_eq!(p.script.stmts.len(), 1);
    let p = parse(b"$a = 1; function f() { $b = 2; ", Some(b"t.cs"));
    assert_eq!(p.script.stmts.len(), 1);
    // The same with no file name (`eval`): the statements are kept, the report is the short one.
    let p = parse(b"$a = 1; if (1) $x = 1; )", None);
    assert_eq!(p.script.stmts.len(), 2);
    assert_eq!(common::texts(&p.diagnostics), ["Syntax error in input."]);
}

#[test]
fn a_malformed_string_ends_the_input_and_what_came_before_is_the_script() {
    // The scanner returns -1 for it, which the parser reads as the end of the input: no error when the
    // statements before it are whole.
    let p = parse(b"$x = 1; \"\\x4\" $y = = 2;", Some(b"t.cs"));
    assert!(!p.syntax_error);
    assert_eq!(p.script.stmts.len(), 1);
    let p = parse(b"$x = 1; $y = \"\\c\" 2;", Some(b"t.cs"));
    assert!(
        p.syntax_error,
        "the input ends in the middle of a statement"
    );
    assert_eq!(p.script.stmts.len(), 1);
}

#[test]
fn a_syntax_error_has_the_four_message_report() {
    for src in [&b"$x = ;"[..], b"1;", b"do {} while (1);", b"function f( {"] {
        let p = parse(src, Some(b"f.cs"));
        assert!(p.syntax_error);
        let levels: Vec<Level> = p.diagnostics.iter().map(|d| d.level).collect();
        assert_eq!(
            levels,
            [Level::Error, Level::Warning, Level::Error, Level::Warning],
            "{:?}",
            lossy(src)
        );
    }
    // A clean parse has no messages.
    assert!(parse(b"$x = 1;", Some(b"f.cs")).diagnostics.is_empty());
}

#[test]
fn the_tokens_of_a_file_are_bounded_at_max_tokens() {
    // A parameter list builds the leanest tree per token. `function f(%a, ..., %a) {}` with n parameters
    // is 2n + 5 tokens and the end of the input one more: MAX_TOKENS tokens may be read, not one more.
    let max = torque_ts_compiler::parser::MAX_TOKENS;
    assert_eq!(max, 1 << 22);
    let list = |params: usize| {
        let mut src = b"function f(".to_vec();
        src.extend(b"%a,".repeat(params - 1));
        src.extend_from_slice(b"%a) {}");
        src
    };
    let fits = (max as usize - 6) / 2;
    // The end of the input is a token too: a file of MAX_TOKENS tokens of its own is one too many. `new X();`
    // (5 tokens) makes the count even.
    let exact = {
        let mut src = b"new X();".to_vec();
        src.extend(list((max as usize - 10) / 2));
        src
    };
    let (exact_error, exact_note) = on_a_one_mebibyte_stack(move || {
        let p = parse(&exact, Some(b"t.cs"));
        (
            p.syntax_error,
            common::texts(&p.diagnostics).last().cloned(),
        )
    });
    assert!(exact_error);
    assert_eq!(
        exact_note.as_deref(),
        Some("the file has more than 4194304 tokens")
    );
    let (ok, over) = on_a_one_mebibyte_stack(move || {
        let ok = parse(&list(fits), Some(b"t.cs"));
        let over = parse(&list(fits + 1), Some(b"t.cs"));
        (
            (ok.syntax_error, ok.script.stmts.len()),
            (
                over.syntax_error,
                common::texts(&over.diagnostics).last().cloned(),
            ),
        )
    });
    assert_eq!(ok, (false, 1));
    assert_eq!(
        over,
        (
            true,
            Some("the file has more than 4194304 tokens".to_string())
        )
    );
}

#[test]
fn flat_lists_are_not_depth_but_chains_that_nest_are() {
    let note = "an expression or statement nested more than 4096 levels deep";
    let n = 20_000;
    on_a_one_mebibyte_stack(move || {
        // Lists are walked, not recursed into: the depth of a list is its deepest element's.
        let list = |open: &str, item: &str, close: &str| {
            format!("{open}{}{item}{close}", format!("{item}, ").repeat(n)).into_bytes()
        };
        for (what, src) in [
            ("arguments", list("f(", "1", ");")),
            ("method arguments", list("%a.f(", "1", ");")),
            ("parameters", list("function f(", "%a", ") {}")),
            ("object arguments", list("new X(N, ", "1", ");")),
            (
                "statements",
                format!("function f() {{ {} }}", "$a = 1; ".repeat(n)).into_bytes(),
            ),
            (
                "block statements",
                format!("if (1) {{ {} }}", "$a = 1; ".repeat(n)).into_bytes(),
            ),
            (
                "object fields",
                format!("new X() {{ {} }};", "a = 1; ".repeat(n)).into_bytes(),
            ),
            (
                "object children",
                format!("new X() {{ {} }};", "new Y(); ".repeat(n)).into_bytes(),
            ),
            (
                "functions of a package",
                format!("package P {{ {} }};", "function f() {} ".repeat(n)).into_bytes(),
            ),
            ("top-level statements", "$a = 1;".repeat(n).into_bytes()),
        ] {
            let p = parse(&src, Some(b"t.cs"));
            assert!(
                !p.syntax_error,
                "{what}: {:?}",
                common::texts(&p.diagnostics)
            );
        }
        // A `case a or b or ...` chain and an index `[a, b, ...]` nest to the left, a level per element.
        let cases =
            |n: usize| format!("switch (1) {{ case {}1: }}", "1 or ".repeat(n)).into_bytes();
        assert!(!parse(&cases(4000), Some(b"t.cs")).syntax_error);
        let p = parse(&cases(5000), Some(b"t.cs"));
        assert_eq!(
            common::texts(&p.diagnostics).last().map(String::as_str),
            Some(note)
        );
        let index = |n: usize| format!("$x = $a[{}1];", "1, ".repeat(n)).into_bytes();
        assert!(!parse(&index(4000), Some(b"t.cs")).syntax_error);
        let p = parse(&index(5000), Some(b"t.cs"));
        assert_eq!(
            common::texts(&p.diagnostics).last().map(String::as_str),
            Some(note)
        );
    });
}

#[test]
fn the_work_of_a_parse_is_its_source_bytes_and_its_parser_steps() {
    // The VM charges `work` to the step budget: bytes scanned plus one per parser step.
    let src = b"$x = 1;";
    let work = parse(src, Some(b"t.cs")).work;
    assert!(work > src.len() as u64, "{work}");
    let more = parse(&src.repeat(100), Some(b"t.cs")).work;
    assert!(more > 100 * work / 2, "{more} vs {work}");
    assert!(more < 100 * work, "{more} vs {work}");
    assert!(
        parse(b"", Some(b"t.cs")).work > 0,
        "an empty file still takes steps"
    );
}

fn stmt_lines(src: &str) -> Vec<u32> {
    parsed(src).stmts.iter().map(|s| s.line).collect()
}

fn first(src: &str) -> Stmt {
    parsed(src).stmts.into_iter().next().expect("a statement")
}

#[test]
fn statements_get_their_lines() {
    // `if`, `while` and `for` take their keyword's line; a `switch` the line of its first `case`.
    assert_eq!(stmt_lines("\n\nif (1)\n{\n$x = 1;\n}\n"), [3]);
    assert_eq!(stmt_lines("if\n(1)\n$x = 1;\nelse\n$y = 2;"), [1]);
    assert_eq!(stmt_lines("\nwhile (1)\n{\n}\n"), [2]);
    assert_eq!(stmt_lines("\n\n\nfor ($i = 0;\n$i < 1;\n$i++)\n{}\n"), [4]);
    assert_eq!(
        stmt_lines("switch (1)\n{\n\ncase 1:\n$x = 1;\ncase 2:\n}\n"),
        [4]
    );
    let stmt = first("switch (1)\n{\ncase 1:\n$x = 1;\n\ncase 2 or\n3:\n}\n");
    let StmtKind::Switch { cases, .. } = &stmt.kind else {
        panic!("not a switch")
    };
    assert_eq!(cases.iter().map(|c| c.line).collect::<Vec<_>>(), [3, 6]);
    // An expression statement takes the line of its node, made when its rule is reduced: after the
    // lookahead that decided it when the rule needed one, else at its last token.
    assert_eq!(
        stmt_lines("%x = 1\n;"),
        [2],
        "`= expr` needs the `;` to end"
    );
    assert_eq!(stmt_lines("foo(1)\n;"), [1], "a call ends at its `)`");
    assert_eq!(stmt_lines("foo(1,\n2);"), [2]);
    assert_eq!(stmt_lines("%x\n=\n1;"), [3]);
    assert_eq!(stmt_lines("%x++\n;"), [1], "`++` ends the statement");
    assert_eq!(stmt_lines("%o.f = {1,\n2\n};"), [3]);
    assert_eq!(
        stmt_lines("new X()\n;"),
        [2],
        "`new X()` might go on with a block"
    );
    assert_eq!(stmt_lines("new X() {\n}\n;"), [2]);
    // A `return`, `break` or `continue` is made at its `;`.
    assert_eq!(stmt_lines("return\n%x\n;"), [3]);
    assert_eq!(stmt_lines("break\n;\ncontinue;"), [2, 3]);
    // A function is made at its closing brace, a package at its `;`.
    assert_eq!(stmt_lines("function f()\n{\n}\n"), [3]);
    assert_eq!(stmt_lines("package P {\nfunction f() {}\n}\n;"), [4]);
    // Inside an expression: a node is made when the token after it is read.
    let stmt = first("%v = %a\n+ 1\n;");
    let StmtKind::Expr(x) = &stmt.kind else {
        panic!()
    };
    assert_eq!(x.line, 3);
    let ExprKind::Assign { value, .. } = &x.kind else {
        panic!()
    };
    assert_eq!(value.line, 3, "the `+` node is made once the `;` is read");
    let ExprKind::Binary { left, right, .. } = &value.kind else {
        panic!()
    };
    assert_eq!(
        (left.line, right.line),
        (2, 2),
        "`%a` is made once the `+` is read, `1` at once"
    );
    // Line counting: only LF counts; CRLF and a lone CR do not add lines of their own.
    assert_eq!(stmt_lines("$a = 1;\r\n$b = 2;\r\n\r\n$c = 3;"), [1, 2, 4]);
    assert_eq!(stmt_lines("$a = 1;\r$b = 2;\r\r$c = 3;"), [1, 1, 1]);
    assert_eq!(stmt_lines("$a = 1; // x\n$b = \"\\n\";\n"), [1, 2]);
}

/// A syntax error report's halt line (the text between the two messages).
fn report(src: &[u8]) -> (Vec<String>, Vec<u8>) {
    let p = parse(src, Some(b"t.cs"));
    assert!(p.syntax_error, "{:?} parses", lossy(src));
    let levels: Vec<Level> = p.diagnostics.iter().map(|d| d.level).collect();
    assert_eq!(
        levels[..4],
        [Level::Error, Level::Warning, Level::Error, Level::Warning]
    );
    assert_eq!(
        p.diagnostics[1].text,
        b">>> Some error context, with ## on sides of error halt:"
    );
    assert_eq!(p.diagnostics[3].text, b">>> Error report complete.\n");
    let texts = common::texts(&p.diagnostics);
    (texts, p.diagnostics[2].text.clone())
}

#[test]
fn syntax_error_reports_halt_where_the_scanner_stopped() {
    // (source, line, context): the halt position is the scanner's read position after it returned the
    // token the parser rejected (the end of the line it read last); the context shows it between `##`s.
    let cases: &[(&[u8], u32, &[u8])] = &[
        // `1.e5` is `1`, `.`, `e5`: the field `e5` of 1, which is no statement; the scanner has read the `;`.
        (b"1.e5;", 1, b".e5;##"),
        // `!` could start `!$=`: the scanner reads the `$` and `x` before it returns `!`.
        (b"!$x;", 1, b"$x;##"),
        // An unterminated string: the quote is an illegal token, read up to the line's end and one byte more.
        (b"$x = \"abc\n$y = 1;", 1, b"x = \"abc\n##$##y = 1;"),
        (b"$x = 'abc", 1, b"x = 'abc##"),
        (b"$x = 1 }", 1, b"x = 1 }##"),
        (b"\n\n  $x = ;\n", 3, b"\n  $x = ;\n##"),
        (
            b"$x = 1;\r\n$y = = 2;\r\n$z = 3;",
            2,
            b"x = 1;\r\n$y = = 2;\r\n##$##z = 3;",
        ),
        // A lone CR does not count as a line (nor ends one) and shows as a LF.
        (b"$x = 1;\r$y = = 2;\r", 1, b"x = 1;\n$y = = 2;\n##"),
        // No `;` after a `do`-`while`, no `namespace`, no end of a `/*` comment.
        (b"do { } while (1);", 1, b"o { } while (1);##"),
        (b"namespace X {}", 1, b"amespace X {}##"),
        (b"$x = 1; /* c", 1, b"x = 1; /* c##"),
        (b"$x = 1 2", 1, b"x = 1 2##"),
        (b"if (1) $x = 1; )", 1, b"f (1) $x = 1; )##"),
        // A malformed escape: the scanner returns -1, the parser's end of input.
        (b"$x = \"\\x4\" ;", 1, b"x = \"\\x4\" ;##"),
        (
            b"function f() {\n  %a = 1;\n  %b = %a +;\n}\n",
            3,
            b"unction f() {\n  %a = 1;\n  %b = %a +;\n##}##\n",
        ),
    ];
    for &(src, line, context) in cases {
        let (texts, got) = report(src);
        assert_eq!(
            texts[0],
            format!("t.cs Line: {line} - Syntax error."),
            "{:?}",
            lossy(src)
        );
        assert_eq!(lossy(&got), lossy(context), "{:?}", lossy(src));
    }
    // Long contexts: at most 256 bytes back unless a line break comes first, 512 forward, 1016 in all.
    let mut src = b"// ".to_vec();
    src.extend([b'c'; 300]);
    src.extend_from_slice(b"\n$x = = 1;\n// ");
    src.extend([b'd'; 600]);
    let (texts, got) = report(&src);
    assert_eq!(texts[0], "t.cs Line: 2 - Syntax error.");
    assert_eq!(
        got,
        [&src[1..314], b"##", &src[314..315], b"##", &src[315..826]].concat()
    );
    let mut src = b"$x = 1;".repeat(60);
    src.push(b'\n');
    src.extend(b"$y = 2;".repeat(60));
    src.extend_from_slice(b" ) ");
    src.extend(b"$z = 3;".repeat(100));
    let (texts, got) = report(&src);
    assert_eq!(texts[0], "t.cs Line: 2 - Syntax error.");
    // The second line is read to the end of the input: the budget goes to the bytes before it.
    assert_eq!(got, [&src[src.len() - 1015..], b"##"].concat());
    let src = format!("$x = {};", "1 + ".repeat(100)).into_bytes();
    let (_, got) = report(&src);
    assert_eq!(got, [&src[1..], b"##"].concat());
    // Text with no file name (`eval`) only says so.
    let p = parse(b"$x = ;", None);
    assert_eq!(common::texts(&p.diagnostics), ["Syntax error in input."]);
}

/// The report's context text around the read position `halt`, split at its `##`s: (before, halt byte, after).
fn context(src: &[u8], halt: usize) -> (Vec<u8>, u8, Vec<u8>) {
    let report = syntax_error_report(src, halt, 1, Some(b"t.cs"));
    let got = &report[2].text;
    let at = got.windows(2).position(|w| w == b"##").expect("a first ##");
    let (before, rest) = got.split_at(at);
    let rest = &rest[2..];
    assert_eq!(&rest[1..3], b"##");
    (before.to_vec(), rest[0], rest[3..].to_vec())
}

/// A lone CR shows as LF in the report.
fn shown(b: &[u8]) -> Vec<u8> {
    b.iter()
        .map(|&c| if c == b'\r' { b'\n' } else { c })
        .collect()
}

#[test]
fn the_error_context_window_stops_at_a_line_break_only_beyond_256_bytes() {
    // The halt byte is the `2` of `= = 2`. A line break 256 bytes or less from it does not stop the window; one
    // 257 or more bytes away does, and the break itself is not shown.
    for brk in [b'\n', b'\r'] {
        for d in [255usize, 256, 257, 258] {
            // A break `d` bytes before the halt byte.
            let mut src = vec![b'$', b'a', b' ', b'=', b' ', b'1', b';', brk];
            src.extend(vec![b' '; d - 8]);
            src.extend_from_slice(b"$y = = 2;\n$z = 3;");
            let halt = d + 7;
            assert_eq!(src[halt], b'2');
            let (before, h, after) = context(&src, halt);
            let start = if d > 256 { halt - (d - 1) } else { 1 };
            assert_eq!(before, shown(&src[start..halt]), "back {d} {brk}");
            assert_eq!(h, b'2');
            assert_eq!(after, shown(&src[halt + 1..]), "back {d}");
            // A break `d` bytes after the halt byte.
            let mut src = b"$y = = 2".to_vec();
            src.extend(vec![b' '; d - 1]);
            src.push(brk);
            src.extend_from_slice(b"$z = 3;");
            let halt = 7;
            assert_eq!(src[halt], b'2');
            let (before, h, after) = context(&src, halt);
            let end = if d > 256 { halt + d } else { src.len() };
            assert_eq!(before, shown(&src[1..halt]), "forward {d}");
            assert_eq!(h, b'2');
            assert_eq!(after, shown(&src[halt + 1..end]), "forward {d} {brk}");
        }
    }
}

#[test]
fn the_error_context_window_has_a_budget_of_1016_bytes_and_512_ahead() {
    // No line breaks: the bytes before the halt byte fill the whole budget (the halt byte is one of the 1016,
    // nothing is left for the bytes after it) ...
    let mut src = b"$x = 1;".repeat(300);
    src.extend_from_slice(b"$y = = 2; $z = 3;");
    let halt = 300 * 7 + 7;
    assert_eq!(src[halt], b'2');
    let (before, h, after) = context(&src, halt);
    assert_eq!(before, &src[halt - 1015..halt]);
    assert_eq!((h, after.len()), (b'2', 0));
    // ... a window that starts nearer takes the rest of the budget after the halt byte ...
    let mut src = b"$x = 1;".repeat(85);
    src.extend_from_slice(b"$y = = 2;");
    src.extend(b"$z = 3;".repeat(200));
    let halt = 85 * 7 + 7;
    assert_eq!(src[halt], b'2');
    let (before, h, after) = context(&src, halt);
    assert_eq!(before, &src[1..halt]);
    assert_eq!(h, b'2');
    assert_eq!(before.len() + 1 + after.len(), 1015);
    assert_eq!(after, &src[halt + 1..halt + 1 + after.len()]);
    // ... and at most 512 bytes (511 after the halt byte) when the budget allows.
    let mut src = b"$y = = 2;".to_vec();
    src.extend(b"$z = 3;".repeat(200));
    let (before, _, after) = context(&src, 7);
    assert_eq!((before.len(), after.len()), (6, 511));
}

#[test]
fn a_0xff_byte_is_text_and_a_nul_ends_the_input() {
    let p = parse(b"$x = 1;\xff$y = 2;", Some(b"t.cs"));
    assert!(p.syntax_error, "an illegal token");
    assert_eq!(p.script.stmts.len(), 1);
    // A NUL ends it too (the source is a C string).
    let out = compile(b"$x = 1;\0$y = = 2;", Some(b"t.cs"), Purpose::Check, &t3d());
    assert!(!out.syntax_error);
}

#[test]
fn the_state_stack_overflows_at_the_grammars_depth() {
    // Torque3D's stack holds 10000 entries; "Parser stack overflow" is reported through the same `yyerror` as a
    // syntax error.
    assert_eq!(MAX_PARSE_DEPTH, 10_000);
    let nested = |n: usize| format!("$x = {}1{};", "(".repeat(n), ")".repeat(n)).into_bytes();
    assert!(!parse(&nested(9994), Some(b"t.cs")).syntax_error);
    let (texts, _) = report(&nested(9995));
    assert_eq!(texts[0], "t.cs Line: 1 - Syntax error.");
}

/// Seconds a call takes.
fn timed<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let start = Instant::now();
    let t = f();
    (t, start.elapsed().as_secs_f64())
}

#[test]
fn the_compilers_own_bounds_give_clean_errors() {
    // MAX_TREE_DEPTH: a left-nested chain of 4092 operators after `$x =` fits, 4093 do not.
    assert_eq!(MAX_TREE_DEPTH, 4096);
    for (start, op) in [
        ("$x = 1", " + 1"),
        ("$x = 1", " @ 1"),
        ("$x = 1", " ? 1 : 1"),
        ("$x = %a", ".f"),
        ("$x = %a", ".f[1]"),
        ("$x = %a", ".f()"),
    ] {
        let chain = |n: usize| format!("{start}{};", op.repeat(n)).into_bytes();
        let (ok, _) = timed(|| compile(&chain(4000), Some(b"t.cs"), Purpose::Run, &t3d()));
        assert!(ok.dso.is_some() && !ok.syntax_error, "{op}: 4000");
        let (deep, secs) = timed(|| compile(&chain(5000), Some(b"t.cs"), Purpose::Run, &t3d()));
        assert!(deep.syntax_error && deep.dso.is_none(), "{op}: 5000");
        let texts = common::texts(&deep.diagnostics);
        assert_eq!(
            texts.last().map(String::as_str),
            Some("an expression or statement nested more than 4096 levels deep"),
            "{op}"
        );
        assert!(secs < 30.0, "{op}: {secs} s");
    }
    let chain = |n: usize| format!("$x = 1{};", " + 1".repeat(n)).into_bytes();
    assert!(!parse(&chain(4092), Some(b"t.cs")).syntax_error);
    assert!(parse(&chain(4093), Some(b"t.cs")).syntax_error);

    // MAX_SOURCE_BYTES: a longer source is refused before it is scanned.
    let mut big = vec![b' '; MAX_SOURCE_BYTES + 1];
    big[0] = b'$';
    let (out, secs) = timed(|| compile(&big, Some(b"big.cs"), Purpose::Run, &t3d()));
    assert!(out.syntax_error && out.dso.is_none());
    assert_eq!(
        common::texts(&out.diagnostics),
        [format!(
            "big.cs: the script is longer than {MAX_SOURCE_BYTES} bytes"
        )]
    );
    assert!(secs < 5.0);
    // ... counted up to its first NUL.
    big[..6].copy_from_slice(b"$x=1;\0");
    assert!(!compile(&big, Some(b"big.cs"), Purpose::Check, &t3d()).syntax_error);
}

#[test]
fn long_tokens_compile() {
    // A token may be any length: long names, strings, runs of white space, comments and numbers.
    for src in [
        format!("$x = {};", "a".repeat(5000)),
        format!("$x = \"{}\";", "s".repeat(5000)),
        format!("{}$x = 1;", " ".repeat(5000)),
        format!("$x = 1; //{}", "c".repeat(5000)),
        format!("$x = {};", "7".repeat(5000)),
    ] {
        let (out, secs) = timed(|| compile(src.as_bytes(), Some(b"t.cs"), Purpose::Run, &t3d()));
        assert!(!out.syntax_error, "{} bytes", src.len());
        assert!(out.diagnostics.is_empty(), "{} bytes", src.len());
        assert!(secs < 5.0);
    }
}

#[test]
fn the_statements_before_a_syntax_error_are_compiled_by_exec_only() {
    // The reductions the default actions allow are made before the error is reported, so `if (1) $x = 1;`
    // is a statement of the file when the `)` after it is rejected (`exec` compiles it).
    for src in [
        &b"$a = 1; if (1) $x = 1; )"[..],
        b"$a = 1; if (1) $x = 1; else $y = 2; )",
        b"$a = 1; function f() {} )",
        b"$a = 1; while (1) {} %x",
    ] {
        let p = parse(src, Some(b"t.cs"));
        assert!(p.syntax_error);
        assert_eq!(p.script.stmts.len(), 2, "{:?}", lossy(src));
        let run = compile(src, Some(b"t.cs"), Purpose::Run, &t3d());
        assert!(run.syntax_error && run.dso.is_some());
        let check = compile(src, Some(b"t.cs"), Purpose::Check, &t3d());
        assert!(check.syntax_error && check.dso.is_none());
    }
    // A statement that the bad token interrupts is not reduced.
    let p = parse(b"$a = 1; $b = 2 )", Some(b"t.cs"));
    assert_eq!(p.script.stmts.len(), 1);
    let p = parse(b"$a = 1; function f() { $b = 2; ", Some(b"t.cs"));
    assert_eq!(p.script.stmts.len(), 1);
}

#[test]
fn max_tokens_ends_the_parse_cleanly() {
    // A parameter list builds the leanest tree per token. `function f(%a, ..., %a) {}` with n parameters
    // is 2n + 5 tokens and the end of the input one more: MAX_TOKENS tokens may be read, not one more.
    assert_eq!(MAX_TOKENS, 1 << 22);
    let list = |params: usize| {
        let mut src = b"function f(".to_vec();
        src.extend(b"%a,".repeat(params - 1));
        src.extend_from_slice(b"%a) {}");
        src
    };
    let fits = (MAX_TOKENS as usize - 6) / 2;
    let (out, secs) = timed(|| compile(&list(fits), Some(b"t.cs"), Purpose::Check, &t3d()));
    assert!(!out.syntax_error, "{:?}", common::texts(&out.diagnostics));
    assert!(secs < 60.0);
    let (out, secs) = timed(|| compile(&list(fits + 1), Some(b"t.cs"), Purpose::Run, &t3d()));
    assert!(out.syntax_error && out.dso.is_none());
    assert_eq!(
        common::texts(&out.diagnostics).last().map(String::as_str),
        Some("the file has more than 4194304 tokens")
    );
    assert!(secs < 60.0);
}
