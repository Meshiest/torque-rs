//! The compiler of the classic dialect for the runtime: torque-ts-compiler's classic compiler with a profile.

use torque_ts_compiler::classic::Profile;
use torque_ts_compiler::{Level as CompilerLevel, Purpose};

use crate::host::{Compiled, Compiler};
use crate::vm::{Level, LogLine};

/// Source text to a classic DSO (torque-ts-compiler's [`classic`](torque_ts_compiler::classic) compiler): what
/// `exec` of a source, `compile` and `eval` use with the [`ClassicExecutor`](super::ClassicExecutor). TGE 1.4's
/// profile by default. Compiles on a thread of its own with a large stack, so a compile at the bottom of a deep
/// recursion costs the `Vm`'s thread little.
#[derive(Debug, Clone, Default)]
pub struct ClassicCompiler {
    pub profile: Profile,
}

impl ClassicCompiler {
    pub fn new(profile: Profile) -> ClassicCompiler {
        ClassicCompiler { profile }
    }
}

impl Compiler for ClassicCompiler {
    fn compile(&self, source: &[u8], name: Option<&[u8]>, purpose: Purpose) -> Compiled {
        let output = torque_ts_compiler::classic::compile(source, name, purpose, &self.profile);
        Compiled {
            report: output
                .diagnostics
                .into_iter()
                .map(|d| LogLine {
                    level: match d.level {
                        CompilerLevel::Normal => Level::Normal,
                        CompilerLevel::Warning => Level::Warning,
                        CompilerLevel::Error => Level::Error,
                    },
                    text: d.text,
                })
                .collect(),
            syntax_error: output.syntax_error,
            code: output.dso,
            work: output.work,
        }
    }
}
