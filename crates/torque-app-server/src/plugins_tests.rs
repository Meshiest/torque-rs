//! `MinimalPlugins`: its plugins in the order the installs ran, their stable ids, and the server it makes.

use std::net::{Ipv4Addr, SocketAddr};

use torque_engine::{Engine, EngineBuilder, PluginGroup, PluginId, PluginType};
use torque_ts_vm::host::MemFiles;

use crate::plugins::{
    GameClassesPlugin, HierarchyPlugin, MinimalPlugins, ModsPlugin, NetHostPlugin,
    ServerGamePlugin, TerrainPlugin, TgeProtocolPlugin, WaterPlugin,
};
use crate::run::DedicatedRunnerPlugin;
use crate::server::{GameServer, Listen, ServerStart};

fn engine(group: impl PluginGroup) -> Engine {
    EngineBuilder::new()
        .add_plugins(group)
        .build()
        .expect("the engine builds")
}

fn start(engine: &Engine) -> Result<GameServer, String> {
    let files = MemFiles::new();
    files.insert("server.cs", b"function startNothing() {}");
    GameServer::start_engine(
        engine,
        files,
        &ServerStart {
            listen: Listen::Memory(SocketAddr::from((Ipv4Addr::LOCALHOST, 28_000))),
            globals: &[],
            start: "startNothing",
            args: &[],
        },
    )
}

/// The transports and the content formats: listed by their own tests.
fn is_listed_apart(id: &str) -> bool {
    id.starts_with("torque.transport-") || id.starts_with("torque.format-")
}

#[test]
fn the_group_has_the_content_formats_the_engine_reads_today() {
    let engine = engine(MinimalPlugins);
    let formats: Vec<&str> = engine
        .plugins()
        .iter()
        .map(|id| id.as_str())
        .filter(|id| id.starts_with("torque.format-"))
        .collect();
    assert_eq!(
        formats,
        [
            "torque.format-gltf",
            "torque.format-dif",
            "torque.format-ter",
            "torque.format-map-layer",
        ]
    );
    // The others are in the group, off, for a game (or its `game.toml`) to turn on.
    let on = EngineBuilder::new()
        .add_plugins(MinimalPlugins)
        .enable_id("torque.format-obj")
        .enable_id("torque.format-blockbench")
        .build()
        .unwrap();
    assert!(on.is_plugin_added_id("torque.format-obj"));
    assert!(on.is_plugin_added_id("torque.format-blockbench"));
    assert!(on.warnings().is_empty(), "{:?}", on.warnings());
}

#[test]
fn the_group_lists_its_plugins_in_the_order_the_installs_ran() {
    let engine = engine(MinimalPlugins);
    let wiring: Vec<PluginId> = engine
        .plugins()
        .iter()
        .copied()
        .filter(|id| !is_listed_apart(id.as_str()))
        .collect();
    assert_eq!(
        wiring,
        [
            ModsPlugin::ID,
            GameClassesPlugin::ID,
            ServerGamePlugin::ID,
            TerrainPlugin::ID,
            WaterPlugin::ID,
            TgeProtocolPlugin::ID,
            NetHostPlugin::ID,
            HierarchyPlugin::ID,
            DedicatedRunnerPlugin::ID,
        ]
    );
    assert!(engine.warnings().is_empty(), "{:?}", engine.warnings());
}

#[test]
fn the_plugin_ids_are_the_stable_names() {
    let ids: Vec<&str> = engine(MinimalPlugins)
        .plugins()
        .iter()
        .map(|id| id.as_str())
        .filter(|id| !is_listed_apart(id))
        .collect();
    assert_eq!(
        ids,
        [
            "torque.mods",
            "torque.game-classes",
            "torque.server-game",
            "torque.terrain",
            "torque.water",
            "torque.net.tge",
            "torque.net-host",
            "torque.hierarchy",
            "torque.dedicated-runner",
        ]
    );
}

#[test]
fn a_server_starts_with_the_game_classes_and_their_natives() {
    let natives = start(&engine(MinimalPlugins))
        .expect("the server starts")
        .vm()
        .native_listing();
    assert!(
        natives.contains("GameConnection::getControlObject"),
        "the game's natives"
    );
    assert!(!natives.contains("ActionMap"), "no client natives");
}

#[test]
fn a_server_needs_its_game_and_its_host() {
    for (off, name) in [
        (
            engine(MinimalPlugins.disable::<NetHostPlugin>()),
            "NetHostPlugin",
        ),
        (
            engine(MinimalPlugins.disable::<ServerGamePlugin>()),
            "ServerGamePlugin",
        ),
    ] {
        let Err(error) = start(&off) else {
            panic!("a server starts without its {name}");
        };
        assert!(error.contains(name), "{error}");
    }
}
