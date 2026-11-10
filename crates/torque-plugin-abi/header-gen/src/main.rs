//! Writes the C header of the native extension ABI (`include/torque_plugin.h`) from `src/lib.rs`, or with `--check` fails
//! when the header in the repository is not the one the ABI makes.

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let check = std::env::args().skip(1).any(|a| a == "--check");
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let crate_dir = here.parent().expect("the ABI crate's folder");
    let header = crate_dir.join("include").join("torque_plugin.h");
    let config = match cbindgen::Config::from_file(here.join("cbindgen.toml")) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("cbindgen.toml: {e}");
            return ExitCode::FAILURE;
        }
    };
    let bindings = match cbindgen::Builder::new()
        .with_config(config)
        .with_src(crate_dir.join("src").join("lib.rs"))
        .generate()
    {
        Ok(bindings) => bindings,
        Err(e) => {
            eprintln!("the header could not be made: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut made = Vec::new();
    bindings.write(&mut made);
    // One form of line ending whatever the checkout's.
    let made = String::from_utf8(made)
        .expect("cbindgen writes UTF-8")
        .replace("\r\n", "\n");
    if check {
        let kept = std::fs::read_to_string(&header)
            .unwrap_or_default()
            .replace("\r\n", "\n");
        if kept != made {
            eprintln!(
                "{} is not what the ABI makes: run `just c-header` and commit the result",
                header.display()
            );
            return ExitCode::FAILURE;
        }
        println!("{} is up to date", header.display());
    } else {
        if let Err(e) = std::fs::write(&header, made) {
            eprintln!("{}: {e}", header.display());
            return ExitCode::FAILURE;
        }
        println!("wrote {}", header.display());
    }
    ExitCode::SUCCESS
}
