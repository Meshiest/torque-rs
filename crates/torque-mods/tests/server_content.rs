//! What a server sends is content, never script: on a client (and a server) the files mounted from a server cannot be read,
//! listed or run as script, whatever their names.

mod common;

use common::files;
use torque_engine::{
    AssetError, Assets, ClientStage, Engine, EngineBuilder, Plugin, PluginId, Spec,
};
use torque_mods::{ModError, Mods, ModsPlugin};
use torque_testkit::zip_of;
use torque_ts_vm::host::{FileRead, MemFiles};
use torque_ts_vm::testing::{console, eval};
use torque_ts_vm::vm::Vm;

struct Words(Vec<String>);
struct Secret;

#[derive(PluginId)]
#[plugin(id = "test.words")]
struct WordsPlugin;

impl Plugin for WordsPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine
            .assets()
            .loader::<Words>(&["words"], Spec::untrusted(1024), |bytes, _| {
                Ok(Words(
                    String::from_utf8_lossy(bytes)
                        .split_whitespace()
                        .map(str::to_string)
                        .collect(),
                ))
            });
        engine
            .assets()
            .loader::<Secret>(&["secret"], Spec::trusted(1024), |_, _| Ok(Secret));
    }
}

fn engine() -> Engine {
    EngineBuilder::new()
        .add_plugins((ModsPlugin::new(), WordsPlugin))
        .build()
        .expect("the engine builds")
}

/// The client script engine the way a host makes it: the files in after `Classes`, the entry script before `Entry`.
fn client(engine: &Engine, files: MemFiles) -> Vm {
    let mut vm = Vm::new();
    for &stage in ClientStage::ALL {
        if stage == ClientStage::Game {
            vm.set_file_host(files.clone());
        }
        if stage == ClientStage::Entry {
            vm.exec_file(b"main.cs", false)
                .expect("the entry script runs");
        }
        engine.replay(stage, &mut vm).expect("the stage replays");
    }
    vm
}

fn sent() -> Vec<(String, Vec<u8>)> {
    let evil: &[u8] = b"$pwned = $pwned @ \"x\"; function greet() { return \"evil\"; }";
    [
        ("art/pic.words", &b"one two"[..]),
        ("art/room.secret", b"s"),
        ("payload.txt", evil),
        ("main.cs", evil),
        ("scripts/evil.cs", evil),
        ("scripts/EVIL2.CS", evil),
        ("scripts/evil.cs.dso", b"\x01\x02"),
        ("gui/menu.gui", evil),
        ("maps/start.mis", evil),
        ("tools/x.ed.cs", evil),
    ]
    .into_iter()
    .map(|(p, b)| (p.to_string(), b.to_vec()))
    .collect()
}

#[test]
fn server_sent_script_never_executes_on_a_client() {
    let engine = engine();
    let mem = files(&[("main.cs", b"function greet() { return \"game\"; }")]);
    let mut vm = client(&engine, mem);
    let mods = Mods::of(&vm).expect("mods");

    // Content mounts; everything that could be script is left out.
    let mounted = mods
        .mount_server_files("server:1", sent())
        .expect("the files mount");
    assert_eq!(mounted, 3, "the pictures, the secret file and payload.txt");
    // A server's file is never copied to a script's name, which would get it past the content-only rule.
    for to in ["copied.cs", "x.cs.dso", "ui/x.GUI"] {
        assert_eq!(
            vm.copy_file(b"payload.txt", to.as_bytes()),
            Ok(false),
            "{to}"
        );
    }

    // Not as script by name...
    for name in [
        "scripts/evil.cs",
        "scripts/EVIL2.CS",
        "gui/menu.gui",
        "maps/start.mis",
        "tools/x.ed.cs",
    ] {
        let ran = eval(&mut vm, &format!("return exec(\"{name}\");"));
        assert_ne!(ran, "1", "{name}");
    }
    // ... and not under another name either: the policy refuses to exec anything a server sent.
    assert_eq!(eval(&mut vm, "return exec(\"payload.txt\");"), "0");
    assert!(
        console(&mut vm).contains("came from a server; a server's files are content, never script")
    );
    // The game's own main.cs is still the game's file: the server's copy was left out.
    assert_eq!(eval(&mut vm, "return exec(\"main.cs\");"), "1");
    assert_eq!(eval(&mut vm, "return $pwned;"), "", "nothing ran");
    assert_eq!(
        eval(&mut vm, "return greet();"),
        "game",
        "the game's own function is the one there is"
    );

    // The scripts are not readable, listable or measurable.
    assert_eq!(eval(&mut vm, "return isFile(\"scripts/evil.cs\");"), "0");
    assert_eq!(
        eval(&mut vm, "return isFile(\"scripts/evil.cs.dso\");"),
        "0"
    );
    assert!(mods.is_server_file("art/pic.words"));
    assert!(!mods.is_server_file("scripts/evil.cs"));
    assert!(
        !mods.is_server_file("main.cs"),
        "the game's own main.cs stays the game's"
    );

    // The content is there for the loaders, as untrusted.
    let assets = Assets::of(&vm).expect("assets");
    let words = assets
        .load::<Words>(vm.file_host(), "art/pic.words")
        .expect("content loads");
    assert_eq!(words.0, ["one", "two"]);
    assert!(matches!(
        assets.load::<Secret>(vm.file_host(), "art/room.secret"),
        Err(AssetError::Untrusted { .. })
    ));

    // Unmounted, it is gone.
    assert!(mods.unmount_server_files("server:1"));
    assert!(matches!(
        assets
            .load::<Words>(vm.file_host(), "art/pic.words")
            .map(drop),
        Ok(()) | Err(AssetError::Missing { .. })
    ));
    assert_eq!(eval(&mut vm, "return exec(\"payload.txt\");"), "0");
}

#[test]
fn a_mod_cannot_use_a_server_name_to_run_a_server_file() {
    // A mod's script that execs a path a server sent is refused like any other exec of it.
    let engine = engine();
    let zip = zip_of(&[
        ("mod.toml", b"id = \"m\"\nversion = \"1\"\n"),
        (
            "server.cs",
            b"function tryIt() { return exec(\"payload.txt\"); }",
        ),
    ]);
    let mem = files(&[("main.cs", b"// the game"), ("mods/m.zip", &zip)]);
    let mut vm = client(&engine, mem);
    let mods = Mods::of(&vm).expect("mods");
    mods.mount_server_files("s", sent()).expect("mounts");
    assert_eq!(eval(&mut vm, "return $pwned;"), "");
    assert_eq!(eval(&mut vm, "return exec(\"payload.txt\");"), "0");
    assert_eq!(eval(&mut vm, "return $pwned;"), "");
}

#[test]
fn server_files_are_held_to_the_same_limits_and_path_rules() {
    let engine = engine();
    let vm = client(&engine, files(&[("main.cs", b"")]));
    let mods = Mods::of(&vm).expect("mods");
    let bad = vec![("../escape.png".to_string(), vec![1u8])];
    assert!(matches!(
        mods.mount_server_files("s", bad),
        Err(ModError::BadPath { .. })
    ));
    let big = vec![(
        "big.bin".to_string(),
        vec![0u8; (mods.limits().file_bytes + 1) as usize],
    )];
    assert!(matches!(
        mods.mount_server_files("s", big),
        Err(ModError::FileTooLarge { .. })
    ));
    let clash = vec![
        ("a.png".to_string(), vec![1u8]),
        ("A.PNG".to_string(), vec![2u8]),
    ];
    assert!(matches!(
        mods.mount_server_files("s", clash),
        Err(ModError::Collision { .. })
    ));
    assert!(!mods.unmount_server_files("s"), "nothing mounted");
}

#[test]
fn all_server_mounts_together_stay_within_the_limits() {
    let limits = torque_mods::ModLimits {
        total_bytes: 1000,
        files: 3,
        ..torque_mods::ModLimits::default()
    };
    let engine = EngineBuilder::new()
        .add_plugins(ModsPlugin::new().limits(limits))
        .build()
        .expect("the engine builds");
    let vm = client(&engine, files(&[("main.cs", b"")]));
    let mods = Mods::of(&vm).expect("mods");
    let part = |name: &str, bytes: usize| vec![(name.to_string(), vec![1u8; bytes])];
    assert_eq!(mods.mount_server_files("a", part("a.png", 600)), Ok(1));
    // Another server's files would pass the total together.
    assert!(matches!(
        mods.mount_server_files("b", part("b.png", 600)),
        Err(ModError::TooManyBytes { .. })
    ));
    // Replacing a server's own mount counts only the new one.
    assert_eq!(mods.mount_server_files("a", part("a2.png", 900)), Ok(1));
    assert_eq!(mods.mount_server_files("a", part("a3.png", 100)), Ok(1));
    assert_eq!(mods.mount_server_files("b", part("b.png", 100)), Ok(1));
    // The files add up across servers too.
    let two = vec![
        ("c1.png".to_string(), vec![1u8]),
        ("c2.png".to_string(), vec![1u8]),
    ];
    assert!(matches!(
        mods.mount_server_files("c", two),
        Err(ModError::TooManyFiles { .. })
    ));
}

#[test]
fn a_lua_file_a_server_sent_is_data_that_nothing_runs() {
    let engine = engine();
    let mem = files(&[("main.cs", b"// the game")]);
    let mut vm = client(&engine, mem);
    let mods = Mods::of(&vm).expect("mods");
    let lua: &[u8] = b"$pwned = 1; os.execute('x')";
    let mounted = mods
        .mount_server_files(
            "server:1",
            vec![
                ("ui/panel.lua".to_string(), lua.to_vec()),
                ("ui/PANEL2.LUA".to_string(), lua.to_vec()),
                ("ui/readme.lua.txt".to_string(), lua.to_vec()),
                ("ui/panel.wasm".to_string(), vec![0, 97, 115, 109]),
            ],
        )
        .expect("the files mount");
    assert_eq!(mounted, 3, "the Lua files stay; the module is dropped");

    // Present, and readable as data.
    for name in ["ui/panel.lua", "ui/PANEL2.LUA", "ui/readme.lua.txt"] {
        assert_eq!(
            eval(&mut vm, &format!("return isFile(\"{name}\");")),
            "1",
            "{name}"
        );
        assert!(
            matches!(vm.file_host().read(name.as_bytes()), FileRead::Found(bytes) if bytes == lua),
            "{name}"
        );
        // Untrusted, whatever asks.
        assert!(mods.is_untrusted(name), "{name}");
        // And never run by the paths the engine runs files by.
        assert_eq!(
            eval(&mut vm, &format!("return exec(\"{name}\");")),
            "0",
            "{name}"
        );
    }
    assert!(
        !mods.is_untrusted("main.cs"),
        "the game's own file is trusted"
    );
    assert!(!mods.is_untrusted("ui/not-there.lua"));
    assert_eq!(eval(&mut vm, "return $pwned;"), "", "nothing ran");
    assert!(console(&mut vm).contains("came from a server"));
    assert_eq!(eval(&mut vm, "return isFile(\"ui/panel.wasm\");"), "0");
}
