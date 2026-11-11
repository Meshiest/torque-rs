//! The rules are replaceable: a second layout (JSON manifests under another file name, alphabetical order, two custom stages,
//! an enable list kept in a script file) loads the same fixture differently from the default, and each slot is replaced on its
//! own. The limits and the path rules still hold for a custom layout.

mod common;

use common::{files, server};
use serde_json::Value;
use torque_engine::{Engine, EngineBuilder, ServerStage};
use torque_mods::{
    Action, Candidate, CandidateKind, EngineMods, ModEnableList, ModInfo, ModLayout, ModLimits,
    ModNames, ModOrder, ModPrereqs, ModRoot, ModStages, ModsPlugin, NameInfo, OrderError,
    PrereqPolicy, Problem, Requirement, Version,
};
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::testing::{console, eval};
use torque_ts_vm::vm::Vm;

/// The same mods in two manifest formats: `mod.toml` for the default layout, `mod.json` for the custom one.
fn fixture() -> MemFiles {
    files(&[
        ("server.cs", b"$order = \"\";"),
        ("mods/alpha/mod.toml", b"id = \"alpha\"\nversion = \"1.0\"\nrequires = [\"beta\"]\n[scripts]\nserver = \"a.cs\"\n"),
        ("mods/alpha/mod.json", br#"{"id":"alpha","version":"1","priority":2,"scripts":{"early":["e.cs"],"late":["l.cs"]}}"#),
        ("mods/alpha/a.cs", b"$order = $order @ \"alpha-toml \";"),
        ("mods/alpha/e.cs", b"$order = $order @ \"alpha-early \";"),
        ("mods/alpha/l.cs", b"$order = $order @ \"alpha-late \";"),
        ("mods/beta/mod.toml", b"id = \"beta\"\nversion = \"1.0\"\n[scripts]\nserver = \"b.cs\"\n"),
        ("mods/beta/mod.json", br#"{"id":"beta","version":"1","priority":1,"scripts":{"early":["e.cs"],"late":["l.cs"]}}"#),
        ("mods/beta/b.cs", b"$order = $order @ \"beta-toml \";"),
        ("mods/beta/e.cs", b"$order = $order @ \"beta-early \";"),
        ("mods/beta/l.cs", b"$order = $order @ \"beta-late \";"),
        ("mods/gamma/mod.json", br#"{"id":"gamma","version":"3","scripts":{"early":["e.cs"]}}"#),
        ("mods/gamma/e.cs", b"$order = $order @ \"gamma-early \";"),
    ])
}

/// A layout of `mod.json` files in the folders of `mods`.
struct JsonLayout;

impl ModLayout for JsonLayout {
    fn roots(&self, _engine: &Engine) -> Vec<ModRoot> {
        vec![ModRoot::files("mods").folders_only()]
    }

    fn recognize(
        &self,
        candidate: &Candidate<'_>,
        _stages: &ModStages,
    ) -> Result<Option<ModInfo>, String> {
        let Some(bytes) = candidate.package.read("mod.json") else {
            return Ok(None);
        };
        let json: Value = serde_json::from_slice(&bytes).map_err(|e| format!("mod.json: {e}"))?;
        let text = |key: &str| json.get(key).and_then(Value::as_str);
        let mut info = ModInfo::new(text("id").ok_or("mod.json has no id")?);
        info.version = text("version").unwrap_or("0").parse::<Version>()?;
        info.hidden.push("mod.json".into());
        if let Some(priority) = json.get("priority").and_then(Value::as_i64) {
            info.extra.insert("priority".into(), priority.to_string());
        }
        if let Some(stages) = json.get("scripts").and_then(Value::as_object) {
            for (stage, paths) in stages {
                let paths = paths
                    .as_array()
                    .map(|list| {
                        list.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                info.scripts.push((stage.clone(), paths));
            }
        }
        Ok(Some(info))
    }
}

struct Alphabetical;

impl ModOrder for Alphabetical {
    fn order(&self, mods: &[&ModInfo]) -> Result<Vec<usize>, OrderError> {
        let mut at: Vec<usize> = (0..mods.len()).collect();
        at.sort_by(|&a, &b| mods[a].id.cmp(&mods[b].id));
        Ok(at)
    }
}

/// By the `priority` field a layout kept, lowest first.
struct ByPriority;

impl ModOrder for ByPriority {
    fn order(&self, mods: &[&ModInfo]) -> Result<Vec<usize>, OrderError> {
        let key = |at: usize| {
            mods[at]
                .extra
                .get("priority")
                .and_then(|p| p.parse::<i64>().ok())
                .unwrap_or(0)
        };
        let mut at: Vec<usize> = (0..mods.len()).collect();
        at.sort_by_key(|&i| key(i));
        Ok(at)
    }
}

/// The enable list as a script file of `$Mod__<id> = 0;` lines.
struct ScriptFileList;

const LIST: &str = "config/mods.cs";

impl ModEnableList for ScriptFileList {
    fn disabled(&self, vm: &mut Vm, _engine: &Engine, _mods: &[&ModInfo]) -> Vec<String> {
        let torque_ts_vm::host::FileRead::Found(bytes) = vm.file_host().read(LIST.as_bytes())
        else {
            return Vec::new();
        };
        String::from_utf8_lossy(&bytes)
            .lines()
            .filter_map(|line| {
                let (name, value) = line.trim().strip_prefix("$Mod__")?.split_once('=')?;
                (value.trim().trim_end_matches(';').trim() == "0").then(|| name.trim().to_string())
            })
            .collect()
    }

    fn save(&self, vm: &mut Vm, _engine: &Engine, states: &[(String, bool)]) {
        let text: String = states
            .iter()
            .map(|(id, on)| format!("$Mod__{id} = {};\n", u8::from(*on)))
            .collect();
        assert!(
            vm.file_host()
                .write(LIST.as_bytes(), text.as_bytes(), false)
        );
    }
}

fn builder() -> EngineBuilder {
    EngineBuilder::new()
}

fn list(vm: &mut Vm) -> String {
    eval(vm, "return getModList();")
}

#[test]
fn a_second_layout_loads_the_same_fixture_differently() {
    // The default: mod.toml, dependency order, the `server` stage by itself, gamma (no mod.toml) left out.
    let default = builder()
        .add_plugins(ModsPlugin::new())
        .build()
        .expect("builds");
    let mut vm = server(&default, fixture(), "server.cs");
    assert_eq!(list(&mut vm), "beta alpha");
    assert_eq!(eval(&mut vm, "return $order;"), "beta-toml alpha-toml ");
    assert!(eval(&mut vm, "return getModField(gamma, reason);").contains("no mod.toml"));

    // The custom rules: JSON manifests, alphabetical, two stages that run when the game says.
    let mut custom = builder();
    custom
        .mods()
        .layout(JsonLayout)
        .order(Alphabetical)
        .stages(ModStages::new(["early", "late"]))
        .enable_list(ScriptFileList);
    let engine = custom
        .add_plugins(ModsPlugin::new())
        .build()
        .expect("builds");
    let mut vm = server(&engine, fixture(), "server.cs");
    assert_eq!(
        list(&mut vm),
        "alpha beta gamma",
        "alphabetical, whatever alpha requires"
    );
    assert_eq!(
        eval(&mut vm, "return $order;"),
        "",
        "no stage runs by itself"
    );
    assert_eq!(eval(&mut vm, "return getModStages();"), "early late");
    assert_eq!(
        eval(&mut vm, "return getModField(alpha, priority);"),
        "2",
        "extra fields are kept"
    );

    // The first stage from Rust, the second from script.
    let mods = torque_mods::Mods::of(&vm).expect("mods");
    assert_eq!(mods.load_stage(&mut vm, "early").expect("stage"), 3);
    assert_eq!(
        eval(&mut vm, "return $order;"),
        "alpha-early beta-early gamma-early "
    );
    assert_eq!(eval(&mut vm, "$order = \"\"; return loadMods(late);"), "2");
    assert_eq!(eval(&mut vm, "return $order;"), "alpha-late beta-late ");
    assert_eq!(
        eval(&mut vm, "return loadMods(late);"),
        "0",
        "a stage runs once"
    );

    // The enable list is a script file now.
    assert_eq!(eval(&mut vm, "return disableMod(beta);"), "1");
    let saved = vm.file_host().read(LIST.as_bytes());
    let torque_ts_vm::host::FileRead::Found(saved) = saved else {
        panic!("the list was not written");
    };
    assert_eq!(
        String::from_utf8_lossy(&saved),
        "$Mod__alpha = 1;\n$Mod__beta = 0;\n$Mod__gamma = 1;\n"
    );
    assert_eq!(
        eval(&mut vm, "return $Pref::Mods::Disabled;"),
        "",
        "the pref is not the list now"
    );
}

#[test]
fn the_script_file_list_starts_mods_disabled() {
    let mut custom = builder();
    custom.mods().layout(JsonLayout).enable_list(ScriptFileList);
    let engine = custom
        .add_plugins(ModsPlugin::new())
        .build()
        .expect("builds");
    let mem = fixture();
    mem.insert(LIST, b"$Mod__beta = 0;\n$Mod__gamma = 1;\n");
    let mut vm = server(&engine, mem, "server.cs");
    assert_eq!(
        eval(&mut vm, "return getModField(beta, status);"),
        "disabled"
    );
    assert_eq!(
        eval(&mut vm, "return getModField(gamma, status);"),
        "enabled"
    );
}

#[test]
fn a_layout_alone_changes_only_how_mods_are_read() {
    // JSON layout, everything else default: the stages are `server` and `client`, so the JSON's `early` and `late` are
    // not declared and their scripts are left out with a warning.
    let mut custom = builder();
    custom.mods().layout(JsonLayout);
    let engine = custom
        .add_plugins(ModsPlugin::new())
        .build()
        .expect("builds");
    let mut vm = server(&engine, fixture(), "server.cs");
    assert_eq!(
        list(&mut vm),
        "alpha beta gamma",
        "no requires in the JSON: the id breaks the tie"
    );
    let log = console(&mut vm);
    assert!(
        log.contains("the stage `early` is not one the game declared"),
        "{log}"
    );
}

#[test]
fn names_alone_map_folder_names_to_categories() {
    struct Prefixed;
    impl ModNames for Prefixed {
        fn map(&self, raw: &str, _kind: CandidateKind) -> Option<NameInfo> {
            let (category, name) = raw.split_once('-')?;
            Some(NameInfo {
                name: name.to_string(),
                category: Some(category.to_string()),
            })
        }
    }
    let mut custom = builder();
    custom.mods().names(Prefixed);
    let engine = custom
        .add_plugins(ModsPlugin::new())
        .build()
        .expect("builds");
    let mem = files(&[
        ("server.cs", b""),
        (
            "mods/ui-minimap/mod.toml",
            b"id = \"minimap\"\nversion = \"1\"\n",
        ),
        ("mods/plain/mod.toml", b"id = \"plain\"\nversion = \"1\"\n"),
    ]);
    let mut vm = server(&engine, mem, "server.cs");
    assert_eq!(
        list(&mut vm),
        "minimap",
        "a name the mapper refuses is not looked at"
    );
    assert_eq!(
        eval(&mut vm, "return getModField(minimap, category);"),
        "ui"
    );
}

#[test]
fn order_alone_orders_by_priority() {
    let mut custom = builder();
    custom.mods().order(ByPriority);
    let engine = custom
        .add_plugins(ModsPlugin::new())
        .build()
        .expect("builds");
    let mem = files(&[
        ("server.cs", b""),
        (
            "mods/a/mod.toml",
            b"id = \"a\"\nversion = \"1\"\n[extra]\npriority = 30\n",
        ),
        (
            "mods/b/mod.toml",
            b"id = \"b\"\nversion = \"1\"\n[extra]\npriority = 10\n",
        ),
        (
            "mods/c/mod.toml",
            b"id = \"c\"\nversion = \"1\"\n[extra]\npriority = 20\n",
        ),
    ]);
    let mut vm = server(&engine, mem, "server.cs");
    assert_eq!(list(&mut vm), "b c a");
}

#[test]
fn a_resolver_that_leaves_mods_out_is_repaired() {
    struct Lossy;
    impl ModOrder for Lossy {
        fn order(&self, mods: &[&ModInfo]) -> Result<Vec<usize>, OrderError> {
            Ok(vec![mods.len() - 1, mods.len() - 1, 99])
        }
    }
    let mut custom = builder();
    custom.mods().order(Lossy);
    let engine = custom
        .add_plugins(ModsPlugin::new())
        .build()
        .expect("builds");
    let mem = files(&[
        ("server.cs", b""),
        ("mods/a/mod.toml", b"id = \"a\"\nversion = \"1\"\n"),
        ("mods/b/mod.toml", b"id = \"b\"\nversion = \"1\"\n"),
        ("mods/c/mod.toml", b"id = \"c\"\nversion = \"1\"\n"),
    ]);
    let mut vm = server(&engine, mem, "server.cs");
    assert_eq!(list(&mut vm), "c a b");
    assert!(console(&mut vm).contains("left mods out or repeated them"));
}

fn broken() -> MemFiles {
    files(&[
        ("server.cs", b""),
        (
            "mods/needs/mod.toml",
            b"id = \"needs\"\nversion = \"1\"\nrequires = [\"absent\"]\n",
        ),
        (
            "mods/loop1/mod.toml",
            b"id = \"loop1\"\nversion = \"1\"\nrequires = [\"loop2\"]\n",
        ),
        (
            "mods/loop2/mod.toml",
            b"id = \"loop2\"\nversion = \"1\"\nrequires = [\"loop1\"]\n",
        ),
        ("mods/fine/mod.toml", b"id = \"fine\"\nversion = \"1\"\n"),
    ])
}

#[test]
fn prerequisites_alone_can_error_or_ignore() {
    // Error: the server does not start, and the error says why.
    let mut strict = builder();
    strict.mods().prereqs(PrereqPolicy::Error);
    let engine = strict
        .add_plugins(ModsPlugin::new())
        .build()
        .expect("builds");
    let mut vm = Vm::new();
    vm.set_file_host(broken());
    let error = engine
        .replay(ServerStage::Prepare, &mut vm)
        .expect_err("a missing requirement is an error")
        .to_string();
    assert!(
        error.contains("requires the mod `absent`") || error.contains("loop of requirements"),
        "{error}"
    );

    // Ignore: the mods load as if the requirements were met, the loop in the order found.
    let mut lax = builder();
    lax.mods().prereqs(PrereqPolicy::Ignore);
    let engine = lax.add_plugins(ModsPlugin::new()).build().expect("builds");
    let mut vm = server(&engine, broken(), "server.cs");
    assert_eq!(list(&mut vm), "fine needs loop1 loop2");
}

#[test]
fn a_game_decides_per_problem() {
    /// Missing mods are fine; a loop is an error.
    struct Picky;
    impl ModPrereqs for Picky {
        fn decide(&self, _id: &str, problem: &Problem) -> Action {
            match problem {
                Problem::MissingMod { .. } => Action::Ignore,
                _ => Action::Error,
            }
        }
    }
    let mut custom = builder();
    custom.mods().prereqs(Picky);
    let engine = custom
        .add_plugins(ModsPlugin::new())
        .build()
        .expect("builds");
    let mem = files(&[
        ("server.cs", b""),
        (
            "mods/needs/mod.toml",
            b"id = \"needs\"\nversion = \"1\"\nrequires = [\"absent\"]\n",
        ),
    ]);
    let mut vm = server(&engine, mem, "server.cs");
    assert_eq!(list(&mut vm), "needs");
}

#[test]
fn stages_alone_run_by_themselves_or_when_asked() {
    let mut custom = builder();
    custom.mods().stages(
        ModStages::new(["boot", "late"])
            .server(["boot"])
            .client(["late"]),
    );
    let engine = custom
        .add_plugins(ModsPlugin::new())
        .build()
        .expect("builds");
    let mem = files(&[
        ("server.cs", b"$order = \"\";"),
        (
            "mods/m/mod.toml",
            b"id = \"m\"\nversion = \"1\"\n[scripts]\nboot = \"b.cs\"\nlate = [\"l1.cs\", \"l2.cs\"]\n",
        ),
        ("mods/m/b.cs", b"$order = $order @ \"boot \";"),
        ("mods/m/l1.cs", b"$order = $order @ \"l1 \";"),
        ("mods/m/l2.cs", b"$order = $order @ \"l2 \";"),
    ]);
    let mut vm = server(&engine, mem, "server.cs");
    assert_eq!(
        eval(&mut vm, "return $order;"),
        "boot ",
        "the server runs boot by itself"
    );
    assert_eq!(eval(&mut vm, "return loadMods(late);"), "2");
    assert_eq!(eval(&mut vm, "return $order;"), "boot l1 l2 ");
}

#[test]
fn a_custom_layout_cannot_get_past_the_limits() {
    // A layout that would take everything: the limits and path rules are applied before it is asked.
    struct Greedy;
    impl ModLayout for Greedy {
        fn roots(&self, _engine: &Engine) -> Vec<ModRoot> {
            vec![ModRoot::files("mods")]
        }
        fn recognize(
            &self,
            candidate: &Candidate<'_>,
            _stages: &ModStages,
        ) -> Result<Option<ModInfo>, String> {
            let mut info = ModInfo::new(candidate.name.name.clone());
            info.requires.push(Requirement::any("x"));
            Ok(Some(info))
        }
    }
    let limits = ModLimits {
        files: 2,
        ..ModLimits::default()
    };
    let mut custom = builder();
    custom.mods().layout(Greedy).prereqs(PrereqPolicy::Ignore);
    let engine = custom
        .add_plugins(ModsPlugin::new().limits(limits))
        .build()
        .expect("builds");
    let mem = files(&[
        ("server.cs", b""),
        ("mods/three/a", b"1"),
        ("mods/three/b", b"1"),
        ("mods/three/c", b"1"),
        ("mods/two/a", b"1"),
        ("mods/two/b", b"1"),
        ("mods/bad/..\\x", b"1"),
    ]);
    let mut vm = server(&engine, mem, "server.cs");
    assert_eq!(list(&mut vm), "two");
    assert!(
        eval(&mut vm, "return getModField(three, reason);")
            .contains("3 files; a mod may have at most 2")
    );
}

#[test]
fn a_second_plugin_for_a_slot_fails_the_build_naming_both() {
    use torque_engine::{Plugin, PluginId};

    #[derive(PluginId)]
    #[plugin(id = "test.alpha-order")]
    struct First;
    impl Plugin for First {
        fn build(&self, engine: &mut EngineBuilder) {
            engine.mods().order(Alphabetical);
        }
    }
    #[derive(PluginId)]
    #[plugin(id = "test.priority-order")]
    struct Second;
    impl Plugin for Second {
        fn build(&self, engine: &mut EngineBuilder) {
            engine.mods().order(ByPriority);
        }
    }
    let error = builder()
        .add_plugins((ModsPlugin::new(), First, Second))
        .build()
        .expect_err("two setters of a slot fail the build")
        .to_string();
    assert!(
        error.contains("test.alpha-order") && error.contains("test.priority-order"),
        "{error}"
    );
}
