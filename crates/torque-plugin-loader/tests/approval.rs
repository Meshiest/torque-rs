//! Hash approval: a library nobody approved is not loaded (none of its code runs), an approved one loads and is recorded,
//! a recorded one loads without a question, and a changed file is asked about again, naming the hash approved before.
//! Every question goes to a callback: nothing waits on a terminal or opens a window. The plugin is off until game.toml
//! or the flag turns it on.

use crate::common;

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use common::{library_file_name, sample_folder, sample_library, sample_manifest, scratch, user_of};
use torque_app_server::config::{EngineConfig, GameConfig};
use torque_engine::{Engine, EngineBuilder};
use torque_plugin_loader::{
    ALLOWLIST_FILE, Allowlist, ApprovalRequest, LoadReport, NativeExtensions,
    NativeExtensionsPlugin, Outcome, enable_by_flag,
};

const ID: &str = "example.native-sample";

/// An engine loading `folder` with `plugin`'s approver, turned on.
fn engine(plugin: NativeExtensionsPlugin) -> Engine {
    EngineBuilder::new()
        .add_plugins(NativeExtensions(plugin))
        .enable_id("torque.native-extensions")
        .build()
        .unwrap()
}

/// An allowlist in `folder` asking a callback that records each question and answers `answer`.
fn asking(folder: &Path, asked: &Rc<RefCell<Vec<ApprovalRequest>>>, answer: bool) -> Allowlist {
    let asked = Rc::clone(asked);
    Allowlist::new(user_of(folder).join(ALLOWLIST_FILE)).asking(move |request: &ApprovalRequest| {
        asked.borrow_mut().push(request.clone());
        answer
    })
}

fn outcome(engine: &Engine) -> Outcome {
    engine
        .resource::<LoadReport>()
        .unwrap()
        .get(ID)
        .unwrap()
        .outcome
        .clone()
}

#[test]
fn unapproved_is_not_loaded_approved_loads_and_a_changed_file_asks_again() {
    let a = sample_folder("approval-a", &[]);
    let asked = Rc::new(RefCell::new(Vec::new()));

    // Not approved: not loaded, and nothing is recorded.
    let denied = engine(common::plugin(&a).approver(asking(&a, &asked, false)));
    assert_eq!(outcome(&denied), Outcome::NotApproved);
    assert!(!denied.is_plugin_added_id(ID));
    assert!(
        denied
            .warnings()
            .iter()
            .any(|w| w.to_string().contains("not approved"))
    );
    assert_eq!(asked.borrow().len(), 1);
    let first = asked.borrow()[0].clone();
    assert_eq!((first.id.as_str(), first.version.as_str()), (ID, "0.1.0"));
    assert_eq!(first.sha256.len(), 64);
    assert_eq!(first.approved_before, None);
    assert!(first.question().contains(&first.sha256));
    assert!(!user_of(&a).join(ALLOWLIST_FILE).exists());

    // Approved: loaded, and recorded.
    let approved = engine(common::plugin(&a).approver(asking(&a, &asked, true)));
    assert_eq!(outcome(&approved), Outcome::Loaded);
    assert!(approved.is_plugin_added_id(ID));
    assert_eq!(asked.borrow().len(), 2);
    let list = std::fs::read_to_string(user_of(&a).join(ALLOWLIST_FILE)).unwrap();
    assert!(
        list.contains(&format!("{} {ID} 0.1.0", first.sha256)),
        "{list}"
    );

    // Recorded: loads with nobody to ask.
    let again = engine(common::plugin(&a).deny_unknown());
    assert_eq!(outcome(&again), Outcome::Loaded);

    // The same id, another file: asked again, with the hash approved before; refused, it is not loaded.
    let b = sample_folder("approval-b", b"one more byte");
    std::fs::create_dir_all(user_of(&b)).unwrap();
    std::fs::copy(
        user_of(&a).join(ALLOWLIST_FILE),
        user_of(&b).join(ALLOWLIST_FILE),
    )
    .unwrap();
    let changed = engine(common::plugin(&b).approver(asking(&b, &asked, false)));
    assert_eq!(outcome(&changed), Outcome::NotApproved);
    assert_eq!(asked.borrow().len(), 3);
    let third = asked.borrow()[2].clone();
    assert_ne!(third.sha256, first.sha256);
    assert_eq!(
        third.approved_before.as_deref(),
        Some(first.sha256.as_str())
    );
    assert!(third.question().contains("changed"));
    // With nobody to ask, the changed file is not loaded either.
    let unasked = engine(common::plugin(&b).deny_unknown());
    assert_eq!(outcome(&unasked), Outcome::NotApproved);
}

#[test]
fn off_unless_game_toml_or_the_flag_turns_it_on() {
    let folder = sample_folder("approval-off", &[]);
    let yes = |_: &ApprovalRequest| true;

    let off = EngineBuilder::new()
        .add_plugins(NativeExtensions(common::plugin(&folder).approver(yes)))
        .build()
        .unwrap();
    assert!(
        off.resource::<LoadReport>().is_none(),
        "the plugin did not run"
    );
    assert!(!off.is_plugin_added_id(ID));

    let config = GameConfig::parse("[plugins]\nenable = [\"torque.native-extensions\"]\n").unwrap();
    let by_config = EngineBuilder::new()
        .add_plugins(NativeExtensions(common::plugin(&folder).approver(yes)))
        .config(&config)
        .build()
        .unwrap();
    assert!(by_config.is_plugin_added_id(ID));

    let mut builder = EngineBuilder::new();
    builder.add_plugins(NativeExtensions(common::plugin(&folder).approver(yes)));
    assert!(!enable_by_flag(&mut builder, ["game", "--windowed"]));
    assert!(enable_by_flag(
        &mut builder,
        ["game", "--native-extensions"]
    ));
    assert!(builder.build().unwrap().is_plugin_added_id(ID));
}

#[test]
fn a_manifest_that_does_not_match_the_library_is_refused() {
    let folder = sample_folder("approval-mismatch", &[]);
    std::fs::write(
        folder.join("native_sample.toml"),
        "id = \"example.other\"\nversion = \"0.1.0\"\nlibrary = \"native_sample\"\n",
    )
    .unwrap();
    let engine = engine(common::plugin(&folder).approver(|_: &ApprovalRequest| true));
    let report = engine.resource::<LoadReport>().unwrap();
    let Outcome::Refused(reason) = &report.get("example.other").unwrap().outcome else {
        panic!("refused");
    };
    assert!(
        reason.contains("reports example.native-sample 0.1.0"),
        "{reason}"
    );
    assert!(!engine.is_plugin_added_id("example.other"));
    assert!(!engine.is_plugin_added_id(ID));
}

#[test]
fn a_file_that_is_not_an_extension_is_refused_before_its_code_runs() {
    let folder = scratch("approval-not-an-extension");
    std::fs::write(folder.join(library_file_name("broken")), b"not a library").unwrap();
    std::fs::write(
        folder.join("broken.toml"),
        "id = \"test.broken\"\nversion = \"1\"\n",
    )
    .unwrap();
    let engine = engine(common::plugin(&folder).approver(|_: &ApprovalRequest| true));
    let report = engine.resource::<LoadReport>().unwrap();
    assert!(matches!(
        report.get("test.broken").unwrap().outcome,
        Outcome::Refused(_)
    ));
}

#[test]
fn an_allowlist_in_the_plugins_folder_is_ignored() {
    let folder = sample_folder("approval-shipped-list", &[]);
    let bytes = std::fs::read(sample_library()).unwrap();
    let sha256 = torque_plugin_loader::sha256_hex(&bytes);
    // A download that ships its own approval of itself.
    std::fs::write(
        folder.join(ALLOWLIST_FILE),
        format!(
            "{sha256} {ID} 0.1.0
"
        ),
    )
    .unwrap();
    let engine = engine(common::plugin(&folder).deny_unknown());
    assert_eq!(outcome(&engine), Outcome::NotApproved);
    assert!(!engine.is_plugin_added_id(ID));
    assert!(
        engine
            .warnings()
            .iter()
            .any(|w| w.to_string().contains("is ignored")),
        "{:?}",
        engine.warnings()
    );
}

#[test]
fn a_user_folder_inside_the_plugins_folder_loads_nothing() {
    let folder = sample_folder("approval-user-inside", &[]);
    let engine = engine(
        NativeExtensionsPlugin::new(&folder)
            .user_folder(folder.join("approvals"))
            .approver(|_: &ApprovalRequest| true),
    );
    assert!(!engine.is_plugin_added_id(ID));
    assert!(
        engine
            .warnings()
            .iter()
            .any(|w| w.to_string().contains("inside the plugins folder")),
        "{:?}",
        engine.warnings()
    );
}

#[test]
fn a_file_swapped_after_it_was_read_is_never_the_one_loaded() {
    let folder = sample_folder("approval-swap", &[]);
    let original = folder.join(library_file_name("native_sample"));
    let cache = user_of(&folder).join("cache");
    // A copy left from an earlier run of another file: cleaned away.
    std::fs::create_dir_all(&cache).unwrap();
    let stale = cache.join(format!(
        "{}.{}",
        "0".repeat(64),
        library_file_name("x").rsplit('.').next().unwrap()
    ));
    std::fs::write(&stale, b"old").unwrap();
    let approved = Rc::new(RefCell::new(String::new()));
    let seen = Rc::clone(&approved);
    let swap = original.clone();
    // The file is replaced while the player is asked: the loader must load what it hashed, not what is there now.
    let engine = engine(
        common::plugin(&folder).approver(move |request: &ApprovalRequest| {
            *seen.borrow_mut() = request.sha256.clone();
            std::fs::write(&swap, b"not the library that was approved").unwrap();
            true
        }),
    );
    assert_eq!(outcome(&engine), Outcome::Loaded, "{:?}", engine.warnings());
    let extension = &engine.list::<torque_plugin_loader::Extension>()[0];
    let approved = approved.borrow().clone();
    assert_eq!(extension.sha256(), Some(approved.as_str()));
    let copy = extension
        .loaded_from()
        .expect("loaded from its private copy");
    assert!(
        copy.is_absolute() && copy.starts_with(std::path::absolute(&cache).unwrap()),
        "{}",
        copy.display()
    );
    assert_eq!(
        torque_plugin_loader::sha256_hex(&std::fs::read(copy).unwrap()),
        approved
    );
    assert!(std::fs::metadata(copy).unwrap().permissions().readonly());
    assert_ne!(
        torque_plugin_loader::sha256_hex(&std::fs::read(&original).unwrap()),
        approved
    );
    assert!(!stale.exists(), "the stale copy is cleaned away");
    let mut vm = common::server_vm(&engine);
    assert_eq!(
        vm.eval(b"return sampleGreet(\"you\");").unwrap(),
        b"Hello, you!"
    );
}

#[test]
fn a_library_built_for_another_abi_is_refused_before_it_is_loaded() {
    let folder = scratch("approval-other-abi");
    let mut bytes = std::fs::read(sample_library()).unwrap();
    let (at, version) = torque_plugin_loader::abi_version_at(&bytes).unwrap();
    assert_eq!(version, torque_plugin_loader::abi::TORQUE_ABI_VERSION);
    bytes[at..at + 4].copy_from_slice(&99u32.to_ne_bytes());
    std::fs::write(folder.join(library_file_name("native_sample")), &bytes).unwrap();
    std::fs::write(folder.join("native_sample.toml"), sample_manifest()).unwrap();
    let engine = engine(common::plugin(&folder).approver(|_: &ApprovalRequest| true));
    let Outcome::Refused(reason) = outcome(&engine) else {
        panic!("refused");
    };
    assert!(reason.contains("ABI version 99"), "{reason}");
    // Refused from the file: no copy was made to load.
    let cache = user_of(&folder).join("cache");
    assert_eq!(std::fs::read_dir(&cache).map(|d| d.count()).unwrap_or(0), 0);
}

#[test]
fn an_extension_whose_id_an_engine_plugin_has_is_refused_and_the_engine_builds() {
    let folder = sample_folder("approval-taken-id", &[]);
    // A second manifest of the same id in the folder: refused too, however it is named.
    std::fs::write(folder.join("zz_again.toml"), sample_manifest()).unwrap();
    let asked = Rc::new(RefCell::new(Vec::new()));
    let seen = Rc::clone(&asked);
    let engine = EngineBuilder::new()
        .add_plugins(common::CompiledSample)
        .add_plugins(NativeExtensions(common::plugin(&folder).approver(
            move |request: &ApprovalRequest| {
                seen.borrow_mut().push(request.id.clone());
                true
            },
        )))
        .enable_id("torque.native-extensions")
        .build()
        .expect("one wrong file does not stop the engine");
    let report = engine.resource::<LoadReport>().unwrap();
    assert_eq!(report.entries.len(), 2);
    for entry in &report.entries {
        let Outcome::Refused(reason) = &entry.outcome else {
            panic!("{} is refused", entry.manifest.display());
        };
        assert!(reason.contains("already has a plugin"), "{reason}");
    }
    assert!(
        asked.borrow().is_empty(),
        "nobody was asked, and none of its code ran"
    );
    assert!(engine.list::<torque_plugin_loader::Extension>().is_empty());
}

#[test]
fn of_two_manifests_with_one_id_only_the_first_loads() {
    let folder = sample_folder("approval-two-ids", &[]);
    std::fs::write(folder.join("zz_again.toml"), sample_manifest()).unwrap();
    let engine = engine(common::plugin(&folder).approver(|_: &ApprovalRequest| true));
    let report = engine.resource::<LoadReport>().unwrap();
    assert_eq!(report.entries[0].outcome, Outcome::Loaded);
    let Outcome::Refused(reason) = &report.entries[1].outcome else {
        panic!("the second is refused");
    };
    assert!(reason.contains("already has a plugin"), "{reason}");
}
