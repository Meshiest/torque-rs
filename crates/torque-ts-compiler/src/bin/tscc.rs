//! `tscc`: compiles a TorqueScript file to a DSO.
//!
//! `tscc <file.cs> <out.dso>` writes the DSO; `tscc --text <file.cs>` prints its text form
//! (`torque_ts_bytecode::dsotext`). `--name <name>` sets the file name the messages use (default: the path).
//! Messages go to the standard error stream; the exit code is 1 after a syntax error. The dialect is Torque3D's.

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let text = args.first().is_some_and(|a| a == "--text");
    if text {
        args.remove(0);
    }
    let mut name: Option<String> = None;
    if args.first().is_some_and(|a| a == "--name") && args.len() >= 2 {
        name = Some(args.remove(1));
        args.remove(0);
    }
    let Some(input) = args.first() else {
        eprintln!("usage: tscc [--text] [--name <name>] <file.cs> [<out.dso>]");
        return ExitCode::from(2);
    };
    let source = match std::fs::read(input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{input}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let name = name.unwrap_or_else(|| input.clone());
    let dialect = torque_ts_bytecode::Dialect::torque3d();
    let out = torque_ts_compiler::compile(
        &source,
        Some(name.as_bytes()),
        torque_ts_compiler::Purpose::Run,
        &dialect,
    );
    for d in &out.diagnostics {
        eprintln!("{:?}: {}", d.level, String::from_utf8_lossy(&d.text));
    }
    let Some(dso) = out.dso else {
        eprintln!("{input}: no code");
        return ExitCode::FAILURE;
    };
    if text {
        print!("{}", torque_ts_bytecode::dsotext::render(&dso, &dialect));
    } else if let Some(path) = args.get(1)
        && let Err(e) = std::fs::write(path, dso.to_bytes(&dialect))
    {
        eprintln!("{path}: {e}");
        return ExitCode::FAILURE;
    }
    if out.syntax_error {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
