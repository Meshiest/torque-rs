//! Builds the engines the reference documents and merges what they hold.

use std::net::{Ipv4Addr, SocketAddr};

use torque_app::client::{ClientBuilder, ClientNet};
use torque_app::gizmos::GizmosPlugin;
use torque_app::hosting::HostingPlugin;
use torque_app::plugins::{DefaultPlugins, ZoomPlugin};
use torque_app::server::{GameServer, Listen, ServerStart};
use torque_client::predict::slot::SLOT_NATIVES;
use torque_engine::{Engine, EngineBuilder, PluginType};
use torque_plugin_gui_layout::GuiLayoutPlugin;
use torque_plugin_props::PropsPlugin;
use torque_plugin_props::predict::PropPredictionPlugin;
use torque_plugin_rope::RopePlugin;
use torque_ts_vm::host::MemFiles;

use crate::collect::{SideVm, collect};
use crate::model::{Reference, Side};

/// An opt-in plugin the reference covers, with a line on what it adds.
struct Optional {
    id: &'static str,
    what: &'static str,
    add: fn(&mut EngineBuilder),
}

/// The opt-in plugins that add script surface and that need nothing from the game to do it. (`LivePerfPlugin` names its
/// console function and pref by its arguments, so it has nothing fixed to list.)
fn optional_plugins() -> [Optional; 7] {
    [
        Optional {
            id: GizmosPlugin::ID.as_str(),
            what: "debug drawing: lines, boxes, spheres, arrows, crosses and labels in world space",
            add: |engine| {
                engine.add_plugins(GizmosPlugin::default());
            },
        },
        Optional {
            id: ZoomPlugin::ID.as_str(),
            what: "the zoom: the field of view and how fast it changes",
            add: |engine| {
                engine.add_plugins(ZoomPlugin);
            },
        },
        Optional {
            id: HostingPlugin::ID.as_str(),
            what: "joining by address or room code: the lookup natives of a join screen",
            add: |engine| {
                engine.add_plugins(HostingPlugin::default());
            },
        },
        Optional {
            id: RopePlugin::ID.as_str(),
            what: "ropes: the RopeData datablock, the console functions that hang, push, reel and take down ropes between points or objects, and a server's ropeNet functions that make ropes every client shows",
            add: |engine| {
                engine.add_plugins(RopePlugin::default());
            },
        },
        Optional {
            id: PropsPlugin::ID.as_str(),
            what: "props: the PropData datablock and the Prop class, rigid bodies a server simulates and every client sees, the prop functions that spawn, push, freeze and read them, and the placement checks a game's own spawn commands make; the Joint class and the joint functions (tethers, winches, welds, hinges, grapples) and their check",
            add: |engine| {
                engine.add_plugins(PropsPlugin);
            },
        },
        Optional {
            id: PropPredictionPlugin::ID.as_str(),
            what: "prop prediction on a client: the props its own Player holds, stands on or pushes drawn where they will be, and the client functions that show a push or a spawn at once (never sent to the server)",
            add: |engine| {
                engine.add_plugins((PropsPlugin, PropPredictionPlugin::default()));
            },
        },
        Optional {
            id: GuiLayoutPlugin::ID.as_str(),
            what: "flex and grid layout: the GuiFlexCtrl and GuiGridCtrl containers, which lay their children out by CSS's flexbox and grid rules",
            add: |engine| {
                engine.add_plugins(GuiLayoutPlugin);
            },
        },
    ]
}

fn build(extra: Option<&Optional>) -> Result<Engine, String> {
    let mut engine = EngineBuilder::new();
    engine.add_plugins(DefaultPlugins);
    if let Some(extra) = extra {
        (extra.add)(&mut engine);
    }
    engine.build().map_err(|e| e.to_string())
}

/// What `engine` holds: a server and a client are made from it, and their script engines read.
fn read(engine: &Engine) -> Result<Reference, String> {
    let files = MemFiles::new();
    files.insert("server.cs", b"function startNothing() {}");
    let server = GameServer::start_engine(
        engine,
        files,
        &ServerStart {
            listen: Listen::Memory(SocketAddr::from((Ipv4Addr::LOCALHOST, 28_000))),
            globals: &[],
            start: "startNothing",
            args: &[],
        },
    )?;
    let client = ClientBuilder::from_engine(engine, None, ClientNet::None)?;
    Ok(collect(
        engine,
        &[
            SideVm {
                side: Side::Server,
                vm: server.vm(),
            },
            SideVm {
                side: Side::Client,
                vm: &client.vm,
            },
        ],
    ))
}

/// The reference: what an engine of [`DefaultPlugins`] holds, and what each opt-in plugin adds to it (an entry the base
/// engine lacks, tagged with the plugin's id).
pub fn reference() -> Result<Reference, String> {
    let mut reference = read(&build(None)?)?;
    for extra in optional_plugins() {
        let with = read(&build(Some(&extra))?)?;
        let tag = vec![extra.id.to_string()];
        for native in with.natives {
            if !reference.natives.iter().any(|n| same_native(n, &native)) {
                // The prediction slot's functions come with any predictor or part, not with the plugin that brought them here.
                let slot = native.namespace.is_empty()
                    && SLOT_NATIVES
                        .iter()
                        .any(|s| s.eq_ignore_ascii_case(&native.name));
                let plugins = if slot {
                    vec![crate::model::SLOT_TAG.to_string()]
                } else {
                    tag.clone()
                };
                reference
                    .natives
                    .push(crate::model::Native { plugins, ..native });
            }
        }
        for class in with.classes {
            if !reference
                .classes
                .iter()
                .any(|c| c.name.eq_ignore_ascii_case(&class.name))
            {
                reference.classes.push(crate::model::Class {
                    plugins: tag.clone(),
                    ..class
                });
            }
        }
        for pref in with.prefs {
            if !reference.prefs.iter().any(|p| p.name == pref.name) {
                reference.prefs.push(crate::model::Pref {
                    plugins: tag.clone(),
                    ..pref
                });
            }
        }
        reference
            .optional
            .push((extra.id.to_string(), extra.what.to_string()));
    }
    Ok(reference)
}

fn same_native(a: &crate::model::Native, b: &crate::model::Native) -> bool {
    a.namespace.eq_ignore_ascii_case(&b.namespace)
        && a.name.eq_ignore_ascii_case(&b.name)
        && a.arity == b.arity
        && a.usage == b.usage
}
