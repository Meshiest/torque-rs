//! The sample extension (examples/native-extension), built on its own as a library and loaded from a plugins folder: its
//! console functions and global work, and its natives are the same plugin's compiled in, to the listing line (the
//! snapshot `tests/natives/sample.txt`; `TORQUE_BLESS=1` rewrites it).

mod common;

use std::path::Path;

use common::{CompiledSample, loaded_engine, sample_folder, sample_natives, server_vm};
use torque_engine::EngineBuilder;
use torque_plugin_loader::{Extension, LoadReport, Outcome};
use torque_ts_vm::listing::check_listing;

#[test]
fn the_sample_loads_and_its_functions_run() {
    let folder = sample_folder("sample-runs", &[]);
    let engine = loaded_engine(&folder);
    // Without the `render` feature the sample's passes are skipped, and that is the one warning.
    let expected = if cfg!(feature = "render") { 0 } else { 1 };
    assert_eq!(engine.warnings().len(), expected, "{:?}", engine.warnings());
    assert!(engine.is_plugin_added_id("example.native-sample"));
    let report = engine.resource::<LoadReport>().unwrap();
    let entry = report
        .get("example.native-sample")
        .expect("the sample's entry");
    assert_eq!(entry.outcome, Outcome::Loaded);
    assert_eq!(entry.sha256.as_ref().map(String::len), Some(64));
    let extensions = engine.list::<Extension>();
    assert_eq!(extensions.len(), 1);
    assert_eq!(
        (extensions[0].id(), extensions[0].version()),
        ("example.native-sample", "0.1.0")
    );

    let mut vm = server_vm(&engine);
    assert_eq!(
        vm.eval(b"return sampleGreet(\"world\");").unwrap(),
        b"Hello, world!"
    );
    assert_eq!(vm.eval(b"return Sample::add(2, 3);").unwrap(), b"5");
    assert_eq!(vm.eval(b"return Sample::add(2, 3, 4);").unwrap(), b"9");
    assert_eq!(
        vm.eval(b"return Sample::greetingTwice();").unwrap(),
        b"Hello Hello"
    );
    vm.eval(b"$Sample::Greeting = \"Hi\";").unwrap();
    assert_eq!(
        vm.eval(b"return Sample::greetingTwice();").unwrap(),
        b"Hi Hi"
    );
    assert!(!extensions[0].is_disabled());
}

#[test]
fn compiled_in_and_loaded_have_the_same_natives() {
    let folder = sample_folder("sample-natives", &[]);
    let loaded = sample_natives(&server_vm(&loaded_engine(&folder)));
    let compiled_engine = EngineBuilder::new()
        .add_plugins(CompiledSample)
        .build()
        .unwrap();
    let compiled = sample_natives(&server_vm(&compiled_engine));
    assert_eq!(loaded.lines().count(), 3, "{loaded}");
    assert_eq!(loaded, compiled);
    check_listing(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/natives/sample.txt"),
        &loaded,
    );
}
