//! The hostile-mod fuzz: random bytes, mutated valid zips, random file lists and random manifests through the zip reader, the
//! package checks and the default layout's manifest parser. It must not panic, and what it accepts must be within the limits.
//! `TORQUE_FUZZ_MODS` sets the number of cases (default 2,000; 100,000 is the full run, `just fuzz-mods`).
//!
//! Memory is bounded by the limits the cases run under (a zip of at most 64 KiB, at most 1 MiB held for all of a mod, at
//! most 64 KiB for a file): the test checks that every accepted package is inside them and that no read returns more than a
//! file's cap, and the zip reader's own bounds (a directory capped, an entry inflated at most its declared size plus one
//! byte) are what keep the cases that are refused small.

mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};

use common::{Rng, zip_of};
use torque_engine::Engine;
use torque_mods::{
    Candidate, CandidateKind, ModLayout, ModLimits, ModPackage, ModStages, ModTomlLayout, NameInfo,
};

fn count() -> u64 {
    std::env::var("TORQUE_FUZZ_MODS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2_000)
}

fn limits() -> ModLimits {
    ModLimits {
        mods: 8,
        total_bytes: 1 << 20,
        files: 64,
        file_bytes: 64 << 10,
        depth: 6,
        ratio: 100,
        zip_bytes: 64 << 10,
    }
}

fn seeds() -> Vec<Vec<u8>> {
    let toml: &[u8] = b"id = \"x\"\nversion = \"1.0\"\n[scripts]\nserver = \"server.cs\"\n";
    vec![
        zip_of(&[("mod.toml", toml), ("server.cs", b"function f() {}")]),
        zip_of(&[
            ("pack/mod.toml", toml),
            ("pack/art/a.png", &[1u8; 300]),
            ("pack/art/b.png", &[2u8; 300]),
            ("pack/server.cs", b"// x"),
        ]),
        zip_of(&[("mod.toml", toml), ("zeros.bin", &vec![0u8; 20_000])]),
        zip_of(&[
            ("a/", b""),
            ("a/b/", b""),
            ("a/b/c.txt", b"text"),
            ("D.TXT", b"d"),
        ]),
    ]
}

fn alphabet(rng: &mut Rng) -> &'static str {
    const PARTS: [&str; 22] = [
        "a", "A", "b", "..", ".", "con", "nul", "COM1", "x.cs", "x.dso", "mod.toml", "art", "é",
        "\u{0}", "\\", ":", " ", "*", "?", "~", "$", "a.",
    ];
    PARTS[rng.below(PARTS.len())]
}

fn random_name(rng: &mut Rng) -> String {
    let mut name = String::new();
    for _ in 0..1 + rng.below(6) {
        name.push_str(alphabet(rng));
        if rng.below(3) > 0 {
            name.push('/');
        }
    }
    name
}

fn mutate(rng: &mut Rng, seed: &[u8]) -> Vec<u8> {
    let mut bytes = seed.to_vec();
    match rng.below(6) {
        0 => bytes.truncate(rng.below(bytes.len() + 1)),
        1 => bytes.extend((0..rng.below(64)).map(|_| rng.next() as u8)),
        2 => {
            let at = rng.below(bytes.len() + 1);
            bytes.splice(at..at, (0..rng.below(32)).map(|_| rng.next() as u8));
        }
        _ => {
            for _ in 0..1 + rng.below(8) {
                if bytes.is_empty() {
                    break;
                }
                let at = rng.below(bytes.len());
                bytes[at] = rng.next() as u8;
            }
        }
    }
    bytes
}

/// What an accepted package must satisfy.
fn check(package: &ModPackage, limits: &ModLimits) {
    assert!(package.held_bytes() <= limits.total_bytes);
    assert!(package.unpacked_bytes() <= limits.total_bytes);
    assert!(package.len() <= limits.files);
    for name in package.names() {
        assert!(
            name.split('/')
                .all(|part| !part.is_empty() && part != ".." && part != "."),
            "{name}"
        );
        assert!(!name.starts_with('/'), "{name}");
        assert!(name.split('/').count() <= limits.depth, "{name}");
        if let Some(bytes) = package.read(name) {
            assert!(bytes.len() as u64 <= limits.file_bytes);
            assert_eq!(Some(bytes.len() as u64), package.size_of(name));
        }
    }
}

/// Runs one case; true when a package was accepted (so the checks ran on something).
fn one_case(case: u64, seeds: &[Vec<u8>], limits: &ModLimits, engine: &Engine) -> bool {
    let mut accepted = false;
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ case.wrapping_mul(0x2545_f491_4f6c_dd1d));
    for _ in 0..4 {
        rng.next();
    }
    match rng.below(20) {
        0..=5 => {
            // Random bytes, sometimes behind a zip header.
            let mut bytes: Vec<u8> = (0..rng.below(2048)).map(|_| rng.next() as u8).collect();
            if rng.below(2) == 0 {
                bytes.splice(0..0, *b"PK\x03\x04");
            }
            if let Ok(package) = ModPackage::from_zip(bytes, limits) {
                check(&package, limits);
                accepted = true;
            }
        }
        6..=14 => {
            let seed = &seeds[rng.below(seeds.len())];
            if let Ok(package) = ModPackage::from_zip(mutate(&mut rng, seed), limits) {
                check(&package, limits);
                accepted = true;
            }
        }
        15..=17 => {
            let files: Vec<(String, Vec<u8>)> = (0..rng.below(12))
                .map(|_| {
                    let top = if rng.below(10) == 0 { 100_000 } else { 400 };
                    let size = rng.below(top);
                    (random_name(&mut rng), vec![rng.next() as u8; size])
                })
                .collect();
            if let Ok(package) = ModPackage::from_files(files, limits) {
                check(&package, limits);
                accepted = true;
            }
        }
        _ => {
            // A random manifest through the default layout.
            let text: Vec<u8> = if rng.below(2) == 0 {
                (0..rng.below(300)).map(|_| rng.next() as u8).collect()
            } else {
                let mut t = b"id = \"x\"\nversion = \"1\"\n".to_vec();
                t.extend(mutate(&mut rng, b"requires = [\"a >= 1\"]\nload_after = [\"b\"]\n[scripts]\nserver = [\"s.cs\"]\n[extra]\np = 1\n"));
                t
            };
            let files = vec![
                ("mod.toml".to_string(), text),
                ("s.cs".to_string(), b"x".to_vec()),
            ];
            let Ok(package) = ModPackage::from_files(files, limits) else {
                return false;
            };
            accepted = true;
            let name = NameInfo::plain("x");
            let candidate = Candidate {
                raw: "x",
                kind: CandidateKind::Folder,
                name: &name,
                source: "mods/x",
                package: &package,
            };
            if let Ok(Some(info)) =
                ModTomlLayout::default().recognize(&candidate, &ModStages::default())
            {
                let _ = info.check();
            }
            let _ = engine;
        }
    }
    accepted
}

#[test]
fn hostile_mods_never_panic() {
    let limits = limits();
    let seeds = seeds();
    let engine = torque_engine::EngineBuilder::new()
        .build()
        .expect("an empty engine");
    let mut panics = Vec::new();
    let mut accepted = 0u64;
    for case in 0..count() {
        match catch_unwind(AssertUnwindSafe(|| {
            one_case(case, &seeds, &limits, &engine)
        })) {
            Ok(true) => accepted += 1,
            Ok(false) => {}
            Err(_) => {
                panics.push(case);
                if panics.len() >= 5 {
                    break;
                }
            }
        }
    }
    assert!(panics.is_empty(), "panics in cases {panics:?}");
    // The cases reach the checks: a good share of them are accepted.
    assert!(
        accepted * 10 >= count(),
        "only {accepted} of {} cases were accepted",
        count()
    );
    println!("{} cases, {accepted} accepted, 0 panics", count());
}
