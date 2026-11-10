//! Prints the SHA-256 of a realm guest and the line a release adds to `ACCEPTED_CLIENT_GUESTS`: the justfile's `client-script-guest-hash`.
//! With `--check` it exits 1 when the guest is not listed (`client-script-check`).

use torque_client_script_host::{ACCEPTED_CLIENT_GUESTS, guest_hash, guest_hex};

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let check = args.first().is_some_and(|a| a == "--check");
    if check {
        args.remove(0);
    }
    let Some(path) = args.first() else {
        eprintln!("usage: guest-hash [--check] <torque_lua_client.wasm>");
        std::process::exit(2);
    };
    let wasm = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("{path}: {error}");
            std::process::exit(1);
        }
    };
    let hash = guest_hash(&wasm);
    let listed = ACCEPTED_CLIENT_GUESTS.contains(&hash);
    println!("sha256 {}", guest_hex(&hash));
    println!(
        "listed: {}",
        if listed {
            "yes"
        } else {
            "no; add this line to ACCEPTED_CLIENT_GUESTS in crates/torque-client-script-host/src/guests.rs:"
        }
    );
    let bytes: Vec<String> = hash.iter().map(|b| format!("0x{b:02x}")).collect();
    println!("    [{}],", bytes.join(", "));
    if check && !listed {
        std::process::exit(1);
    }
}
