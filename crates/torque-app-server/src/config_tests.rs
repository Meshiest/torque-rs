//! `game.toml`: parsing, the limits on untrusted files, applying it to an engine, and the entry scripts a server reads from it.

use std::net::{Ipv4Addr, SocketAddr};

use torque_engine::{BuildError, EngineBuilder, ProtocolKind, Warning};
use torque_ts_vm::host::MemFiles;

use crate::config::{
    ConfigError, DEFAULT_CLIENT_ENTRY, DEFAULT_SERVER_ENTRY, EngineConfig, GameConfig, MAX_BYTES,
    MAX_IDS, MAX_NAME_LEN, PATH, client_entry, server_entry,
};
use crate::files::GameFiles;
use crate::plugins::{
    EngineProtocolPlugin, MinimalPlugins, TerrainPlugin, TgeProtocolPlugin, WaterPlugin,
    protocol_from_env, protocol_plugin_id,
};
use crate::server::{GameServer, Listen, ServerStart};

const SAMPLE: &str = r#"
[game]
name = "Sample"
server = "boot/server.cs"
client = "boot/client.cs"

[plugins]
disable = ["torque.water", "torque.terrain"]
enable = ["torque.audio"]

[runner]
id = "torque.dedicated-runner"
"#;

fn parse(text: &str) -> GameConfig {
    GameConfig::parse(text).unwrap_or_else(|e| panic!("{e}"))
}

fn error(text: &str) -> ConfigError {
    GameConfig::parse(text).expect_err("an error")
}

fn line_of(e: &ConfigError) -> Option<usize> {
    e.at.map(|(line, _)| line)
}

#[test]
fn a_full_config_reads_every_key() {
    let config = parse(SAMPLE);
    assert_eq!(config.name.as_deref(), Some("Sample"));
    assert_eq!(config.server, "boot/server.cs");
    assert_eq!(config.client, "boot/client.cs");
    assert_eq!(config.disable, ["torque.water", "torque.terrain"]);
    assert_eq!(config.enable, ["torque.audio"]);
    assert_eq!(config.runner.as_deref(), Some("torque.dedicated-runner"));
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
}

#[test]
fn an_empty_or_missing_file_is_todays_behavior() {
    let config = parse("");
    assert_eq!(config, GameConfig::default());
    assert_eq!(config.server, DEFAULT_SERVER_ENTRY);
    assert_eq!(config.client, DEFAULT_CLIENT_ENTRY);
    assert_eq!(GameConfig::load(&GameFiles::default()).unwrap(), config);
    assert_eq!(parse("# only a comment\n"), config);
}

#[test]
fn the_config_loads_from_the_games_files_whatever_they_came_from() {
    let mut files = GameFiles::default();
    files.insert("Game.TOML", SAMPLE.as_bytes());
    files.insert("server.cs", b"");
    assert_eq!(
        GameConfig::load(&files).unwrap().name.as_deref(),
        Some("Sample")
    );
    // The web pack carries it like any file.
    let packed = GameFiles::unpack(&files.pack()).unwrap();
    assert_eq!(GameConfig::load(&packed).unwrap(), parse(SAMPLE));
    // A byte-order mark from an editor is skipped.
    let mut bom = b"\xEF\xBB\xBF".to_vec();
    bom.extend_from_slice(SAMPLE.as_bytes());
    assert_eq!(GameConfig::from_bytes(&bom).unwrap(), parse(SAMPLE));
}

#[test]
fn keys_it_does_not_know_are_warnings() {
    let config = parse(
        "title = 1\n[game]\nname = \"G\"\nfullscreen = true\n[plugins]\nreload = []\n[runner]\nid = \"torque.window-runner\"\nfps = 60\n[extras]\nx = 1\n",
    );
    assert_eq!(config.name.as_deref(), Some("G"));
    let mut warnings = config.warnings.clone();
    warnings.sort();
    assert_eq!(
        warnings,
        [
            "the key `extras` is not known; skipped",
            "the key `game.fullscreen` is not known; skipped",
            "the key `plugins.reload` is not known; skipped",
            "the key `runner.fps` is not known; skipped",
            "the key `title` is not known; skipped",
        ]
    );
}

#[test]
fn an_id_in_both_lists_is_a_warning_and_a_repeat_is_dropped() {
    let config = parse("[plugins]\ndisable = [\"a.b\", \"a.b\"]\nenable = [\"a.b\"]\n");
    assert_eq!(config.disable, ["a.b"]);
    assert_eq!(config.warnings.len(), 1, "{:?}", config.warnings);
}

#[test]
fn malformed_toml_is_an_error_with_a_line_and_column() {
    let e = error("[game]\nname = \"a\"\nserver = = 3\n");
    assert_eq!(line_of(&e), Some(3), "{e}");
    assert!(e.to_string().starts_with("game.toml:3:"), "{e}");
    let e = error("[game\nname = 1\n");
    assert_eq!(line_of(&e), Some(1), "{e}");
    let e = error("[game]\nname = \"unfinished\n");
    assert_eq!(line_of(&e), Some(2), "{e}");
    // A column counts characters, not bytes.
    let e = error("# \u{e9}\u{e9}\nx = ?\n");
    assert_eq!(e.at, Some((2, 5)), "{e}");
}

#[test]
fn a_key_of_the_wrong_type_is_an_error_naming_where() {
    let e = error("[game]\nname = \"G\"\n[plugins]\ndisable = \"torque.water\"\n");
    assert_eq!(line_of(&e), Some(4), "{e}");
    let e = error("[game]\nserver = 5\n");
    assert_eq!(line_of(&e), Some(2), "{e}");
    let e = error("[plugins]\nenable = [1, 2]\n");
    assert_eq!(line_of(&e), Some(2), "{e}");
    let e = error("game = 1\n");
    assert!(e.at.is_some(), "{e}");
}

#[test]
fn values_it_cannot_use_are_errors() {
    for (text, says) in [
        ("[game]\nserver = \"\"\n", "game.server"),
        ("[game]\nserver = \"../x.cs\"\n", "game.server"),
        ("[game]\nserver = \"/etc/x.cs\"\n", "game.server"),
        ("[game]\nclient = \"c:/x.cs\"\n", "game.client"),
        ("[game]\nclient = \"a//b.cs\"\n", "game.client"),
        ("[game]\nname = \"a\\u0007b\"\n", "game.name"),
        ("[plugins]\ndisable = [\"\"]\n", "plugins.disable"),
        ("[plugins]\nenable = [\"a b\"]\n", "plugins.enable"),
        ("[runner]\nid = \"\"\n", "runner.id"),
    ] {
        let e = error(text);
        assert!(e.message.contains(says), "{text:?}: {e}");
    }
    // A backslash is a folder separator.
    assert_eq!(parse("[game]\nserver = \"a\\\\b.cs\"\n").server, "a/b.cs");
}

#[test]
fn lists_and_names_are_bounded() {
    let ids = |n: usize| {
        (0..n)
            .map(|i| format!("\"p.{i}\""))
            .collect::<Vec<_>>()
            .join(",")
    };
    let e = error(&format!("[plugins]\ndisable = [{}]\n", ids(MAX_IDS + 1)));
    assert!(e.message.contains("plugins.disable"), "{e}");
    let ok = parse(&format!("[plugins]\ndisable = [{}]\n", ids(MAX_IDS)));
    assert_eq!(ok.disable.len(), MAX_IDS);
    let long = "x".repeat(MAX_NAME_LEN + 1);
    let e = error(&format!("[game]\nname = \"{long}\"\n"));
    assert!(e.message.contains("game.name"), "{e}");
    let e = error(&format!("[plugins]\nenable = [\"{long}\"]\n"));
    assert!(e.message.contains("plugins.enable"), "{e}");
    // A key it does not know is cut short in the warning.
    let warning = &parse(&format!("{long} = 1\n")).warnings[0];
    assert!(warning.len() < MAX_NAME_LEN + 64, "{}", warning.len());
}

#[test]
fn an_oversize_file_is_refused_before_it_is_parsed() {
    let filler = "#".repeat(MAX_BYTES - 1);
    assert!(GameConfig::parse(&format!("{filler}\n")).is_ok());
    let e = GameConfig::parse(&format!("{filler}\n\n")).expect_err("too long");
    assert!(e.message.contains("65537"), "{e}");
    assert_eq!(e.at, None);
    let mut files = GameFiles::default();
    files.insert(PATH, &vec![b'#'; MAX_BYTES + 1]);
    assert!(GameConfig::load(&files).is_err());
    // Nesting that would overflow a recursive parser is an error, not a crash.
    let deep = format!("a = {}{}\n", "[".repeat(30_000), "]".repeat(30_000));
    assert!(GameConfig::parse(&deep).is_err());
    let deep = format!("{}a = 1\n", "[".repeat(30_000));
    assert!(GameConfig::parse(&deep).is_err());
}

#[test]
fn text_that_is_not_utf8_is_an_error_with_a_line() {
    let e = GameConfig::from_bytes(b"[game]\nname = \"\xFF\"\n").expect_err("not UTF-8");
    assert_eq!(line_of(&e), Some(2), "{e}");
}

#[test]
fn applying_turns_plugins_off_and_on_by_id() {
    let config = parse("[plugins]\ndisable = [\"torque.water\"]\n");
    let engine = EngineBuilder::new()
        .add_plugins(MinimalPlugins)
        .config(&config)
        .build()
        .unwrap();
    assert!(!engine.is_plugin_added::<WaterPlugin>());
    assert!(engine.is_plugin_added::<TerrainPlugin>());
    assert!(engine.warnings().is_empty(), "{:?}", engine.warnings());

    // An id in both lists ends up on: the enables apply last.
    let config = parse("[plugins]\ndisable = [\"torque.water\"]\nenable = [\"torque.water\"]\n");
    let engine = EngineBuilder::new()
        .add_plugins(MinimalPlugins)
        .config(&config)
        .build()
        .unwrap();
    assert!(engine.is_plugin_added::<WaterPlugin>());
}

#[test]
fn an_id_no_plugin_has_is_a_warning_the_engine_keeps() {
    let config = parse(
        "[plugins]\ndisable = [\"torque.watr\"]\nenable = [\"nope.nope\"]\n[game]\nbogus = 1\n",
    );
    let engine = EngineBuilder::new()
        .add_plugins(MinimalPlugins)
        .config(&config)
        .build()
        .unwrap();
    let shown: Vec<String> = engine.warnings().iter().map(ToString::to_string).collect();
    assert_eq!(shown.len(), 3, "{shown:?}");
    assert!(
        shown
            .iter()
            .any(|w| w == "game.toml: the key `game.bogus` is not known; skipped")
    );
    assert!(engine.warnings().iter().any(|w| matches!(
        w,
        Warning::UnknownPlugin { id, action: "disable" } if id == "torque.watr"
    )));
    assert!(engine.warnings().iter().any(|w| matches!(
        w,
        Warning::UnknownPlugin { id, action: "enable" } if id == "nope.nope"
    )));
}

#[test]
fn a_runner_that_is_not_in_the_build_is_a_warning_and_the_set_one_stays() {
    let config = parse("[runner]\nid = \"torque.window-runner\"\n");
    let engine = EngineBuilder::new()
        .add_plugins(MinimalPlugins)
        .config(&config)
        .build()
        .unwrap();
    assert!(
        engine
            .warnings()
            .iter()
            .any(|w| w.to_string().contains("torque.window-runner"))
    );
    assert!(engine.is_plugin_added_id("torque.dedicated-runner"));
    let config = parse("[runner]\nid = \"torque.dedicated-runner\"\n");
    let engine = EngineBuilder::new()
        .add_plugins(MinimalPlugins)
        .config(&config)
        .build()
        .unwrap();
    assert!(engine.warnings().is_empty());
}

#[test]
fn the_net_protocol_reads_engine_and_tge_and_refuses_anything_else() {
    assert_eq!(parse("").protocol, None);
    assert_eq!(
        parse("[net]\nprotocol = \"engine\"\n").protocol,
        Some(ProtocolKind::Engine)
    );
    assert_eq!(
        parse("[net]\nprotocol = \"tge\"\n").protocol,
        Some(ProtocolKind::Tge)
    );
    // A value that is no protocol is an error at the value's own line and column.
    let e = error("[game]\nname = \"G\"\n[net]\nprotocol = \"udp\"\n");
    assert_eq!(e.at, Some((4, 12)), "{e}");
    assert!(e.message.contains("udp"), "{e}");
    for text in ["[net]\nprotocol = \"ENGINE\"\n", "[net]\nprotocol = 3\n"] {
        assert_eq!(line_of(&error(text)), Some(2), "{text:?}");
    }
    let config = parse("[net]\nspeed = 3\n");
    assert_eq!(
        config.warnings,
        ["the key `net.speed` is not known; skipped"]
    );
}

#[test]
fn the_net_protocol_picks_the_plugin_that_sets_the_slot() {
    // The protocol is what the groups give: Torque's unless `TORQUE_NET_PROTOCOL` names the engine's.
    let engine = EngineBuilder::new()
        .add_plugins(MinimalPlugins)
        .build()
        .unwrap();
    let given = protocol_from_env();
    assert_eq!(engine.protocol(), given);
    assert_eq!(
        engine.is_plugin_added::<EngineProtocolPlugin>(),
        given == ProtocolKind::Engine
    );
    assert_eq!(
        engine.is_plugin_added::<TgeProtocolPlugin>(),
        given != ProtocolKind::Engine
    );
    let config = parse("[net]\nprotocol = \"tge\"\n");
    let engine = EngineBuilder::new()
        .add_plugins(MinimalPlugins)
        .config(&config)
        .build()
        .unwrap();
    assert_eq!(engine.protocol(), ProtocolKind::Tge);
    assert!(engine.warnings().is_empty(), "{:?}", engine.warnings());
    // The engine's own protocol: the group's plugin of it is turned on, Torque's off.
    let config = parse("[net]\nprotocol = \"engine\"\n");
    let engine = EngineBuilder::new()
        .add_plugins(MinimalPlugins)
        .config(&config)
        .build()
        .unwrap();
    assert_eq!(engine.protocol(), ProtocolKind::Engine);
    assert!(engine.is_plugin_added::<EngineProtocolPlugin>());
    assert!(!engine.is_plugin_added::<TgeProtocolPlugin>());
    assert!(engine.warnings().is_empty(), "{:?}", engine.warnings());
    // A build without that plugin warns and keeps the protocol it has.
    let engine = EngineBuilder::new()
        .add_plugins(TgeProtocolPlugin)
        .config(&config)
        .build()
        .unwrap();
    assert_eq!(engine.protocol(), ProtocolKind::Tge);
    assert!(
        engine.warnings().iter().any(|w| w
            .to_string()
            .contains("the protocol `engine` is not in this build")),
        "{:?}",
        engine.warnings()
    );
}

#[test]
fn a_second_plugin_that_sets_the_protocol_fails_the_build_naming_both() {
    // Turn on the one the group left off.
    let other = if protocol_from_env() == ProtocolKind::Engine {
        ProtocolKind::Tge
    } else {
        ProtocolKind::Engine
    };
    let error = EngineBuilder::new()
        .add_plugins(MinimalPlugins)
        .enable_id(protocol_plugin_id(other).as_str())
        .build()
        .unwrap_err();
    assert!(matches!(error, BuildError::DuplicateSlot { .. }), "{error}");
    let text = error.to_string();
    assert!(
        text.contains("torque.net.tge") && text.contains("torque.net.engine"),
        "{text}"
    );
}

fn start(engine: &torque_engine::Engine, files: &MemFiles) -> Result<GameServer, String> {
    GameServer::start_engine(
        engine,
        files.clone(),
        &ServerStart {
            listen: Listen::Memory(SocketAddr::from((Ipv4Addr::LOCALHOST, 28_000))),
            globals: &[],
            start: "go",
            args: &[],
        },
    )
}

#[test]
fn a_server_runs_the_entry_script_the_config_names() {
    let files = MemFiles::new();
    files.insert("boot/server.cs", b"function go() { echo(\"booted\"); }");
    let named = EngineBuilder::new()
        .add_plugins(MinimalPlugins)
        .config(&parse(SAMPLE))
        .build()
        .unwrap();
    assert_eq!(server_entry(&named), "boot/server.cs");
    assert_eq!(client_entry(&named), "boot/client.cs");
    let mut server = start(&named, &files).expect("the named script runs");
    server.tick();
    assert!(server.drain_log().iter().any(|l| l.contains("booted")));

    // With no config, `server.cs` as always; the error names the script that is missing.
    let plain = EngineBuilder::new()
        .add_plugins(MinimalPlugins)
        .build()
        .unwrap();
    assert_eq!(server_entry(&plain), "server.cs");
    assert_eq!(client_entry(&plain), "main.cs");
    let e = start(&plain, &files).err().expect("no server.cs");
    assert_eq!(e, "server.cs is not in the game folder");
    let e = start(&named, &MemFiles::new())
        .err()
        .expect("no boot/server.cs");
    assert_eq!(e, "boot/server.cs is not in the game folder");
}

/// A deterministic stream of numbers (xorshift), so a failure repeats.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

#[test]
fn damaged_and_random_files_never_panic() {
    const BITS: &[&str] = &[
        "[",
        "]",
        "[[",
        "]]",
        "=",
        "\"",
        "'",
        "\"\"\"",
        "\\",
        "\n",
        "\r\n",
        ",",
        ".",
        "#",
        " ",
        "{",
        "}",
        "true",
        "1",
        "-",
        "0x",
        "\u{e9}",
        "\u{feff}",
        "\0",
        "game",
        "plugins",
        "disable",
        "enable",
        "runner",
        "id",
        "server",
        "client",
        "name",
        "a.b",
        "1979-05-27T07:32:00Z",
        "1e999",
        "nan",
    ];
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for round in 0..4_000 {
        let mut bytes = SAMPLE.as_bytes().to_vec();
        for _ in 0..1 + rng.below(8) {
            let at = rng.below(bytes.len() + 1);
            match rng.below(5) {
                0 if at < bytes.len() => bytes[at] = rng.below(256) as u8,
                1 if at < bytes.len() => {
                    bytes.remove(at);
                }
                2 => {
                    let bit = BITS[rng.below(BITS.len())];
                    bytes.splice(at..at, bit.bytes());
                }
                3 => bytes.truncate(at),
                _ => {
                    let end = (at + rng.below(40)).min(bytes.len());
                    let copy = bytes[at..end].to_vec();
                    let to = rng.below(bytes.len() + 1);
                    bytes.splice(to..to, copy);
                }
            }
        }
        check_total(&bytes, round);
    }
    for round in 0..2_000 {
        let n = rng.below(300);
        let bytes: Vec<u8> = (0..n).map(|_| rng.below(256) as u8).collect();
        check_total(&bytes, round);
        let text: String = (0..n % 60).map(|_| BITS[rng.below(BITS.len())]).collect();
        check_total(text.as_bytes(), round);
    }
}

/// The bytes read to a config that respects the limits, or to an error; never a panic.
fn check_total(bytes: &[u8], round: usize) {
    match GameConfig::from_bytes(bytes) {
        Ok(config) => {
            assert!(
                config.disable.len() <= MAX_IDS && config.enable.len() <= MAX_IDS,
                "round {round}"
            );
            assert!(
                !config.server.is_empty() && !config.server.starts_with('/'),
                "round {round}"
            );
            assert!(
                !config.server.split('/').any(|p| p == ".."),
                "round {round}"
            );
            let mut builder = EngineBuilder::new();
            builder.add_plugins(MinimalPlugins).config(&config);
            builder.build().expect("any config builds an engine");
        }
        Err(e) => {
            let shown = e.to_string();
            assert!(shown.starts_with("game.toml"), "round {round}: {shown}");
            if let Some((line, column)) = e.at {
                assert!(line >= 1 && column >= 1, "round {round}: {shown}");
            }
        }
    }
}

#[test]
fn the_mods_section_reads_and_turns_the_plugin_off() {
    use torque_mods::{ModsConfig, ModsPlugin};

    let config = parse("[mods]\nenabled = false\nfolder = \"addons\"\ndisable = [\"a\", \"b\"]\n");
    assert!(!config.mods.enabled);
    assert_eq!(config.mods.folder, "addons");
    assert_eq!(config.mods.disable, ["a", "b"]);
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    // The default is mods on, in `mods`.
    assert_eq!(parse("").mods, ModsConfig::default());

    let engine = EngineBuilder::new()
        .add_plugins(MinimalPlugins)
        .config(&config)
        .build()
        .unwrap();
    assert!(!engine.is_plugin_added::<ModsPlugin>());
    assert_eq!(*engine.resource::<ModsConfig>().unwrap(), config.mods);

    // A folder that climbs out of the game's files is an error, as for the entry scripts.
    assert!(GameConfig::parse("[mods]\nfolder = \"../x\"\n").is_err());
    // An unknown key is a warning.
    assert_eq!(parse("[mods]\nspeed = 1\n").warnings.len(), 1);
}
