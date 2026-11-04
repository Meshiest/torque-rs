//! Random well-formed TorqueScript, for the tests that need many valid programs.

use super::Rng;

/// A few top-level declarations (statements, functions, packages) of random statements and expressions
/// using every construct of the grammar, nested at most `depth` deep.
pub fn random_program(rng: &mut Rng, decls: usize, depth: u32) -> String {
    let mut g = Gen {
        rng,
        out: String::new(),
    };
    for _ in 0..decls {
        g.decl(depth);
        g.out.push('\n');
    }
    g.out
}

struct Gen<'a> {
    rng: &'a mut Rng,
    out: String,
}

const BINARY: &[&str] = &[
    "^", "%", "&", "|", "+", "-", "*", "/", "<", ">", ">=", "<=", "==", "!=", "||", "<<", ">>",
    "&&", "$=", "!$=", "@", "SPC", "TAB", "NL",
];
const ASSIGN: &[&str] = &["+=", "-=", "*=", "/=", "%=", "&=", "^=", "|=", "<<=", ">>="];
const ATOMS: &[&str] = &[
    "1", "0", "42", "2.5", "1e3", "0x1F", "true", "false", "\"s\"", "\"\"", "'tag'", "%a", "$b",
    "%this", "$Pref::x", "foo", "Foo", "break",
];
const NAMES: &[&str] = &["foo", "Bar", "f", "getId", "x", "dataBlock"];
const VARS: &[&str] = &["%a", "$b", "%i", "$Pref::x", "%this"];

impl Gen<'_> {
    fn push(&mut self, s: &str) {
        self.out.push_str(s);
    }

    fn pick(&mut self, items: &[&str]) {
        let s = *self.rng.pick(items);
        self.push(s);
    }

    fn list(&mut self, min: usize, depth: u32) {
        for i in 0..min + self.rng.below(3) {
            if i > 0 {
                self.push(", ");
            }
            self.expr(depth);
        }
    }

    /// An expression that may be followed by `.name` (not one ending in a number).
    fn object(&mut self, depth: u32) {
        match self.rng.below(4) {
            0 => {
                self.push("(");
                self.expr(depth);
                self.push(")");
            }
            1 => self.pick(NAMES),
            _ => self.pick(VARS),
        }
    }

    fn field(&mut self, depth: u32) {
        self.object(depth);
        self.push(".");
        self.pick(NAMES);
        if self.rng.chance(0.3) {
            self.push("[");
            self.list(1, depth);
            self.push("]");
        }
    }

    fn call(&mut self, depth: u32) {
        match self.rng.below(3) {
            0 => {
                self.object(depth);
                self.push(".");
            }
            1 => self.pick(&["Ns::", "Parent::"]),
            _ => {}
        }
        self.pick(NAMES);
        self.push("(");
        self.list(0, depth);
        self.push(")");
    }

    fn expr(&mut self, depth: u32) {
        if depth == 0 || self.rng.chance(0.3) {
            self.pick(ATOMS);
            return;
        }
        let d = depth - 1;
        match self.rng.below(11) {
            0 | 1 => {
                self.expr(d);
                self.push(" ");
                self.pick(BINARY);
                self.push(" ");
                self.expr(d);
            }
            2 => {
                self.pick(&["- ", "!", "~", "* "]);
                self.expr(d);
            }
            3 => {
                self.expr(d);
                self.push(" ? ");
                self.expr(d);
                self.push(" : ");
                self.expr(d);
            }
            4 => {
                self.push("(");
                self.expr(d);
                self.push(")");
            }
            5 => self.call(d),
            6 => self.field(d),
            7 => {
                self.pick(VARS);
                self.push("[");
                self.list(1, d);
                self.push("]");
            }
            8 => self.stmt_expr(d),
            9 => self.object_decl(d),
            _ => self.pick(ATOMS),
        }
    }

    fn assign_tail(&mut self, depth: u32) {
        match self.rng.below(4) {
            0 => {
                self.push(" ");
                self.pick(ASSIGN);
                self.push(" ");
                self.expr(depth);
            }
            1 => self.pick(&["++", "--"]),
            _ => {
                self.push(" = ");
                self.expr(depth);
            }
        }
    }

    fn stmt_expr(&mut self, depth: u32) {
        match self.rng.below(6) {
            0 => {
                self.pick(VARS);
                self.assign_tail(depth);
            }
            1 => {
                self.pick(VARS);
                self.push("[");
                self.list(1, depth);
                self.push("]");
                self.assign_tail(depth);
            }
            2 => {
                self.field(depth);
                if self.rng.chance(0.2) {
                    self.push(" = {");
                    self.list(1, depth);
                    self.push("}");
                } else {
                    self.assign_tail(depth);
                }
            }
            3 => self.object_decl(depth),
            _ => self.call(depth),
        }
    }

    fn object_decl(&mut self, depth: u32) {
        self.push("new ");
        if self.rng.chance(0.2) {
            self.push("(");
            self.expr(depth);
            self.push(")");
        } else {
            self.push("ScriptObject");
        }
        self.push("(");
        if self.rng.chance(0.6) {
            self.expr(depth);
        }
        if self.rng.chance(0.3) {
            self.push(" : Parent");
        }
        if self.rng.chance(0.3) {
            self.push(", ");
            self.list(1, depth);
        }
        self.push(")");
        if self.rng.chance(0.5) {
            self.push(" { ");
            self.slots(depth);
            if depth > 0 {
                for _ in 0..self.rng.below(3) {
                    self.object_decl(depth - 1);
                    self.push("; ");
                }
            }
            self.push("}");
        }
    }

    fn slots(&mut self, depth: u32) {
        for _ in 0..self.rng.below(3) {
            match self.rng.below(3) {
                0 => {
                    self.pick(NAMES);
                    self.push("[");
                    self.list(1, depth);
                    self.push("] = ");
                }
                1 => self.push("datablock = "),
                _ => {
                    self.pick(NAMES);
                    self.push(" = ");
                }
            }
            self.expr(depth);
            self.push("; ");
        }
    }

    fn block(&mut self, depth: u32) {
        if self.rng.chance(0.6) {
            self.push("{ ");
            for _ in 0..self.rng.below(4) {
                self.stmt(depth);
            }
            self.push("} ");
        } else {
            self.stmt(depth);
        }
    }

    fn stmt(&mut self, depth: u32) {
        let d = depth.saturating_sub(1);
        let which = if depth == 0 { 0 } else { self.rng.below(9) };
        match which {
            1 => {
                self.push("if (");
                self.expr(d);
                self.push(") ");
                self.block(d);
                if self.rng.chance(0.5) {
                    self.push("else ");
                    self.block(d);
                }
            }
            2 => {
                self.push("while (");
                self.expr(d);
                self.push(") ");
                self.block(d);
            }
            3 => {
                self.push("for (");
                self.expr(d);
                self.push("; ");
                self.expr(d);
                self.push("; ");
                self.expr(d);
                self.push(") ");
                self.block(d);
            }
            4 => {
                self.pick(&["switch", "switch$"]);
                self.push(" (");
                self.expr(d);
                self.push(") { ");
                for _ in 0..1 + self.rng.below(3) {
                    self.push("case ");
                    self.expr(d);
                    for _ in 0..self.rng.below(3) {
                        self.push(" or ");
                        self.expr(d);
                    }
                    self.push(": ");
                    for _ in 0..self.rng.below(3) {
                        self.stmt(d);
                    }
                }
                if self.rng.chance(0.5) {
                    self.push("default: ");
                    for _ in 0..self.rng.below(3) {
                        self.stmt(d);
                    }
                }
                self.push("} ");
            }
            5 => self.pick(&["return; ", "break; ", "continue; "]),
            6 => {
                self.push("return ");
                self.expr(d);
                self.push("; ");
            }
            7 => {
                self.push("datablock ItemData(");
                self.pick(NAMES);
                if self.rng.chance(0.5) {
                    self.push(" : Base");
                }
                self.push(") { ");
                self.slots(d);
                self.push("x = 1; }; ");
            }
            _ => {
                self.stmt_expr(d);
                self.push("; ");
            }
        }
    }

    fn function(&mut self, depth: u32) {
        self.push("function ");
        if self.rng.chance(0.4) {
            self.push("Ns::");
        }
        self.pick(NAMES);
        self.push("(");
        for i in 0..self.rng.below(4) {
            if i > 0 {
                self.push(", ");
            }
            self.pick(VARS);
        }
        self.push(") { ");
        for _ in 0..self.rng.below(5) {
            self.stmt(depth);
        }
        self.push("} ");
    }

    fn decl(&mut self, depth: u32) {
        match self.rng.below(6) {
            0 => self.function(depth),
            1 => {
                self.push("package P { ");
                for _ in 0..1 + self.rng.below(3) {
                    self.function(depth);
                }
                self.push("}; ");
            }
            _ => self.stmt(depth),
        }
    }
}
