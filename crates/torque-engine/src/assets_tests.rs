//! Asset loaders: a plugin's loader of a tiny text format, the cache, the caps and refusals, hot reload with fake change
//! times, datablocks whose `Filename` fields name assets, and random bytes that never panic the registry.

use std::cell::Cell;
use std::rc::Rc;

use torque_ts_vm::host::{FileHost, MemFiles, ResourceRules};
use torque_ts_vm::vm::Vm;

use super::*;
use crate::{BuildError, EngineClass, Filename, Plugin, PluginId, ServerStage};

/// A tiny text format: `key = value` lines, `#` comments; anything else is an error.
#[derive(Debug, PartialEq)]
struct Pairs(Vec<(String, String)>);

fn parse_pairs(bytes: &[u8]) -> Result<Pairs, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "not UTF-8 text".to_string())?;
    let mut pairs = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("line {} has no '='", n + 1))?;
        pairs.push((key.trim().to_owned(), value.trim().to_owned()));
    }
    Ok(Pairs(pairs))
}

/// The cap of the test loader.
const CAP: u64 = 64;

/// Counts the loader's runs.
#[derive(Default)]
struct Runs(Rc<Cell<u32>>);

/// A plugin adding the `Pairs` loader for `.pairs` and `.kv`, from any source.
#[derive(PluginId)]
#[plugin(id = "test.pairs")]
struct PairsPlugin {
    runs: Rc<Cell<u32>>,
}

impl Plugin for PairsPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        let runs = Rc::clone(&self.runs);
        engine.assets().loader(
            &["pairs", ".KV"],
            Spec::untrusted(CAP),
            move |bytes, _cx| {
                runs.set(runs.get() + 1);
                parse_pairs(bytes)
            },
        );
    }
}

/// A word list from trusted files only: its loader is not hardened.
struct Words(Vec<String>);

#[derive(PluginId)]
#[plugin(id = "test.words")]
struct WordsPlugin;

impl Plugin for WordsPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine
            .assets()
            .loader(&["words"], Spec::trusted(1024), |bytes, cx| {
                assert_eq!(
                    cx.origin(),
                    Origin::Trusted,
                    "never given an untrusted file"
                );
                Ok(Words(
                    String::from_utf8_lossy(bytes)
                        .split_whitespace()
                        .map(str::to_owned)
                        .collect(),
                ))
            });
    }
}

fn engine(runs: &Runs) -> Engine {
    EngineBuilder::new()
        .add_plugins((
            PairsPlugin {
                runs: Rc::clone(&runs.0),
            },
            WordsPlugin,
        ))
        .build()
        .expect("the engine builds")
}

fn files() -> MemFiles {
    let files = MemFiles::new();
    files.insert("data/door.pairs", b"# a door\nwidth = 2\nheight = 3\n");
    files.insert("data/bad.pairs", b"width 2\n");
    files.insert("data/big.pairs", &[b'#'; CAP as usize + 1]);
    files.insert("data/list.words", b"red green blue");
    files.insert("mods/evil/list.words", b"x y");
    files
}

fn pairs(text: &[(&str, &str)]) -> Pairs {
    Pairs(
        text.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
    )
}

#[test]
fn a_plugins_loader_loads_its_format_and_the_cache_hits_by_normalized_path() {
    let runs = Runs::default();
    let assets = Assets::new(&engine(&runs));
    let files = files();
    let door = assets
        .load::<Pairs>(&files, "data/door.pairs")
        .expect("loads");
    assert_eq!(*door, pairs(&[("width", "2"), ("height", "3")]));
    for spelling in [
        "data/door.pairs",
        "DATA/Door.PAIRS",
        "./data//door.pairs",
        "data\\x\\..\\door.pairs",
        "/data/door.pairs",
    ] {
        let again = assets.load::<Pairs>(&files, spelling).expect("cached");
        assert!(Rc::ptr_eq(&door, &again), "{spelling} is the cached asset");
    }
    assert_eq!((runs.0.get(), assets.reads(), assets.len()), (1, 1, 1));
    assert!(assets.get::<Pairs>("Data/Door.pairs").is_some());
    assert!(
        assets.get::<Words>("data/door.pairs").is_none(),
        "cached by type too"
    );
    assets.forget("data/DOOR.pairs");
    assert!(assets.is_empty());
    assets
        .load::<Pairs>(&files, "data/door.pairs")
        .expect("loads again");
    assert_eq!(runs.0.get(), 2);
}

#[test]
fn the_loaders_are_listed_with_their_extensions_in_lower_case() {
    let engine = engine(&Runs::default());
    let listed: Vec<_> = engine
        .asset_loaders()
        .map(|l| {
            (
                l.plugin.as_str(),
                l.extensions.join(","),
                l.spec.untrusted_ok,
            )
        })
        .collect();
    assert_eq!(
        listed,
        [
            ("test.pairs", "pairs,kv".to_string(), true),
            ("test.words", "words".to_string(), false)
        ]
    );
}

#[test]
fn a_file_over_the_cap_is_a_clean_error_and_is_not_read() {
    let runs = Runs::default();
    let assets = Assets::new(&engine(&runs));
    let error = assets
        .load::<Pairs>(&files(), "data/big.pairs")
        .unwrap_err();
    assert_eq!(
        error,
        AssetError::OverCap {
            path: "data/big.pairs".into(),
            len: CAP + 1,
            max: CAP
        }
    );
    assert_eq!(
        error.to_string(),
        "data/big.pairs is 65 bytes, over its loader's 64"
    );
    assert_eq!((runs.0.get(), assets.reads(), assets.len()), (0, 0, 0));
}

#[test]
fn a_loader_of_trusted_files_is_never_given_an_untrusted_one() {
    let assets = Assets::new(&engine(&Runs::default()));
    assets.set_origins(|path| {
        if path.to_ascii_lowercase().starts_with("mods/") {
            Origin::Untrusted
        } else {
            Origin::Trusted
        }
    });
    let files = files();
    let error = assets.load::<Words>(&files, "mods/evil/list.words").err();
    assert_eq!(
        error,
        Some(AssetError::Untrusted {
            path: "mods/evil/list.words".into()
        })
    );
    assert_eq!(assets.reads(), 0, "refused before it is read");
    assert_eq!(
        assets
            .load::<Words>(&files, "data/list.words")
            .expect("trusted")
            .0,
        ["red", "green", "blue"]
    );
    files.insert("mods/evil/ok.pairs", b"a = b");
    assert!(
        assets.load::<Pairs>(&files, "mods/evil/ok.pairs").is_ok(),
        "a hardened loader takes it"
    );
}

#[test]
fn missing_refused_unknown_and_unreadable_files_are_clean_errors() {
    let assets = Assets::new(&engine(&Runs::default()));
    let files = files();
    let cases: [(&str, AssetError, &str); 5] = [
        (
            "data/none.pairs",
            AssetError::Missing {
                path: "data/none.pairs".into(),
            },
            "data/none.pairs is missing",
        ),
        (
            "data/bad.pairs",
            AssetError::Parse {
                path: "data/bad.pairs".into(),
                message: "line 1 has no '='".into(),
            },
            "data/bad.pairs did not load: line 1 has no '='",
        ),
        (
            "data/door.txt",
            AssetError::NoLoader {
                path: "data/door.txt".into(),
                ty: std::any::type_name::<Pairs>(),
            },
            "data/door.txt has no loader for Pairs",
        ),
        (
            "../outside.pairs",
            AssetError::BadPath {
                path: "../outside.pairs".into(),
            },
            "\"../outside.pairs\" is not a file's path",
        ),
        (
            "data/",
            AssetError::BadPath {
                path: "data/".into(),
            },
            "\"data/\" is not a file's path",
        ),
    ];
    for (path, want, text) in cases {
        let error = assets.load::<Pairs>(&files, path).unwrap_err();
        assert_eq!(error, want, "{path}");
        assert_eq!(error.to_string(), text);
    }
    assert!(assets.is_empty(), "nothing failed is cached");

    let refusing = MemFiles::new().with_rules(ResourceRules {
        refusal: |_| Some(torque_ts_vm::host::Refusal::NotDefaultDirectory),
        ..ResourceRules::default()
    });
    refusing.insert("data/door.pairs", b"a = b");
    assert_eq!(
        assets
            .load::<Pairs>(&refusing, "data/door.pairs")
            .unwrap_err(),
        AssetError::Refused {
            path: "data/door.pairs".into()
        }
    );
}

#[test]
fn an_errors_text_escapes_control_characters_so_it_stays_one_line() {
    let assets = Assets::new(&engine(&Runs::default()));
    let files = MemFiles::new();
    let error = assets
        .load::<Pairs>(&files, "data/a\n\u{1b}[31mFAKE.pairs")
        .unwrap_err();
    assert_eq!(
        error.path(),
        "data/a\n\u{1b}[31mFAKE.pairs",
        "the error keeps the path itself"
    );
    assert_eq!(
        error.to_string(),
        r"data/a\n\u{1b}[31mFAKE.pairs is missing"
    );
    files.insert("data/b.pairs", b"line\r\nmore");
    let error = assets.load::<Pairs>(&files, "data/b.pairs").unwrap_err();
    assert_eq!(
        error.to_string(),
        "data/b.pairs did not load: line 1 has no '='"
    );
    let parse = AssetError::Parse {
        path: "x\t.pairs".into(),
        message: "bad\nline".into(),
    };
    assert_eq!(parse.to_string(), r"x\t.pairs did not load: bad\nline");
    assert!(!parse.to_string().chars().any(char::is_control));
}

#[test]
fn a_panicking_loader_is_a_parse_error() {
    #[derive(PluginId)]
    #[plugin(id = "test.panics")]
    struct Panics;
    impl Plugin for Panics {
        fn build(&self, engine: &mut EngineBuilder) {
            engine
                .assets()
                .loader::<()>(&["boom"], Spec::untrusted(16), |_, _| {
                    panic!("a bug in the loader")
                });
        }
    }
    let engine = EngineBuilder::new()
        .add_plugins(Panics)
        .build()
        .expect("builds");
    let files = MemFiles::new();
    files.insert("a.boom", b"x");
    let error = Assets::new(&engine)
        .load::<()>(&files, "a.boom")
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "a.boom did not load: its loader panicked"
    );
}

#[test]
fn bad_extensions_and_a_second_loader_of_one_type_fail_the_build() {
    #[derive(PluginId)]
    #[plugin(id = "test.again")]
    struct Again;
    impl Plugin for Again {
        fn build(&self, engine: &mut EngineBuilder) {
            engine
                .assets()
                .loader(&["PAIRS"], Spec::trusted(1), |b, _| parse_pairs(b));
        }
    }
    let error = EngineBuilder::new()
        .add_plugins((
            PairsPlugin {
                runs: Rc::default(),
            },
            Again,
        ))
        .build()
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "the plugin `test.again`: `test.pairs` already added a loader of Pairs assets for .pairs"
    );

    #[derive(PluginId)]
    #[plugin(id = "test.dotted")]
    struct Dotted;
    impl Plugin for Dotted {
        fn build(&self, engine: &mut EngineBuilder) {
            engine
                .assets()
                .loader(&["tar.gz"], Spec::trusted(1), |b, _| parse_pairs(b));
        }
    }
    let error = EngineBuilder::new()
        .add_plugins(Dotted)
        .build()
        .unwrap_err();
    assert!(matches!(error, BuildError::Rejected { .. }), "{error}");
}

#[test]
fn hot_reload_follows_the_change_times_and_keeps_the_old_asset_on_a_failure() {
    let runs = Runs::default();
    let assets = Assets::new(&engine(&runs));
    let files = MemFiles::new();
    files.insert_at("data/door.pairs", b"width = 2", 100);
    files.insert_at("data/list.words", b"a b", 100);
    let door = assets
        .load::<Pairs>(&files, "Data/Door.pairs")
        .expect("loads");
    assets
        .load::<Words>(&files, "data/list.words")
        .expect("loads");

    // Nothing changed: no reload.
    assert!(assets.poll(&files, 1_000).is_empty());
    assert!(assets.changed().is_empty());

    // A change inside the interval waits for the next look.
    files.insert_at("data/door.pairs", b"width = 5", 200);
    assert!(
        assets
            .poll(&files, 1_000 + HOT_RELOAD_INTERVAL_MS - 1)
            .is_empty()
    );
    assert!(assets.changed().is_empty());
    assert!(
        assets
            .poll(&files, 1_000 + HOT_RELOAD_INTERVAL_MS)
            .is_empty()
    );
    assert_eq!(assets.changed(), ["Data/Door.pairs"]);
    let new_door = assets.get::<Pairs>("data/door.pairs").expect("cached");
    assert_eq!(*new_door, pairs(&[("width", "5")]));
    assert_eq!(
        *door,
        pairs(&[("width", "2")]),
        "a holder of the old asset keeps it"
    );
    assert_eq!(runs.0.get(), 2);

    // The next frame lists nothing.
    assert!(
        assets
            .poll(&files, 1_000 + HOT_RELOAD_INTERVAL_MS + 1)
            .is_empty()
    );
    assert!(assets.changed().is_empty());

    // New contents that do not load keep the old asset, once, until the file changes again; the counter may wrap.
    files.insert_at("data/door.pairs", b"nonsense", 300);
    let failed = assets.poll(&files, u32::MAX);
    assert_eq!(failed.len(), 1);
    assert!(matches!(failed[0], AssetError::Parse { .. }));
    assert!(assets.changed().is_empty());
    assert_eq!(
        *assets.get::<Pairs>("data/door.pairs").expect("kept"),
        pairs(&[("width", "5")])
    );
    assert!(
        assets.poll(&files, HOT_RELOAD_INTERVAL_MS).is_empty(),
        "the failure is not retried"
    );
    files.insert_at("data/door.pairs", b"width = 7", 400);
    assert!(assets.poll(&files, 2 * HOT_RELOAD_INTERVAL_MS).is_empty());
    assert_eq!(assets.changed(), ["Data/Door.pairs"]);

    // A file that went is a failure, and the asset stays.
    assert!(files.remove(b"data/list.words"));
    let failed = assets.poll(&files, 3 * HOT_RELOAD_INTERVAL_MS);
    assert_eq!(
        failed,
        [AssetError::Missing {
            path: "data/list.words".into()
        }]
    );
    assert!(assets.get::<Words>("data/list.words").is_some());
}

#[test]
fn hot_reload_is_off_in_release_unless_the_dev_pref_or_the_host_turns_it_on() {
    let engine = engine(&Runs::default());
    let mut vm = Vm::new();
    let files = MemFiles::new();
    files.insert_at("data/door.pairs", b"width = 2", 100);
    vm.set_file_host(files.clone());
    let assets = engine.assets(&mut vm);
    assert!(Rc::ptr_eq(&assets, &Assets::of(&vm).expect("attached")));
    assert!(
        Rc::ptr_eq(&assets, &engine.assets(&mut vm)),
        "one cache per script engine"
    );
    assets
        .load::<Pairs>(vm.file_host(), "data/door.pairs")
        .expect("loads");
    assert_eq!(assets.hot_reload_on(&vm), cfg!(debug_assertions));

    assets.set_hot_reload(Some(false));
    files.insert_at("data/door.pairs", b"width = 3", 200);
    assert!(assets.poll_in(&vm).is_empty());
    assert!(assets.changed().is_empty(), "turned off");

    assets.set_hot_reload(None);
    vm.set_global(crate::script::DEV_PREF, "1");
    assert!(assets.hot_reload_on(&vm));
    assert!(assets.poll_in(&vm).is_empty());
    assert_eq!(assets.changed(), ["data/door.pairs"]);
}

#[test]
fn the_resource_layer_serves_the_loaders() {
    use torque_resource::files::{Limits, MountPolicy, ResourceFiles};
    let engine = engine(&Runs::default());
    let files = ResourceFiles::open_with_parts(
        std::env::temp_dir().join("torque-engine-assets-no-such-folder"),
        MountPolicy {
            rules: ResourceRules {
                scanned: &["data"],
                ..ResourceRules::default()
            },
            ..MountPolicy::default()
        },
        Limits::default(),
        Vec::new(),
        vec![("data/door.pairs".into(), b"width = 4".to_vec())],
    );
    let mut vm = Vm::new();
    vm.set_file_host(files);
    let assets = engine.assets(&mut vm);
    let door = assets
        .load::<Pairs>(vm.file_host(), "DATA/door.pairs")
        .expect("loads");
    assert_eq!(*door, pairs(&[("width", "4")]));
    // A file a script wrote is a file too.
    assert!(vm.file_host().write(b"data/new.pairs", b"a = 1", false));
    assert!(
        assets
            .load::<Pairs>(vm.file_host(), "data/new.pairs")
            .is_ok()
    );
}

/// A datablock whose `list` names a `.words` asset and whose `door` a `.pairs` one.
#[derive(EngineClass)]
#[class(name = "SignData", parent = "SimDataBlock", preload = SignData::check)]
struct SignData {
    #[field(asset = Words)]
    list: Filename,
    #[field(asset = Pairs)]
    #[allow(dead_code, reason = "the preload loads it; nothing reads it back")]
    door: Filename,
}

impl SignData {
    fn check(&self, vm: &mut Vm, _id: crate::class::ObjId) -> Result<(), String> {
        let assets = Assets::of(vm).ok_or("no cache")?;
        let words = assets
            .get::<Words>(self.list.as_str())
            .ok_or("the list is not loaded")?;
        if words.0.is_empty() {
            return Err("the list is empty".into());
        }
        Ok(())
    }
}

#[derive(PluginId)]
#[plugin(id = "test.signs")]
struct SignPlugin;

impl Plugin for SignPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.script().class::<SignData>();
    }
}

fn logged(vm: &Vm, text: &str) -> bool {
    vm.log_texts().iter().any(|line| line.contains(text))
}

#[test]
fn a_datablocks_asset_fields_preload_their_files_and_fail_cleanly() {
    let engine = EngineBuilder::new()
        .add_plugins((
            PairsPlugin {
                runs: Rc::default(),
            },
            WordsPlugin,
            SignPlugin,
        ))
        .build()
        .expect("builds");
    let mut vm = Vm::new();
    let files = files();
    files.insert("data/empty.words", b"");
    vm.set_file_host(files);
    for &stage in ServerStage::ALL {
        engine.replay(stage, &mut vm).expect("replays");
    }
    vm.eval(
        b"datablock SignData(Good) { list = \"data/list.words\"; door = \"data/door.pairs\"; };",
    )
    .expect("runs");
    assert!(vm.objects().find(b"Good").is_some());
    let assets = Assets::of(&vm).expect("the preload attached the cache");
    assert!(assets.get::<Words>("data/list.words").is_some());
    assert!(assets.get::<Pairs>("data/door.pairs").is_some());

    vm.eval(b"datablock SignData(NoDoor) { list = \"data/list.words\"; };")
        .expect("runs");
    assert!(
        vm.objects().find(b"NoDoor").is_some(),
        "an empty name names nothing"
    );

    let failures = [
        (
            "Missing",
            "list = \"data/none.words\";",
            "preload failed for Missing: list: data/none.words is missing.",
        ),
        (
            "Big",
            "list = \"data/list.words\"; door = \"data/big.pairs\";",
            "preload failed for Big: door: data/big.pairs is 65 bytes, over its loader's 64.",
        ),
        (
            "Broken",
            "list = \"data/list.words\"; door = \"data/bad.pairs\";",
            "preload failed for Broken: door: data/bad.pairs did not load: line 1 has no '='.",
        ),
        (
            "Wrong",
            "list = \"data/door.pairs\";",
            "preload failed for Wrong: list: data/door.pairs has no loader for Words.",
        ),
        (
            "Empty",
            "list = \"data/empty.words\";",
            "preload failed for Empty: the list is empty.",
        ),
    ];
    for (name, fields, line) in failures {
        vm.eval(format!("datablock SignData({name}) {{ {fields} }};").as_bytes())
            .expect("runs");
        assert!(
            vm.objects().find(name.as_bytes()).is_none(),
            "{name} is deleted"
        );
        assert!(logged(&vm, line), "{line}");
    }

    assets.set_origins(|_| Origin::Untrusted);
    assets.clear();
    vm.eval(b"datablock SignData(Sent) { list = \"data/list.words\"; };")
        .expect("runs");
    assert!(vm.objects().find(b"Sent").is_none());
    assert!(logged(
        &vm,
        "preload failed for Sent: list: data/list.words is from an untrusted source, which its loader does not take."
    ));
}

/// A xorshift generator, so the cases are the same every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[test]
fn random_bytes_under_the_cap_never_panic_the_registry() {
    let runs = Runs::default();
    let assets = Assets::new(&engine(&runs));
    let files = MemFiles::new();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let cases = 20_000;
    let mut loaded = 0;
    for case in 0..cases {
        let len = (rng.next() % (CAP + 8)) as usize;
        let alphabet = b"ab =#\n\r\t\xff\xc3";
        let bytes: Vec<u8> = (0..len)
            .map(|_| {
                let r = rng.next();
                if r & 1 == 0 {
                    alphabet[(r >> 1) as usize % alphabet.len()]
                } else {
                    (r >> 8) as u8
                }
            })
            .collect();
        // Random paths too, inside and outside the game folder.
        let path = match rng.next() % 4 {
            0 => format!("f/{case}.pairs"),
            1 => format!("f/../{case}.KV"),
            2 => format!("..\\{case}.pairs"),
            _ => format!("f/{case}"),
        };
        files.insert(&format!("f/{case}.pairs"), &bytes);
        files.insert(&format!("{case}.kv"), &bytes);
        match assets.load::<Pairs>(&files, &path) {
            Ok(_) => loaded += 1,
            Err(
                AssetError::Parse { .. }
                | AssetError::OverCap { .. }
                | AssetError::BadPath { .. }
                | AssetError::NoLoader { .. },
            ) => {}
            Err(other) => panic!("{path}: unexpected {other}"),
        }
        if case % 64 == 0 {
            let _ = assets.poll(&files, case as u32 * HOT_RELOAD_INTERVAL_MS);
        }
    }
    assert!(loaded > 0, "some cases load");
    assert!(runs.0.get() > 0);
}

#[test]
fn normalize_keeps_the_case_and_drops_what_means_nothing() {
    assert_eq!(
        normalize("./Art//Rope\\x.TXT").as_deref(),
        Ok("Art/Rope/x.TXT")
    );
    assert_eq!(normalize("a/b/../c.txt").as_deref(), Ok("a/c.txt"));
    assert!(normalize("").is_err());
    assert!(normalize("a/../..").is_err());
    assert!(normalize("a\0b.txt").is_err());
    assert_eq!(extension("a/b.c/d"), None);
    assert_eq!(extension("a/.hidden"), None);
    assert_eq!(extension("a/x.GLB").as_deref(), Some("glb"));
}
