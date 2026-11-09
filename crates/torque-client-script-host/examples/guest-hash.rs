//! Prints the SHA-256 of a realm guest and the line a release adds to `ACCEPTED_CLIENT_GUESTS`: the justfile's `client-script-guest-hash`.

use torque_client_script_host::{ACCEPTED_CLIENT_GUESTS, guest_hash, guest_hex};

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: guest-hash <torque_lua_client.wasm>");
        std::process::exit(2);
    };
    let wasm = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("{path}: {error}");
            std::process::exit(1);
        }
    };
    let hash = guest_hash(&wasm);
    println!("sha256 {}", guest_hex(&hash));
    println!(
        "listed: {}",
        if ACCEPTED_CLIENT_GUESTS.contains(&hash) {
            "yes"
        } else {
            "no; add this line to ACCEPTED_CLIENT_GUESTS in crates/torque-client-script-host/src/guests.rs:"
        }
    );
    let bytes: Vec<String> = hash.iter().map(|b| format!("0x{b:02x}")).collect();
    println!("    [{}],", bytes.join(", "));
}
