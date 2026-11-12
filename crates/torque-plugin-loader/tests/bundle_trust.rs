//! A packaged game's extensions: a library is approved without a question only when its exact bytes have a hash the game
//! compiled into its executable (`shipped`). The bundle's `manifest.toml` lives in the game's folder, so it is only a start-up
//! report of changed or missing files and approves nothing, whatever it lists. The manifest is the one `torque-bundle` writes.

mod common;

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use common::{library_file_name, sample_library, sample_manifest, scratch, user_of};
use torque_bundle::manifest::{Entry, Header, write};
use torque_engine::{Engine, EngineBuilder};
use torque_plugin_loader::{
    ALLOWLIST_FILE, ApprovalRequest, LoadReport, NativeExtensions, NativeExtensionsPlugin, Outcome,
    sha256_hex,
};

const ID: &str = "example.native-sample";

/// A bundle folder: `boot.cs` and `data.bin` under `game/`, the sample's library and manifest under `plugins/`, and a
/// `manifest.toml` listing all of them.
struct Bundle {
    dir: PathBuf,
    library: PathBuf,
    /// The hash of the real library: what a game would have compiled in.
    real_hash: String,
}

impl Bundle {
    fn plugins(&self) -> PathBuf {
        self.dir.join("plugins")
    }

    /// Writes `manifest.toml` to list the files named by `paths` exactly as they are on disk now.
    fn list(&self, paths: &[&str]) {
        let entries: Vec<Entry> = paths
            .iter()
            .map(|p| {
                let bytes = std::fs::read(self.dir.join(p)).expect("a listed file exists");
                Entry::of(p, &bytes)
            })
            .collect();
        std::fs::write(self.dir.join("manifest.toml"), write(&header(), &entries))
            .expect("the manifest is written");
    }
}

fn header() -> Header {
    Header {
        name: "demo".into(),
        version: "0.1.0".into(),
        target: "x86_64-unknown-linux-gnu".into(),
        mode: "compiled".into(),
        features: Vec::new(),
        rustc: "rustc 1.99.0".into(),
    }
}

/// The library's path in a bundle, as the manifest writes it.
fn library_entry() -> String {
    format!("plugins/{}", library_file_name("native_sample"))
}

/// A bundle named `name` whose manifest lists every file in it.
fn bundle(name: &str) -> Bundle {
    let dir = scratch(name);
    std::fs::create_dir_all(dir.join("game")).expect("game folder");
    std::fs::create_dir_all(dir.join("plugins")).expect("plugins folder");
    std::fs::write(dir.join("game/boot.cs"), b"echo(\"boot\");").expect("boot.cs");
    std::fs::write(dir.join("game/data.bin"), [7u8; 5000]).expect("data.bin");
    let entry = library_entry();
    std::fs::copy(sample_library(), dir.join(&entry)).expect("the library is copied");
    std::fs::write(dir.join("plugins/native_sample.toml"), sample_manifest())
        .expect("its manifest");
    let real_hash = sha256_hex(&std::fs::read(dir.join(&entry)).expect("the library reads"));
    let bundle = Bundle {
        library: dir.join(&entry),
        dir,
        real_hash,
    };
    bundle.list(&[
        "game/boot.cs",
        "game/data.bin",
        "plugins/native_sample.toml",
        &entry,
    ]);
    bundle
}

/// An engine over the bundle's `plugins/` that has `shipped` hashes compiled in and asks a callback `answer` about anything
/// else, counting the questions in `asked`.
fn engine(bundle: &Bundle, asked: &Rc<Cell<u32>>, answer: bool, shipped: &[&str]) -> Engine {
    let asked = Rc::clone(asked);
    let plugin = NativeExtensionsPlugin::new(bundle.plugins())
        .user_folder(user_of(&bundle.dir))
        .bundle(&bundle.dir)
        .shipped(shipped)
        .approver(move |_: &ApprovalRequest| {
            asked.set(asked.get() + 1);
            answer
        });
    EngineBuilder::new()
        .add_plugins(NativeExtensions(plugin))
        .enable_id("torque.native-extensions")
        .build()
        .expect("the engine builds")
}

fn report(engine: &Engine) -> LoadReport {
    engine
        .resource::<LoadReport>()
        .expect("the loader ran")
        .clone()
}

fn outcome_of(engine: &Engine, id: &str) -> Outcome {
    report(engine)
        .get(id)
        .expect("the extension has an entry")
        .outcome
        .clone()
}

fn bundle_warnings(engine: &Engine) -> Vec<String> {
    engine
        .warnings()
        .iter()
        .map(ToString::to_string)
        .filter(|w| w.contains("bundle:"))
        .collect()
}

/// Another library beside the real one, with its own extension manifest, listed in the manifest too.
fn add_evil(bundle: &Bundle) {
    let file = library_file_name("evil");
    std::fs::write(
        bundle.plugins().join(&file),
        b"not a library, but it is not shipped",
    )
    .expect("evil library");
    std::fs::write(
        bundle.plugins().join("evil.toml"),
        "id = \"someone.evil\"\nversion = \"1.0.0\"\nlibrary = \"evil\"\n",
    )
    .expect("evil manifest");
    bundle.list(&[
        "game/boot.cs",
        "game/data.bin",
        "plugins/native_sample.toml",
        &library_entry(),
        &format!("plugins/{file}"),
        "plugins/evil.toml",
    ]);
}

#[test]
fn an_untouched_bundle_reports_nothing_and_agrees_with_the_bundlers_own_check() {
    let b = bundle("trust-clean");
    let engine = engine(&b, &Rc::new(Cell::new(0)), false, &[b.real_hash.as_str()]);
    assert!(report(&engine).bundle_problems.is_empty());
    assert!(bundle_warnings(&engine).is_empty());
    assert!(
        torque_bundle::manifest::verify(&b.dir)
            .expect("the bundler's check runs")
            .is_empty()
    );
}

#[test]
fn a_changed_or_missing_bundled_file_is_reported_at_start_by_name() {
    let b = bundle("trust-changed");
    std::fs::write(b.dir.join("game/boot.cs"), b"echo(\"changed\");").expect("boot.cs");
    // The same length: only the hash tells.
    let mut data = std::fs::read(b.dir.join("game/data.bin")).expect("data.bin");
    data[10] ^= 1;
    std::fs::write(b.dir.join("game/data.bin"), data).expect("data.bin");
    std::fs::remove_file(b.dir.join("plugins/native_sample.toml")).expect("its manifest");
    let engine = engine(&b, &Rc::new(Cell::new(0)), false, &[b.real_hash.as_str()]);
    let problems = &report(&engine).bundle_problems;
    assert_eq!(problems.len(), 3, "{problems:?}");
    for (file, what) in [
        ("game/boot.cs", ": changed"),
        ("game/data.bin", ": changed"),
        ("plugins/native_sample.toml", ": missing"),
    ] {
        assert!(
            problems
                .iter()
                .any(|p| p.starts_with(&format!("{file}{what}"))),
            "{problems:?}"
        );
    }
    let warned = bundle_warnings(&engine);
    assert_eq!(warned.len(), 3, "{warned:?}");
    assert!(warned.iter().all(|w| w.starts_with("bundle: ")));
    // The bundler's own check names the same files.
    let theirs = torque_bundle::manifest::verify(&b.dir).expect("the bundler's check runs");
    for file in [
        "game/boot.cs",
        "game/data.bin",
        "plugins/native_sample.toml",
    ] {
        assert!(theirs.iter().any(|p| p.starts_with(file)), "{theirs:?}");
    }
}

#[test]
fn a_file_the_manifest_does_not_list_is_not_a_change() {
    let b = bundle("trust-extra-file");
    std::fs::create_dir_all(b.dir.join("game/mods")).expect("mods folder");
    std::fs::write(b.dir.join("game/mods/mine.txt"), b"a player's own").expect("a mod");
    let engine = engine(&b, &Rc::new(Cell::new(0)), false, &[b.real_hash.as_str()]);
    assert!(report(&engine).bundle_problems.is_empty());
}

#[test]
fn the_shipped_library_loads_without_asking() {
    let b = bundle("trust-shipped");
    let asked = Rc::new(Cell::new(0));
    let engine = engine(&b, &asked, false, &[b.real_hash.as_str()]);
    assert_eq!(outcome_of(&engine, ID), Outcome::Loaded);
    assert!(engine.is_plugin_added_id(ID));
    assert_eq!(asked.get(), 0, "nobody was asked");
    assert!(
        !user_of(&b.dir).join(ALLOWLIST_FILE).exists(),
        "a shipped hash is not copied into the player's approvals"
    );
}

#[test]
fn a_shipped_library_whose_bytes_changed_goes_to_the_ask_path() {
    let b = bundle("trust-changed-library");
    let mut bytes = std::fs::read(&b.library).expect("the library reads");
    bytes.extend_from_slice(b"one more byte");
    std::fs::write(&b.library, bytes).expect("the library is changed");
    let asked = Rc::new(Cell::new(0));
    let refused = engine(&b, &asked, false, &[b.real_hash.as_str()]);
    assert_eq!(asked.get(), 1);
    assert_eq!(outcome_of(&refused, ID), Outcome::NotApproved);
    let problems = &report(&refused).bundle_problems;
    assert!(
        problems
            .iter()
            .any(|p| p.starts_with("plugins/") && p.contains(": changed")),
        "{problems:?}"
    );
    // The player can still approve it, as any library.
    let approved = engine(&b, &asked, true, &[b.real_hash.as_str()]);
    assert_eq!(asked.get(), 2);
    assert_eq!(outcome_of(&approved, ID), Outcome::Loaded);
}

#[test]
fn a_library_swapped_in_with_a_manifest_rewritten_to_match_still_asks() {
    let b = bundle("trust-swapped");
    let mut evil = std::fs::read(&b.library).expect("the library reads");
    evil.extend_from_slice(b"an attacker's bytes");
    std::fs::write(&b.library, &evil).expect("the library is swapped");
    // The attacker also rewrites the manifest to list the swapped file's correct hash: the folder is self-consistent.
    b.list(&[
        "game/boot.cs",
        "game/data.bin",
        "plugins/native_sample.toml",
        &library_entry(),
    ]);
    let asked = Rc::new(Cell::new(0));
    let engine = engine(&b, &asked, false, &[b.real_hash.as_str()]);
    assert_eq!(asked.get(), 1, "the swapped library is asked about");
    assert_eq!(outcome_of(&engine, ID), Outcome::NotApproved);
    assert!(!engine.is_plugin_added_id(ID));
    // The report is advisory: it cannot see this change, and nothing relies on it.
    assert!(report(&engine).bundle_problems.is_empty());
}

#[test]
fn an_added_library_with_a_manifest_entry_still_asks() {
    let b = bundle("trust-added");
    add_evil(&b);
    let asked = Rc::new(Cell::new(0));
    let engine = engine(&b, &asked, false, &[b.real_hash.as_str()]);
    assert_eq!(asked.get(), 1, "only the added library is asked about");
    assert_eq!(outcome_of(&engine, "someone.evil"), Outcome::NotApproved);
    assert_eq!(outcome_of(&engine, ID), Outcome::Loaded);
    assert!(report(&engine).bundle_problems.is_empty());
}

#[test]
fn a_manifest_alone_approves_nothing() {
    let b = bundle("trust-manifest-alone");
    let asked = Rc::new(Cell::new(0));
    // The manifest lists the real library with its right hash, but the game compiled in no hash.
    let engine = engine(&b, &asked, false, &[]);
    assert_eq!(asked.get(), 1);
    assert_eq!(outcome_of(&engine, ID), Outcome::NotApproved);
    // A malformed hash in the compiled-in list is ignored, not read as a wildcard.
    let engine = self::engine(&b, &asked, false, &["", "*", &b.real_hash[..63]]);
    assert_eq!(asked.get(), 2);
    assert_eq!(outcome_of(&engine, ID), Outcome::NotApproved);
}

#[test]
fn a_manifest_entry_that_escapes_the_folder_is_refused_and_reported() {
    let b = bundle("trust-traversal");
    let file = library_file_name("native_sample");
    let mut text =
        std::fs::read_to_string(b.dir.join("manifest.toml")).expect("the manifest reads");
    for path in [
        format!("../plugins/{file}"),
        format!("plugins/../plugins/{file}"),
        format!("/plugins/{file}"),
        format!("C:/plugins/{file}"),
        format!("plugins\\{file}"),
    ] {
        text.push_str(&format!(
            "\n[[file]]\npath = {path:?}\nsize = 1\nsha256 = \"{}\"\n",
            b.real_hash
        ));
    }
    std::fs::write(b.dir.join("manifest.toml"), text).expect("the manifest is written");
    let asked = Rc::new(Cell::new(0));
    let engine = engine(&b, &asked, false, &[b.real_hash.as_str()]);
    let problems = &report(&engine).bundle_problems;
    assert_eq!(
        problems
            .iter()
            .filter(|p| p.starts_with("manifest entry refused"))
            .count(),
        5,
        "{problems:?}"
    );
    assert_eq!(
        problems.len(),
        5,
        "no listed file outside the folder was read: {problems:?}"
    );
    assert_eq!(asked.get(), 0);
    // With nothing compiled in, the entries approve nothing.
    let engine = self::engine(&b, &asked, false, &[]);
    assert_eq!(asked.get(), 1);
    assert_eq!(outcome_of(&engine, ID), Outcome::NotApproved);
}

#[test]
fn a_bundle_without_a_usable_manifest_reports_it_and_the_shipped_library_still_loads() {
    let b = bundle("trust-no-manifest");
    std::fs::remove_file(b.dir.join("manifest.toml")).expect("the manifest is removed");
    let asked = Rc::new(Cell::new(0));
    let engine = engine(&b, &asked, false, &[b.real_hash.as_str()]);
    assert_eq!(asked.get(), 0);
    assert_eq!(outcome_of(&engine, ID), Outcome::Loaded);
    assert_eq!(bundle_warnings(&engine).len(), 1);
    assert_eq!(report(&engine).bundle_problems.len(), 1);
    // A manifest of another format is no better.
    std::fs::write(b.dir.join("manifest.toml"), "format = 2\n").expect("the manifest is written");
    let engine = self::engine(&b, &asked, false, &[b.real_hash.as_str()]);
    assert_eq!(report(&engine).bundle_problems.len(), 1);
    // And no hash compiled in means a question, with or without a manifest.
    let engine = self::engine(&b, &asked, false, &[]);
    assert_eq!(asked.get(), 1);
    assert_eq!(outcome_of(&engine, ID), Outcome::NotApproved);
}

#[cfg(unix)]
#[test]
fn a_listed_file_replaced_by_a_link_is_a_change_and_is_never_read_through() {
    let b = bundle("trust-link");
    let elsewhere = scratch("trust-link-target");
    let boot = std::fs::read(b.dir.join("game/boot.cs")).expect("boot.cs");
    std::fs::write(elsewhere.join("boot.cs"), boot).expect("the target");
    std::fs::remove_file(b.dir.join("game/boot.cs")).expect("boot.cs removed");
    std::os::unix::fs::symlink(elsewhere.join("boot.cs"), b.dir.join("game/boot.cs"))
        .expect("a link");
    // The file itself is a link.
    let engine = engine(&b, &Rc::new(Cell::new(0)), false, &[b.real_hash.as_str()]);
    let problems = &report(&engine).bundle_problems;
    assert!(
        problems
            .iter()
            .any(|p| p.starts_with("game/boot.cs: is a link")),
        "{problems:?}"
    );
    // A folder on the way is a link.
    std::fs::remove_dir_all(b.dir.join("game")).expect("game removed");
    std::fs::create_dir_all(elsewhere.join("game")).expect("the target folder");
    std::fs::write(elsewhere.join("game/data.bin"), [7u8; 5000]).expect("data.bin");
    std::fs::write(elsewhere.join("game/boot.cs"), b"echo(\"boot\");").expect("boot.cs");
    std::os::unix::fs::symlink(elsewhere.join("game"), b.dir.join("game")).expect("a link");
    let engine = self::engine(&b, &Rc::new(Cell::new(0)), false, &[b.real_hash.as_str()]);
    let problems = &report(&engine).bundle_problems;
    assert_eq!(
        problems
            .iter()
            .filter(|p| p.contains(": is a link"))
            .count(),
        2,
        "{problems:?}"
    );
}
