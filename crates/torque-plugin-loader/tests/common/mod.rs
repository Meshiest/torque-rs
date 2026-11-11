//! What the loader's tests share: the sample extension built on its own (examples/native-extension), folders to load it
//! from, and the same plugin compiled into the engine.
//! tests/app (`torque-test-app`) also compiles this file through `#[path]`: a new dev-dependency here must be added there too, it
//! relies on tests/app sitting as deep as crates/<name> (`CARGO_MANIFEST_DIR/../..`), and `render` reads that crate's feature.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use torque_engine::{Engine, EngineBuilder, PluginId, ServerStage, VmRef, function, natives};
use torque_plugin_loader::{ApprovalRequest, NativeExtensions, NativeExtensionsPlugin};
use torque_ts_vm::vm::Vm;

/// The workspace's folder.
pub fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The build folder the tests use: CARGO_TARGET_DIR's, else the workspace's.
pub fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace().join("target"))
}

/// A new empty folder for `name` below the build folder (never the system's temporary folder).
pub fn scratch(name: &str) -> PathBuf {
    let dir = target_dir()
        .join("native-ext-tests")
        .join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(user_of(&dir));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The platform's file name of the library `name`.
pub fn library_file_name(name: &str) -> String {
    if cfg!(target_os = "windows") {
        format!("{name}.dll")
    } else if cfg!(target_os = "macos") {
        format!("lib{name}.dylib")
    } else {
        format!("lib{name}.so")
    }
}

/// The sample extension's library, built once per test binary with its own workspace and build folder.
pub fn sample_library() -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT
        .get_or_init(|| {
            let manifest = workspace().join("examples/native-extension/Cargo.toml");
            let target = target_dir().join("native-extension");
            let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
            let status = Command::new(cargo)
                .args(["build", "--quiet", "--manifest-path"])
                .arg(&manifest)
                .arg("--target-dir")
                .arg(&target)
                .status()
                .expect("cargo runs");
            assert!(status.success(), "the sample extension builds");
            let library = target
                .join("debug")
                .join(library_file_name("native_sample"));
            assert!(library.is_file(), "{} is built", library.display());
            library
        })
        .clone()
}

/// The sample's manifest, as in examples/native-extension.
pub fn sample_manifest() -> String {
    std::fs::read_to_string(workspace().join("examples/native-extension/native_sample.toml"))
        .unwrap()
}

/// A folder `name` holding the sample's library (with `extra` bytes appended) and its manifest.
pub fn sample_folder(name: &str, extra: &[u8]) -> PathBuf {
    let dir = scratch(name);
    let mut bytes = std::fs::read(sample_library()).unwrap();
    bytes.extend_from_slice(extra);
    std::fs::write(dir.join(library_file_name("native_sample")), bytes).unwrap();
    std::fs::write(dir.join("native_sample.toml"), sample_manifest()).unwrap();
    dir
}

/// The per-user folder the tests keep `folder`'s approvals and loaded copies in: beside it, never inside it.
pub fn user_of(folder: &Path) -> PathBuf {
    let name = folder.file_name().unwrap().to_string_lossy().into_owned();
    folder.with_file_name(format!("{name}-user"))
}

/// The plugin loading `folder`, with its own per-user folder ([`user_of`]).
pub fn plugin(folder: &Path) -> NativeExtensionsPlugin {
    NativeExtensionsPlugin::new(folder).user_folder(user_of(folder))
}

/// An engine that loads the extensions of `folder`, each approved.
pub fn loaded_engine(folder: &Path) -> Engine {
    EngineBuilder::new()
        .add_plugins(NativeExtensions(
            plugin(folder).approver(|_: &ApprovalRequest| true),
        ))
        .enable_id("torque.native-extensions")
        .build()
        .expect("the engine is built")
}

/// A server's script engine of `engine`.
pub fn server_vm(engine: &Engine) -> Vm {
    let mut vm = Vm::new();
    for &stage in ServerStage::ALL {
        engine.replay(stage, &mut vm).unwrap();
    }
    vm
}

/// The lines of `vm`'s natives listing that are the sample's.
pub fn sample_natives(vm: &Vm) -> String {
    vm.native_listing()
        .lines()
        .filter(|l| l.starts_with("sampleGreet ") || l.starts_with("Sample::"))
        .map(|l| format!("{l}\n"))
        .collect()
}

// The sample extension again, compiled into the engine as a plugin of the same id.

/// greets someone by name
#[function]
fn sample_greet(name: &str) -> String {
    format!("Hello, {name}!")
}

/// adds two or three whole numbers
#[function(ns = "Sample")]
fn add(a: i32, b: i32, c: Option<i32>) -> i32 {
    a.wrapping_add(b).wrapping_add(c.unwrap_or(0))
}

/// the greeting global, said twice
#[function(ns = "Sample", name = "greetingTwice")]
fn greeting_twice(vm: VmRef) -> String {
    let greeting =
        String::from_utf8_lossy(&vm.global("$Sample::Greeting").unwrap_or_default()).into_owned();
    format!("{greeting} {greeting}")
}

/// The four corners of a quad as two triangles, as the SDK's `Triangles::quad` lays them.
#[cfg(feature = "render")]
fn quad(corners: [[f32; 3]; 4], color: [f32; 4]) -> Vec<torque_plugin_loader::render::Vertex> {
    [0, 1, 2, 0, 2, 3]
        .map(|i| torque_plugin_loader::render::Vertex {
            position: corners[i],
            color,
        })
        .to_vec()
}

/// The sample compiled in.
pub struct CompiledSample;

impl torque_engine::PluginType for CompiledSample {
    const ID: PluginId = PluginId::new("example.native-sample");
}

impl torque_engine::Plugin for CompiledSample {
    fn build(&self, engine: &mut EngineBuilder) {
        engine
            .script()
            .natives(natives![sample_greet, add, greeting_twice])
            .global("$Sample::Greeting", "Hello");
        #[cfg(feature = "render")]
        {
            use torque_app::render::{EngineRender, Stage};
            use torque_plugin_loader::render::{Space, TrianglesPass};
            engine
                .render()
                .pass(Stage::Opaque, || {
                    TrianglesPass::new(|_, out| {
                        let corners = [
                            [-2.0, 0.0, 1.0],
                            [2.0, 0.0, 1.0],
                            [2.0, 0.0, 5.0],
                            [-2.0, 0.0, 5.0],
                        ];
                        out.push(Space::World, &quad(corners, [1.0, 0.35, 0.08, 1.0]));
                    })
                })
                .pass(Stage::Overlay, || {
                    TrianglesPass::new(|_, out| {
                        let corners = [
                            [-0.95, 0.6, 0.0],
                            [-0.6, 0.6, 0.0],
                            [-0.6, 0.95, 0.0],
                            [-0.95, 0.95, 0.0],
                        ];
                        out.push(Space::Clip, &quad(corners, [0.1, 0.6, 1.0, 0.8]));
                    })
                });
        }
    }
}
