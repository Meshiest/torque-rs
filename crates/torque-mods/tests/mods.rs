//! The default layout end to end: zip and folder mods overriding scripts, adding datablocks and assets, load order, the
//! prerequisites, disabling, the limits' refusals, and what a server sends.

mod common;

use common::{files, server};
use torque_engine::{
    AssetError, Assets, ClientStage, Engine, EngineBuilder, Plugin, PluginId, Spec,
};
use torque_mods::{MANIFEST, ModLimits, ModStatus, Mods, ModsConfig, ModsPlugin, Version};
use torque_testkit::zip_of;
use torque_ts_vm::testing::{console, eval};
use torque_ts_vm::vm::Vm;

/// A word list: what a test loader makes of a `.words` file.
struct Words(Vec<String>);

/// A loader that was not written for hostile bytes.
struct Secret;

#[derive(PluginId)]
#[plugin(id = "test.words")]
struct WordsPlugin;

impl Plugin for WordsPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine
            .assets()
            .loader::<Words>(&["words"], Spec::untrusted(1024), |bytes, _| {
                let text = std::str::from_utf8(bytes).map_err(|_| "not text".to_string())?;
                Ok(Words(text.split_whitespace().map(str::to_string).collect()))
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

const BASE: &[(&str, &[u8])] = &[(
    "server.cs",
    b"function greet() { return \"base\"; }\nfunction base() { return \"kept\"; }\n",
)];

fn game(extra: &[(&str, &[u8])]) -> torque_ts_vm::host::MemFiles {
    let mut all: Vec<(&str, &[u8])> = BASE.to_vec();
    all.extend_from_slice(extra);
    files(&all)
}

fn hello_zip() -> Vec<u8> {
    zip_of(&[
        (MANIFEST, b"id = \"hello\"\nversion = \"1.0.0\"\n"),
        (
            "server.cs",
            b"function greet() { return \"mod\"; }\ndatablock SimDataBlock(HelloBlock) { };\n",
        ),
        ("art/greeting.words", b"hello world"),
        ("art/room.secret", b"x"),
    ])
}

fn mod_toml(id: &str, extra: &str) -> Vec<u8> {
    format!("id = \"{id}\"\nversion = \"1.0\"\n{extra}").into_bytes()
}

fn order_of(vm: &mut Vm) -> String {
    eval(vm, "return getModList();")
}

fn reason(vm: &mut Vm, id: &str) -> String {
    eval(vm, &format!("return getModField({id}, reason);"))
}

#[test]
fn a_zip_mod_overrides_a_function_adds_a_datablock_and_adds_an_asset() {
    let engine = engine();
    let zip = hello_zip();
    let mut vm = server(&engine, game(&[("mods/hello.zip", &zip)]), "server.cs");

    // The package overrides the game's function; what it does not override is the game's.
    assert_eq!(eval(&mut vm, "return greet();"), "mod");
    assert_eq!(eval(&mut vm, "return base();"), "kept");
    // The mod's own server.cs did not replace the game's file of that name.
    assert!(console(&mut vm).contains("Executing mods/hello/server.cs."));
    // The datablock exists.
    assert_eq!(eval(&mut vm, "return isObject(HelloBlock);"), "1");
    // The asset is one a loader loads, from the mod's own path, and it is untrusted.
    let assets = Assets::of(&vm).expect("the asset cache");
    let words = assets
        .load::<Words>(vm.file_host(), "art/greeting.words")
        .expect("the asset loads");
    assert_eq!(words.0, ["hello", "world"]);
    assert!(matches!(
        assets.load::<Secret>(vm.file_host(), "art/room.secret"),
        Err(AssetError::Untrusted { .. })
    ));
    assert_eq!(
        eval(&mut vm, "return getModField(hello, version);"),
        "1.0.0"
    );
    assert_eq!(
        eval(&mut vm, "return getModField(\"hello\", \"package\");"),
        "Mod_hello"
    );
}

#[test]
fn a_client_over_its_servers_script_engine_keeps_the_servers_mods_and_runs_none_again() {
    let engine = engine();
    let zip = hello_zip();
    let mut vm = server(&engine, game(&[("mods/hello.zip", &zip)]), "server.cs");
    let mods = Mods::of(&vm).expect("the server's mods");
    console(&mut vm);
    for &stage in ClientStage::ALL {
        engine
            .replay_shared(stage, &mut vm)
            .expect("the stage replays");
    }
    let kept = Mods::of(&vm).expect("the mods");
    assert!(std::rc::Rc::ptr_eq(&mods, &kept), "the server's mods stand");
    let log = console(&mut vm);
    assert!(!log.contains("Executing mods/hello/server.cs."), "{log}");
    assert_eq!(eval(&mut vm, "return greet();"), "mod");
}

#[test]
fn a_folder_mod_loads_like_a_zip() {
    let engine = engine();
    let mut vm = server(
        &engine,
        game(&[
            ("mods/folk/mod.toml", &mod_toml("folk", "")),
            (
                "mods/folk/server.cs",
                b"function greet() { return \"folk\"; }",
            ),
            ("mods/folk/art/a.words", b"one"),
        ]),
        "server.cs",
    );
    assert_eq!(eval(&mut vm, "return greet();"), "folk");
    let assets = Assets::of(&vm).expect("the asset cache");
    assert!(assets.load::<Words>(vm.file_host(), "art/a.words").is_ok());
}

#[test]
fn a_mods_file_replaces_the_games_file_of_its_path() {
    let engine = engine();
    let vm = server(
        &engine,
        game(&[
            ("art/a.words", b"game"),
            ("mods/one/mod.toml", &mod_toml("one", "")),
            ("mods/one/art/a.words", b"one"),
            (
                "mods/two/mod.toml",
                &mod_toml("two", "load_after = [\"one\"]"),
            ),
            ("mods/two/art/a.words", b"two"),
        ]),
        "server.cs",
    );
    let assets = Assets::of(&vm).expect("the asset cache");
    let words = assets
        .load::<Words>(vm.file_host(), "art/a.words")
        .expect("loads");
    assert_eq!(words.0, ["two"], "the later mount wins");
}

#[test]
fn load_order_follows_requires_then_the_id() {
    let engine = engine();
    let mut vm = server(
        &engine,
        game(&[
            ("mods/zeta/mod.toml", &mod_toml("zeta", "")),
            (
                "mods/beta/mod.toml",
                &mod_toml("beta", "requires = [\"zeta\"]"),
            ),
            (
                "mods/alpha/mod.toml",
                &mod_toml("alpha", "load_after = [\"beta\"]"),
            ),
            (
                "mods/gamma/mod.toml",
                &mod_toml("gamma", "load_before = [\"zeta\"]"),
            ),
        ]),
        "server.cs",
    );
    assert_eq!(order_of(&mut vm), "gamma zeta beta alpha");
}

#[test]
fn a_cycle_skips_its_mods_with_a_warning_and_the_rest_load() {
    let engine = engine();
    let mut vm = server(
        &engine,
        game(&[
            ("mods/a/mod.toml", &mod_toml("a", "requires = [\"b\"]")),
            ("mods/b/mod.toml", &mod_toml("b", "requires = [\"c\"]")),
            ("mods/c/mod.toml", &mod_toml("c", "requires = [\"a\"]")),
            ("mods/d/mod.toml", &mod_toml("d", "requires = [\"a\"]")),
            ("mods/ok/mod.toml", &mod_toml("ok", "")),
        ]),
        "server.cs",
    );
    assert_eq!(order_of(&mut vm), "ok");
    let log = console(&mut vm);
    assert!(log.contains("loop of requirements ("), "{log}");
    assert!(log.contains("the mod `d` is not loaded"), "{log}");
    let mods = Mods::of(&vm).expect("mods");
    assert!(matches!(
        mods.get("a").expect("a").status,
        ModStatus::Skipped(_)
    ));
}

#[test]
fn missing_and_wrong_prerequisites_skip_the_mod_with_the_reason() {
    let engine = engine();
    let mut vm = server(
        &engine,
        game(&[
            ("mods/core/mod.toml", b"id = \"core\"\nversion = \"1.2\"\n"),
            (
                "mods/new/mod.toml",
                &mod_toml("new", "requires = [\"core >= 2\"]"),
            ),
            (
                "mods/old/mod.toml",
                &mod_toml("old", "requires = [\"core < 2\"]"),
            ),
            (
                "mods/lost/mod.toml",
                &mod_toml("lost", "requires = [\"nothing\"]"),
            ),
            (
                "mods/audio/mod.toml",
                &mod_toml("audio", "requires_plugins = [\"no.such.plugin\"]"),
            ),
            (
                "mods/on/mod.toml",
                &mod_toml("on", "requires_plugins = [\"test.words\"]"),
            ),
            (
                "mods/child/mod.toml",
                &mod_toml("child", "requires = [\"lost\"]"),
            ),
        ]),
        "server.cs",
    );
    assert_eq!(order_of(&mut vm), "core old on");
    let log = console(&mut vm);
    assert!(
        log.contains("it requires `core >= 2`, but that mod is version 1.2"),
        "{log}"
    );
    assert!(log.contains("requires the mod `nothing`"), "{log}");
    assert!(
        log.contains("requires the plugin `no.such.plugin`"),
        "{log}"
    );
    assert!(log.contains("requires the mod `lost`"), "{log}");
}

#[test]
fn a_client_side_mod_is_not_loaded_by_a_server_and_a_duplicate_id_is_refused() {
    let engine = engine();
    let mut vm = server(
        &engine,
        game(&[
            ("mods/gui/mod.toml", &mod_toml("gui", "client_side = true")),
            ("mods/one/mod.toml", &mod_toml("same", "")),
            ("mods/two/mod.toml", &mod_toml("same", "")),
        ]),
        "server.cs",
    );
    assert_eq!(order_of(&mut vm), "same");
    let log = console(&mut vm);
    assert!(
        log.contains("another mod already has the id `same`"),
        "{log}"
    );
    assert!(
        !log.contains("client-side"),
        "a mod for the other side is no fault: {log}"
    );
}

#[test]
fn a_package_name_is_a_valid_identifier_made_from_the_id_and_two_ids_may_not_share_one() {
    assert_eq!(
        torque_mods::package_name("bigger-bombs"),
        "Mod_bigger_bombs"
    );
    assert_eq!(torque_mods::package_name("a.b_c"), "Mod_a_b_c");
    let engine = engine();
    let mut vm = server(
        &engine,
        game(&[
            ("mods/a/mod.toml", &mod_toml("big-bombs", "")),
            (
                "mods/a/server.cs",
                b"function bomb() { return \"hyphen\"; }",
            ),
            ("mods/b/mod.toml", &mod_toml("big_bombs", "")),
            (
                "mods/b/server.cs",
                b"function bomb() { return \"underscore\"; }",
            ),
        ]),
        "server.cs",
    );
    // The first found (by id) loads; the second maps to the same package and is skipped with the reason.
    assert_eq!(order_of(&mut vm), "big-bombs");
    assert_eq!(eval(&mut vm, "return bomb();"), "hyphen");
    assert!(reason(&mut vm, "big_bombs").contains("Mod_big_bombs"));
    let log = console(&mut vm);
    assert!(log.contains("the mod `big_bombs` is skipped"), "{log}");
}

#[test]
fn disabling_deactivates_the_package_and_the_override_is_gone() {
    let engine = engine();
    let zip = hello_zip();
    let dependent = zip_of(&[
        (
            MANIFEST,
            b"id = \"child\"\nversion = \"1\"\nrequires = [\"hello\"]\n",
        ),
        ("server.cs", b"function childOnly() { return \"child\"; }"),
        ("art/child.words", b"c"),
    ]);
    let mut vm = server(
        &engine,
        game(&[("mods/hello.zip", &zip), ("mods/child.zip", &dependent)]),
        "server.cs",
    );
    assert_eq!(eval(&mut vm, "return greet();"), "mod");
    assert_eq!(eval(&mut vm, "return childOnly();"), "child");

    assert_eq!(eval(&mut vm, "return disableMod(hello);"), "1");
    assert_eq!(
        eval(&mut vm, "return greet();"),
        "base",
        "the override is gone"
    );
    assert_eq!(eval(&mut vm, "return isModEnabled(hello);"), "0");
    assert_eq!(
        eval(&mut vm, "return isModEnabled(child);"),
        "0",
        "the mod that needs it goes too"
    );
    assert_eq!(
        eval(&mut vm, "return childOnly();"),
        "",
        "its package is deactivated"
    );
    let assets = Assets::of(&vm).expect("assets");
    assert!(
        matches!(
            assets.load::<Words>(vm.file_host(), "art/child.words"),
            Err(AssetError::Missing { .. })
        ),
        "its files are unmounted"
    );
    let mods = Mods::of(&vm).expect("mods");
    assert_eq!(
        mods.get("hello").expect("hello").status,
        ModStatus::Disabled
    );
    assert!(vm.namespaces().active_packages().is_empty());

    // The child cannot come back before the mod it needs.
    assert_eq!(eval(&mut vm, "return enableMod(child);"), "0");
    assert_eq!(eval(&mut vm, "return enableMod(hello);"), "1");
    assert_eq!(eval(&mut vm, "return enableMod(child);"), "1");
    assert_eq!(eval(&mut vm, "return greet();"), "mod");
    assert_eq!(eval(&mut vm, "return childOnly();"), "child");
    assert_eq!(
        vm.namespaces().active_packages(),
        [b"Mod_hello".to_vec(), b"Mod_child".to_vec()],
        "the load order is the stack order"
    );
}

#[test]
fn a_mod_the_config_disables_starts_off_and_can_be_enabled() {
    let mut builder = EngineBuilder::new();
    builder.insert_resource(ModsConfig {
        disable: vec!["hello".to_string()],
        ..ModsConfig::default()
    });
    let engine = builder
        .add_plugins(ModsPlugin::new())
        .build()
        .expect("builds");
    let zip = hello_zip();
    let mut vm = server(&engine, game(&[("mods/hello.zip", &zip)]), "server.cs");
    assert_eq!(eval(&mut vm, "return greet();"), "base");
    assert_eq!(
        eval(&mut vm, "return getModField(hello, status);"),
        "disabled"
    );
    assert_eq!(eval(&mut vm, "return enableMod(hello);"), "1");
    assert_eq!(
        eval(&mut vm, "return greet();"),
        "mod",
        "its scripts ran for the stage that had run"
    );
}

#[test]
fn the_disabled_pref_is_the_default_enable_list_and_is_kept() {
    let engine = engine();
    let zip = hello_zip();
    let mut vm = server(&engine, game(&[("mods/hello.zip", &zip)]), "server.cs");
    eval(&mut vm, "disableMod(hello);");
    assert_eq!(eval(&mut vm, "return $Pref::Mods::Disabled;"), "hello");
    eval(&mut vm, "enableMod(hello);");
    assert_eq!(eval(&mut vm, "return $Pref::Mods::Disabled;"), "");
}

#[test]
fn stages_run_once_and_unknown_ones_are_refused() {
    let engine = engine();
    let zip = hello_zip();
    let mut vm = server(&engine, game(&[("mods/hello.zip", &zip)]), "server.cs");
    assert_eq!(eval(&mut vm, "return getModStages();"), "server client");
    assert_eq!(
        eval(&mut vm, "return loadMods(server);"),
        "0",
        "it ran already"
    );
    assert_eq!(
        eval(&mut vm, "return loadMods(client);"),
        "0",
        "the mod has no client script"
    );
    assert_eq!(eval(&mut vm, "return loadMods(nonsense);"), "0");
    assert!(console(&mut vm).contains("`nonsense` is not a mod stage"));
}

#[test]
fn a_mod_with_a_bad_manifest_is_listed_with_why() {
    let engine = engine();
    let mut vm = server(
        &engine,
        game(&[
            ("mods/nomanifest/server.cs", b"function x() {}"),
            ("mods/badver/mod.toml", b"id = \"badver\"\nversion = \"one\"\n"),
            ("mods/noid/mod.toml", b"version = \"1\"\n"),
            (
                "mods/odd/mod.toml",
                b"id = \"odd\"\nversion = \"1\"\nspeed = 3\n[scripts]\nserver = \"missing.cs\"\nnonsense = \"x.cs\"\n",
            ),
            ("mods/_hidden/mod.toml", &mod_toml("hidden", "")),
        ]),
        "server.cs",
    );
    assert_eq!(order_of(&mut vm), "odd");
    assert_eq!(eval(&mut vm, "return getModCount();"), "4");
    assert!(reason(&mut vm, "nomanifest").contains("no mod.toml"));
    assert!(reason(&mut vm, "badver").contains("is not a version"));
    assert!(reason(&mut vm, "noid").contains("no `id`"));
    let log = console(&mut vm);
    assert!(
        log.contains("the key `speed` of mod.toml is not known"),
        "{log}"
    );
    assert!(
        log.contains("the script `missing.cs` is not in the mod"),
        "{log}"
    );
    assert!(
        log.contains("the stage `nonsense` is not one the game declared"),
        "{log}"
    );
}

#[test]
fn over_the_limits_a_mod_is_refused_with_the_reason() {
    let limits = ModLimits {
        files: 3,
        file_bytes: 100,
        depth: 3,
        ..ModLimits::default()
    };
    let engine = EngineBuilder::new()
        .add_plugins(ModsPlugin::new().limits(limits))
        .build()
        .expect("builds");
    let many = zip_of(&[
        (MANIFEST, &mod_toml("many", "")),
        ("a", b"1"),
        ("b", b"1"),
        ("c", b"1"),
    ]);
    let big = zip_of(&[(MANIFEST, &mod_toml("big", "")), ("x.bin", &[7u8; 101])]);
    let deep = zip_of(&[(MANIFEST, &mod_toml("deep", "")), ("a/b/c/d.txt", b"1")]);
    let mut vm = server(
        &engine,
        game(&[
            ("mods/many.zip", &many),
            ("mods/big.zip", &big),
            ("mods/deep.zip", &deep),
        ]),
        "server.cs",
    );
    assert_eq!(order_of(&mut vm), "");
    assert!(reason(&mut vm, "many").contains("4 files; a mod may have at most 3"));
    assert!(reason(&mut vm, "big").contains("101 bytes"));
    assert!(reason(&mut vm, "deep").contains("more than 3 folders deep"));
}

#[test]
fn a_zip_that_unpacks_to_far_more_than_it_takes_is_refused() {
    let engine = engine();
    let bomb = zip_of(&[
        (MANIFEST, &mod_toml("bomb", "")),
        ("zeros.bin", &vec![0u8; 8 << 20]),
    ]);
    assert!(bomb.len() < 100 << 10);
    let mut vm = server(&engine, game(&[("mods/bomb.zip", &bomb)]), "server.cs");
    assert!(reason(&mut vm, "bomb").contains("more than 100 to 1"));
}

#[test]
fn malformed_and_hostile_zips_are_refused_with_a_reason() {
    let engine = engine();
    let ok = mod_toml("x", "");
    let hostile: Vec<(&str, Vec<u8>)> = vec![
        ("dotdot", zip_of(&[(MANIFEST, &ok), ("../evil.cs", b"x")])),
        ("absolute", zip_of(&[(MANIFEST, &ok), ("/evil.cs", b"x")])),
        ("drive", zip_of(&[(MANIFEST, &ok), ("C:/evil.cs", b"x")])),
        ("nul", zip_of(&[(MANIFEST, &ok), ("a\0b.cs", b"x")])),
        (
            "backslash",
            zip_of(&[(MANIFEST, &ok), ("..\\evil.cs", b"x")]),
        ),
        (
            "casing",
            zip_of(&[(MANIFEST, &ok), ("a.cs", b"x"), ("A.CS", b"y")]),
        ),
        ("garbage", b"this is not a zip".to_vec()),
        ("truncated", hello_zip()[..40].to_vec()),
    ];
    let mem = game(&[]);
    for (name, bytes) in &hostile {
        mem.insert(&format!("mods/{name}.zip"), bytes);
    }
    let mut vm = server(&engine, mem, "server.cs");
    assert_eq!(order_of(&mut vm), "");
    for (name, _) in &hostile {
        assert!(!reason(&mut vm, name).is_empty(), "{name} was not refused");
    }
    assert!(reason(&mut vm, "dotdot").contains("refused"));
    assert!(reason(&mut vm, "casing").contains("differ only in case"));
    assert!(reason(&mut vm, "garbage").contains("not a zip"));
}

#[test]
fn compiled_scripts_are_left_out_of_a_mod() {
    let engine = engine();
    let zip = zip_of(&[
        (MANIFEST, &mod_toml("dso", "")),
        ("server.cs", b"function dsoTest() { return 1; }"),
        ("server.cs.dso", b"\x00\x01\x02"),
    ]);
    let mut vm = server(&engine, game(&[("mods/dso.zip", &zip)]), "server.cs");
    assert_eq!(eval(&mut vm, "return dsoTest();"), "1");
    assert_eq!(
        eval(&mut vm, "return isFile(\"mods/dso/server.cs.dso\");"),
        "0"
    );
    assert!(console(&mut vm).contains("is compiled script"));
}

#[test]
fn a_zip_with_one_top_folder_reads_as_that_folder() {
    let engine = engine();
    let zip = zip_of(&[
        ("pack/mod.toml", &mod_toml("packed", "")),
        ("pack/server.cs", b"function packed() { return 7; }"),
    ]);
    let mut vm = server(&engine, game(&[("mods/pack.zip", &zip)]), "server.cs");
    assert_eq!(eval(&mut vm, "return packed();"), "7");
}

#[test]
fn natives_list_the_mods() {
    let engine = engine();
    let zip = hello_zip();
    let mut vm = server(&engine, game(&[("mods/hello.zip", &zip)]), "server.cs");
    assert_eq!(eval(&mut vm, "return getModCount();"), "1");
    assert_eq!(eval(&mut vm, "return getModField(hello, files);"), "4");
    eval(&mut vm, "dumpMods();");
    assert!(console(&mut vm).contains("0: hello 1.0.0 (enabled) from mods/hello.zip"));
}

#[test]
fn versions_compare_by_number() {
    let a: Version = "1.10".parse().expect("version");
    assert!(a > "1.9".parse().expect("version"));
}

#[test]
fn a_mod_the_prefs_switch_off_is_never_mounted_while_the_entry_script_runs() {
    let engine = engine();
    let entry: &[u8] = b"$Pref::Mods::Disabled = \"evil\";
$statusDuring = getModField(\"evil\", \"status\");
$artDuring = isFile(\"art/x.words\");
$enableDuring = enableMod(\"good\");
exec(\"lib.cs\");
";
    let mem = files(&[
        ("server.cs", entry),
        ("lib.cs", b"$lib = \"game\";"),
        ("mods/evil/mod.toml", &mod_toml("evil", "")),
        ("mods/evil/lib.cs", b"$lib = \"evil\";"),
        ("mods/good/mod.toml", &mod_toml("good", "")),
        ("mods/good/art/x.words", b"x"),
    ]);
    let mut vm = server(&engine, mem, "server.cs");
    // The entry script saw the game's files only, and no mod could be switched on or off yet.
    assert_eq!(
        eval(&mut vm, "return $lib;"),
        "game",
        "the game's own file ran"
    );
    assert_eq!(eval(&mut vm, "return $statusDuring;"), "pending");
    assert_eq!(
        eval(&mut vm, "return $artDuring;"),
        "0",
        "no mod's file was there"
    );
    assert_eq!(eval(&mut vm, "return $enableDuring;"), "0");
    // After it, the list the entry script loaded decides: evil was never mounted, good is.
    assert_eq!(
        eval(&mut vm, "return getModField(evil, status);"),
        "disabled"
    );
    assert_eq!(
        eval(&mut vm, "return getModField(good, status);"),
        "enabled"
    );
    assert_eq!(eval(&mut vm, "return isFile(\"art/x.words\");"), "1");
    assert_eq!(
        eval(&mut vm, "$lib = \"\"; exec(\"lib.cs\"); return $lib;"),
        "game"
    );
    assert_eq!(
        order_of(&mut vm),
        "evil good",
        "the list is accurate: the mod is found, just off"
    );
}

#[test]
fn functions_of_a_game_script_a_mod_execs_stay_out_of_its_package() {
    let engine = engine();
    let mem = files(&[
        ("server.cs", b""),
        (
            "shared/helper.cs",
            b"function helper() { return \"game-helper\"; }",
        ),
        ("mods/m/mod.toml", &mod_toml("m", "")),
        (
            "mods/m/server.cs",
            b"exec(\"shared/helper.cs\"); exec(\"./own.cs\"); function modFn() { return \"mod\"; }",
        ),
        ("mods/m/own.cs", b"function ownFn() { return \"own\"; }"),
    ]);
    let mut vm = server(&engine, mem, "server.cs");
    assert_eq!(eval(&mut vm, "return modFn();"), "mod");
    assert_eq!(eval(&mut vm, "return ownFn();"), "own");
    assert_eq!(eval(&mut vm, "return helper();"), "game-helper");
    eval(&mut vm, "disableMod(m);");
    assert_eq!(eval(&mut vm, "return modFn();"), "");
    assert_eq!(
        eval(&mut vm, "return ownFn();"),
        "",
        "the mod's own file is in its package"
    );
    assert_eq!(
        eval(&mut vm, "return helper();"),
        "game-helper",
        "the game's file declared into the base layer"
    );
}

#[test]
fn a_nested_stage_does_not_end_the_outer_mods_package_early() {
    let engine = engine();
    let mem = files(&[
        ("server.cs", b""),
        ("mods/a/mod.toml", &mod_toml("a", "")),
        (
            "mods/a/server.cs",
            b"function aBefore() { return 1; } loadMods(client); function aAfter() { return 2; }",
        ),
        ("mods/b/mod.toml", &mod_toml("b", "")),
        ("mods/b/client.cs", b"function bFn() { return 3; }"),
    ]);
    let mut vm = server(&engine, mem, "server.cs");
    assert_eq!(
        eval(&mut vm, "return bFn();"),
        "3",
        "b's client stage ran inside a's script"
    );
    assert_eq!(eval(&mut vm, "return aAfter();"), "2");
    eval(&mut vm, "disableMod(b);");
    assert_eq!(eval(&mut vm, "return bFn();"), "");
    assert_eq!(
        eval(&mut vm, "return aAfter();"),
        "2",
        "a's function after the nested stage is a's, not b's"
    );
    eval(&mut vm, "disableMod(a);");
    assert_eq!(eval(&mut vm, "return aBefore();"), "");
    assert_eq!(
        eval(&mut vm, "return aAfter();"),
        "",
        "it was in a's package, not the base layer"
    );
}

#[test]
fn a_prefix_read_of_a_mods_file_is_the_start_of_what_a_read_gives() {
    use torque_ts_vm::host::FileRead;
    let engine = engine();
    let vm = server(
        &engine,
        game(&[
            ("art/a.words", b"game words"),
            ("mods/one/mod.toml", &mod_toml("one", "")),
            ("mods/one/art/a.words", b"one mod's words"),
        ]),
        "server.cs",
    );
    let host = vm.file_host();
    assert_eq!(
        host.read_prefix(b"art/a.words", 3),
        FileRead::Found(b"one".to_vec())
    );
    assert_eq!(
        host.read_prefix(b"server.cs", 8),
        FileRead::Found(b"function".to_vec())
    );
    assert_eq!(host.read_prefix(b"art/none.words", 8), FileRead::Missing);
}

#[test]
fn a_prefix_read_of_a_zip_mods_file_is_its_start() {
    use torque_ts_vm::host::FileRead;
    let engine = engine();
    let big = vec![b'w'; 2 << 20];
    let zip = zip_of(&[
        (MANIFEST, b"id = \"big\"\nversion = \"1.0.0\"\n"),
        ("art/big.words", &big),
    ]);
    let vm = server(&engine, game(&[("mods/big.zip", &zip)]), "server.cs");
    let host = vm.file_host();
    assert_eq!(
        host.read_prefix(b"art/big.words", 4),
        FileRead::Found(b"wwww".to_vec())
    );
    assert_eq!(host.read_prefix(b"art/none.words", 4), FileRead::Missing);
}

fn info_of(vm: &Vm, id: &str) -> torque_mods::ModInfo {
    Mods::of(vm)
        .expect("the mods")
        .get(id)
        .expect("the mod is listed")
        .info
}

#[test]
fn client_script_the_manifest_asks_for_scripts_and_a_bad_request_is_a_warning() {
    let engine = engine();
    let mut vm = server(
        &engine,
        game(&[
            (
                "mods/good/mod.toml",
                &mod_toml(
                    "good",
                    "[client_script]\nentry = \"client/main.lua\"\ntiers = [\"ui\", \"net\", \"ui\"]\n",
                ),
            ),
            ("mods/good/client/main.lua", b"realm.log('x')"),
            (
                "mods/missing/mod.toml",
                &mod_toml("missing", "[client_script]\nentry = \"client/main.lua\"\n"),
            ),
            (
                "mods/escape/mod.toml",
                &mod_toml("escape", "[client_script]\nentry = \"../x.lua\"\n"),
            ),
            ("mods/escape/x.lua", b"x"),
            (
                "mods/notlua/mod.toml",
                &mod_toml("notlua", "[client_script]\nentry = \"client/main.cs\"\n"),
            ),
            ("mods/notlua/client/main.cs", b"x"),
            (
                "mods/staged/mod.toml",
                &mod_toml(
                    "staged",
                    "[client_script]\nentry = \"main.lua\"\n[scripts]\nserver = \"main.lua\"\n",
                ),
            ),
            ("mods/staged/main.lua", b"x"),
            (
                "mods/noentry/mod.toml",
                &mod_toml("noentry", "[client_script]\ntiers = [\"ui\"]\n"),
            ),
            (
                "mods/oddtier/mod.toml",
                &mod_toml(
                    "oddtier",
                    "[client_script]\nentry = \"m.lua\"\ntiers = [\"UI;\"]\n",
                ),
            ),
            ("mods/oddtier/m.lua", b"x"),
            (
                "mods/quirk/mod.toml",
                &mod_toml(
                    "quirk",
                    "[client_script]\nentry = \"m.lua\"\nwhere = \"/etc\"\n",
                ),
            ),
            ("mods/quirk/m.lua", b"x"),
        ]),
        "server.cs",
    );
    let good = info_of(&vm, "good").client_script.expect("asked for");
    assert_eq!(good.entry, "client/main.lua");
    assert_eq!(good.tiers, ["ui", "net"], "each once, in order");
    for id in [
        "missing", "escape", "notlua", "staged", "noentry", "oddtier",
    ] {
        assert_eq!(info_of(&vm, id).client_script, None, "{id}");
        assert!(
            info_of(&vm, id)
                .warnings
                .iter()
                .any(|w| w.contains("no client scripts")),
            "{id}: {:?}",
            info_of(&vm, id).warnings
        );
    }
    assert!(info_of(&vm, "quirk").client_script.is_some());
    let log = console(&mut vm);
    assert!(
        log.contains("the key `where` of [client_script] is not known"),
        "{log}"
    );
}

#[test]
fn client_script_a_required_mod_is_offered_with_its_entry_tiers_and_content_hash() {
    let engine = EngineBuilder::new()
        .add_plugins((ModsPlugin::new().required_mods(), WordsPlugin))
        .build()
        .expect("the engine builds");
    let vm = server(
        &engine,
        game(&[
            (
                "mods/hud/mod.toml",
                &mod_toml(
                    "hud",
                    "[client_script]\nentry = \"client/main.lua\"\ntiers = [\"ui\", \"net\"]\n",
                ),
            ),
            ("mods/hud/client/main.lua", b"realm.log('x')"),
            ("mods/hud/art/pic.words", b"a b"),
            ("mods/plain/mod.toml", &mod_toml("plain", "")),
            ("mods/plain/art/pic.words", b"a b"),
        ]),
        "server.cs",
    );
    let mods = Mods::of(&vm).expect("the mods");
    let offers = mods.client_script_offers();
    assert_eq!(offers.len(), 1, "{offers:?}");
    let offer = &offers[0];
    assert_eq!(
        (offer.mod_id.as_str(), offer.entry.as_str()),
        ("hud", "client/main.lua")
    );
    assert_eq!(offer.tiers, ["ui", "net"]);
    assert_eq!(
        offer.hash,
        mods.content_of("hud").expect("its content").hash,
        "the hash the clients verify, which covers the entry"
    );
    let required = mods.required();
    assert!(
        required
            .iter()
            .any(|m| m.id == "hud" && m.hash == offer.hash)
    );
}
