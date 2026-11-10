//! The notice keeps up with the lockfile: every crate Rapier pulls in is either named in this crate's NOTICE (the Apache-2.0
//! ones, whose license asks for it) or listed below with the permissive license it is used under. A dependency update that
//! brings a new crate fails here until it is sorted into one or the other.

use std::collections::{BTreeMap, BTreeSet};

/// The crates of Rapier's tree that are not Apache-2.0 only, with the license this workspace uses them under (each also
/// allowed by `deny.toml`).
const ELSEWHERE: &[(&str, &str)] = &[
    ("allocator-api2", "MIT"),
    ("autocfg", "MIT"),
    ("arrayvec", "MIT"),
    ("bitflags", "MIT"),
    ("bytemuck", "MIT"),
    ("bytemuck_derive", "MIT"),
    ("byteorder", "MIT"),
    ("downcast-rs", "MIT"),
    ("either", "MIT"),
    ("ena", "MIT"),
    ("equivalent", "MIT"),
    ("foldhash", "Zlib"),
    ("glam", "MIT"),
    ("glamx", "MIT"),
    ("hash32", "MIT"),
    ("hashbrown", "MIT"),
    ("heapless", "MIT"),
    ("indexmap", "MIT"),
    ("libm", "MIT"),
    ("log", "MIT"),
    ("matrixmultiply", "MIT"),
    ("num-bigint", "MIT"),
    ("num-complex", "MIT"),
    ("num-derive", "MIT"),
    ("num-integer", "MIT"),
    ("num-rational", "MIT"),
    ("num-traits", "MIT"),
    ("ordered-float", "MIT"),
    ("proc-macro2", "MIT"),
    ("profiling", "MIT"),
    ("profiling-procmacros", "MIT"),
    ("quote", "MIT"),
    ("rawpointer", "MIT"),
    ("robust", "MIT"),
    ("rstar", "MIT"),
    ("safe_arch", "MIT"),
    ("serde", "MIT"),
    ("serde_core", "MIT"),
    ("serde_derive", "MIT"),
    ("slab", "MIT"),
    ("smallvec", "MIT"),
    ("spade", "MIT"),
    ("stable_deref_trait", "MIT"),
    ("static_assertions", "MIT"),
    ("syn", "MIT"),
    ("thiserror", "MIT"),
    ("thiserror-impl", "MIT"),
    ("typenum", "MIT"),
    ("unicode-ident", "MIT AND Unicode-3.0"),
    ("wide", "MIT"),
];

/// The lockfile's packages: each name with the names it depends on (versions dropped: the notice names crates, not versions).
fn lockfile() -> BTreeMap<String, BTreeSet<String>> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock");
    let text = std::fs::read_to_string(path).expect("the workspace's Cargo.lock");
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut name: Option<String> = None;
    let mut in_deps = false;
    for line in text.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            name = None;
            in_deps = false;
        } else if let Some(v) = line.strip_prefix("name = ") {
            let n = v.trim_matches('"').to_string();
            out.entry(n.clone()).or_default();
            name = Some(n);
        } else if line.starts_with("dependencies = [") {
            in_deps = true;
        } else if in_deps && line == "]" {
            in_deps = false;
        } else if in_deps && let Some(n) = &name {
            let dep = line.trim_matches(|c| c == '"' || c == ',');
            let dep = dep.split(' ').next().unwrap_or(dep).to_string();
            out.entry(n.clone()).or_default().insert(dep);
        }
    }
    out
}

#[test]
fn the_notice_names_every_apache_crate_rapier_pulls_in() {
    let lock = lockfile();
    let mut reached = BTreeSet::new();
    let mut todo = vec!["rapier3d".to_string()];
    while let Some(n) = todo.pop() {
        if !reached.insert(n.clone()) {
            continue;
        }
        todo.extend(lock.get(&n).into_iter().flatten().cloned());
    }
    assert!(
        reached.len() > 10,
        "the walk found Rapier's tree: {reached:?}"
    );
    let notice = torque_physics_rapier::NOTICE;
    let named = |n: &str| {
        notice
            .lines()
            .any(|l| l.split_whitespace().next() == Some(n))
    };
    let elsewhere: BTreeSet<&str> = ELSEWHERE.iter().map(|(n, _)| *n).collect();
    let unsorted: Vec<&String> = reached
        .iter()
        .filter(|n| !named(n) && !elsewhere.contains(n.as_str()))
        .collect();
    assert!(
        unsorted.is_empty(),
        "crates in Rapier's tree neither in NOTICE nor in ELSEWHERE: {unsorted:?}"
    );
    for n in ["rapier3d", "parry3d", "nalgebra", "simba"] {
        assert!(named(n), "{n} is in the notice");
        assert!(!elsewhere.contains(n));
    }
    // Every crate the notice names is still in the tree (an entry left behind after an update is stale).
    for line in notice.lines() {
        let Some(first) = line.split_whitespace().next() else {
            continue;
        };
        if line.contains("https://github.com/") {
            assert!(
                reached.contains(first),
                "{first} is named but no longer pulled in"
            );
        }
    }
}

#[test]
fn the_license_text_ships_with_the_notice() {
    let text = torque_physics_rapier::APACHE_LICENSE;
    assert!(text.contains("Apache License"));
    assert!(text.contains("Version 2.0, January 2004"));
    assert!(text.contains("END OF TERMS AND CONDITIONS"));
    // The copy is Rapier's own (nalgebra's file, the same in each of its crates): its appendix names their author.
    let appendix = text
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("Copyright "))
        .expect("the appendix's copyright line");
    assert_eq!(appendix, "Copyright 2020 Sébastien Crozet");
    assert!(torque_physics_rapier::NOTICE.contains("Apache License, Version 2.0"));
    // The workspace's own NOTICE points at them.
    let root = include_str!("../../../NOTICE");
    assert!(root.contains("crates/torque-physics-rapier/NOTICE"));
}
