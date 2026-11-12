//! Which realm guests this build runs: the SHA-256 of each accepted `torque_lua_client.wasm`. The guest is the one piece of the
//! realm that is not Rust built with the engine (Luau's C++, compiled with the WASI SDK), so the engine runs only bytes whose
//! hash a release lists here. A missing or different file makes server scripts unavailable.

use std::fmt;

use sha2::{Digest, Sha256};

/// The hash of a guest.
pub type GuestHash = [u8; 32];

/// The guests this build accepts. A release lists the hash of the guest `just lua-client-web` built for it (`just
/// client-script-guest-hash` prints the line to add); a build with the list empty runs none, so server scripts are
/// unavailable in it. Adding a hash is a change a reviewer sees, and the guest it names is the code that runs the server's
/// Lua. The book's "Server scripts" chapter names the toolchain that reproduces each hash.
pub const ACCEPTED_CLIENT_GUESTS: &[GuestHash] = &[
    // WASI SDK 34.0, Rust 1.99.1, Windows x86_64.
    [
        0xa9, 0x70, 0xd9, 0xc2, 0x63, 0x0b, 0x05, 0x71, 0x76, 0x49, 0xc5, 0x17, 0x47, 0x04, 0xcf,
        0xbc, 0xc9, 0x06, 0x84, 0x42, 0x45, 0xd1, 0x66, 0xb0, 0xe5, 0x0d, 0xbc, 0x21, 0x9f, 0x7b,
        0x70, 0x32,
    ],
];

/// A guest that is not on the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuestRejected {
    /// The hash of the bytes offered, as 64 lower-case hexadecimal digits.
    pub hash: String,
}

impl fmt::Display for GuestRejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the realm guest {} is not one this build accepts; server scripts are unavailable",
            self.hash
        )
    }
}

impl std::error::Error for GuestRejected {}

/// The SHA-256 of `wasm`.
pub fn guest_hash(wasm: &[u8]) -> GuestHash {
    Sha256::digest(wasm).into()
}

/// A hash as 64 lower-case hexadecimal digits.
pub fn guest_hex(hash: &GuestHash) -> String {
    use fmt::Write;
    hash.iter().fold(String::with_capacity(64), |mut text, b| {
        let _ = write!(text, "{b:02x}");
        text
    })
}

/// The accepted hashes as hexadecimal text, for a page that checks its own copy of the guest.
pub fn accepted_hex() -> Vec<String> {
    ACCEPTED_CLIENT_GUESTS.iter().map(guest_hex).collect()
}

/// Checks `wasm` against `list`.
///
/// # Errors
///
/// [`GuestRejected`] when its hash is not in the list.
pub fn check_against(list: &[GuestHash], wasm: &[u8]) -> Result<(), GuestRejected> {
    let hash = guest_hash(wasm);
    if list.contains(&hash) {
        Ok(())
    } else {
        Err(GuestRejected {
            hash: guest_hex(&hash),
        })
    }
}

/// Checks `wasm` against [`ACCEPTED_CLIENT_GUESTS`].
///
/// # Errors
///
/// [`GuestRejected`] when its hash is not listed.
pub fn check_guest(wasm: &[u8]) -> Result<(), GuestRejected> {
    check_against(ACCEPTED_CLIENT_GUESTS, wasm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_guest_runs_only_if_its_hash_is_listed() {
        let guest = b"\0asm\x01\0\0\0 a guest";
        let list = [guest_hash(guest), [7; 32]];
        assert!(check_against(&list, guest).is_ok());
        let mut changed = guest.to_vec();
        changed[10] ^= 1;
        let err = check_against(&list, &changed).expect_err("one byte differs");
        assert_eq!(err.hash, guest_hex(&guest_hash(&changed)));
        assert!(
            check_against(&[], guest).is_err(),
            "an empty list accepts nothing"
        );
        assert!(check_against(&list, b"").is_err());
    }

    #[test]
    fn the_hash_is_sha256_in_lower_case_hex() {
        assert_eq!(
            guest_hex(&guest_hash(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn this_build_lists_no_guest_it_cannot_name() {
        // Every listed hash is a whole 32-byte digest, and none repeats.
        for (n, hash) in ACCEPTED_CLIENT_GUESTS.iter().enumerate() {
            assert_eq!(hash.len(), 32);
            assert!(!ACCEPTED_CLIENT_GUESTS[..n].contains(hash));
        }
        assert_eq!(accepted_hex().len(), ACCEPTED_CLIENT_GUESTS.len());
    }
}
