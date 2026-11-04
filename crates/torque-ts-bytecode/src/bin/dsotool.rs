use std::process::ExitCode;

use torque_ts_bytecode::decode::{MAX_PROBLEMS, decode_dso, render, verify};
use torque_ts_bytecode::opcodes::Op;
use torque_ts_bytecode::{Dialect, DsoFile};

fn usage() -> ExitCode {
    eprintln!("usage: dsotool <info|decode|text> <file.dso>");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let (Some(command), Some(path)) = (args.get(1), args.get(2)) else {
        return usage();
    };
    if command != "info" && command != "decode" && command != "text" {
        return usage();
    }

    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    // The built-in dialect: Torque3D's.
    let dialect = Dialect::torque3d();
    let dso = match DsoFile::parse(&bytes, &dialect) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    if command == "text" {
        let text = torque_ts_bytecode::dsotext::render(&dso, &dialect);
        // The text form must read back to the same bytes.
        match torque_ts_bytecode::dsotext::parse(&text) {
            Ok(back) if back.to_bytes(&dialect) == bytes => {
                print!("{text}");
                return ExitCode::SUCCESS;
            }
            Ok(_) => eprintln!("{path}: the text form does not read back to the same bytes"),
            Err(e) => eprintln!("{path}: the text form does not read back: {e}"),
        }
        return ExitCode::FAILURE;
    }
    let instructions = match decode_dso(&dso, &dialect) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    if command == "decode" {
        print!("{}", render(&dso, &instructions));
        return ExitCode::SUCCESS;
    }

    let problems = verify(&dso, &instructions);
    let functions = instructions.iter().filter(|i| i.op == Op::FuncDecl).count();
    println!("file: {path}");
    println!("version: {}", dso.version);
    println!(
        "global strings: {} bytes, {} floats",
        dso.global_strings.len(),
        dso.global_floats.len()
    );
    println!(
        "function strings: {} bytes, {} floats",
        dso.function_strings.len(),
        dso.function_floats.len()
    );
    println!("code words: {}", dso.code.len());
    println!("local variable entries: {}", dso.local_variables.len());
    println!("line break pairs: {}", dso.line_breaks.len() / 2);
    println!("identifier entries: {}", dso.identifiers.len());
    println!("instructions: {}", instructions.len());
    println!("functions: {functions}");
    // `verify` stops collecting at MAX_PROBLEMS, so a full list means "at least that many".
    if problems.len() >= MAX_PROBLEMS {
        println!("consistency problems: {}+", problems.len());
    } else {
        println!("consistency problems: {}", problems.len());
    }
    for p in &problems {
        println!("  {p}");
    }
    if problems.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
