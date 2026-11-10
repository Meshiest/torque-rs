//! Builds the engines the reference documents and merges what they hold.

use std::net::{Ipv4Addr, SocketAddr};

use torque_app::client::{ClientBuilder, ClientNet};
use torque_app::gizmos::GizmosPlugin;
use torque_app::hosting::HostingPlugin;
use torque_app::plugins::{DefaultPlugins, HierarchyPlugin, ZoomPlugin, set_protocol};
use torque_app::server::{GameServer, Listen, ServerStart};
use torque_client::predict::slot::SLOT_NATIVES;
use torque_engine::net_classes::{GhostRef, Xf};
use torque_engine::rpc::{Allow, Delivery, RpcCx, RpcError};
use torque_engine::{
    Engine, EngineBuilder, NetMessage, Plugin, PluginGroup, PluginGroupBuilder, PluginId,
    PluginType, ProtocolKind,
};
use torque_plugin_gui_layout::GuiLayoutPlugin;
use torque_plugin_props::PropsPlugin;
use torque_plugin_props::predict::PropPredictionPlugin;
use torque_plugin_rope::RopePlugin;
use torque_plugin_tcp::TcpPlugin;
use torque_ts_vm::host::MemFiles;

use crate::collect::{SideVm, collect};
use crate::model::{Reference, Side};

/// An opt-in plugin the reference covers, with a line on what it adds.
struct Optional {
    id: &'static str,
    what: &'static str,
    add: fn(&mut EngineBuilder),
    /// The group the engine starts from in place of [`DefaultPlugins`], for a plugin of that group that is off by default.
    group: Option<fn() -> PluginGroupBuilder>,
}

/// The opt-in plugins that add script surface and that need nothing from the game to do it. (`LivePerfPlugin` names its
/// console function and pref by its arguments, so it has nothing fixed to list.)
fn optional_plugins() -> [Optional; 9] {
    [
        Optional {
            id: HierarchyPlugin::ID.as_str(),
            what: "the scene hierarchy (in the default plugins, on in engine mode): the SceneObject methods that hang an object on another at a local pose, read the tree and delete a subtree, a server API",
            add: |_| {},
            group: Some(|| DefaultPlugins.set(HierarchyPlugin::always())),
        },
        Optional {
            id: GizmosPlugin::ID.as_str(),
            what: "debug drawing: lines, boxes, spheres, arrows, crosses and labels in world space",
            add: |engine| {
                engine.add_plugins(GizmosPlugin::default());
            },
            group: None,
        },
        Optional {
            id: ZoomPlugin::ID.as_str(),
            what: "the zoom: the field of view and how fast it changes",
            add: |engine| {
                engine.add_plugins(ZoomPlugin);
            },
            group: None,
        },
        Optional {
            id: HostingPlugin::ID.as_str(),
            what: "joining by address or room code: the lookup natives of a join screen",
            add: |engine| {
                engine.add_plugins(HostingPlugin::default());
            },
            group: None,
        },
        Optional {
            id: RopePlugin::ID.as_str(),
            what: "ropes: the RopeData datablock, the console functions that hang, push, reel and take down ropes between points or objects, and a server's ropeNet functions that make ropes every client shows",
            add: |engine| {
                engine.add_plugins(RopePlugin::default());
            },
            group: None,
        },
        Optional {
            id: PropsPlugin::ID.as_str(),
            what: "props: the PropData datablock and the Prop class, rigid bodies a server simulates and every client sees, the prop functions that spawn, push, freeze and read them, and the placement checks a game's own spawn commands make; the Joint class and the joint functions (tethers, winches, welds, hinges, grapples) and their check",
            add: |engine| {
                engine.add_plugins(PropsPlugin);
            },
            group: None,
        },
        Optional {
            id: PropPredictionPlugin::ID.as_str(),
            what: "prop prediction on a client: the props its own Player holds, stands on or pushes drawn where they will be, and the client functions that show a push or a spawn at once (never sent to the server)",
            add: |engine| {
                engine.add_plugins((PropsPlugin, PropPredictionPlugin::default()));
            },
            group: None,
        },
        Optional {
            id: TcpPlugin::ID.as_str(),
            what: "TCP and HTTP from script: the TCPObject class (connect, listen, send and the line callbacks) and the HTTPObject class (get and post, HTTPS too), with caps on sockets and bytes per tick",
            add: |engine| {
                engine.add_plugins(TcpPlugin::default());
            },
            group: None,
        },
        Optional {
            id: GuiLayoutPlugin::ID.as_str(),
            what: "flex and grid layout: the GuiFlexCtrl and GuiGridCtrl containers, which lay their children out by CSS's flexbox and grid rules",
            add: |engine| {
                engine.add_plugins(GuiLayoutPlugin);
            },
            group: None,
        },
    ]
}

/// A request of the sample RPCs, with one field of each kind a message documents: a capped string, a sized integer, a float
/// that must be finite and a ghost.
#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
struct SampleRequest {
    #[net(max_len = 64)]
    text: String,
    #[net(bits = 8)]
    count: u8,
    #[net(finite)]
    scale: f32,
    target: GhostRef,
}

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
struct SampleReply {
    #[net(max_len = 64)]
    text: String,
    accepted: bool,
}

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
struct SampleNotice {
    #[net(max_len = 32)]
    label: String,
    #[net(finite)]
    at: Xf,
}

/// Registers one RPC of each kind: a call with a reply and a Rust handler, a notification a client sends that the game's
/// script handles, and a notification the server sends that a client's script handles. An engine with any RPC registers the
/// RPC natives, which the reference documents from this one.
#[derive(PluginId)]
#[plugin(id = "reference.sample-rpc")]
struct SampleRpcPlugin;

impl Plugin for SampleRpcPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine
            .net()
            .rpc::<SampleRequest, SampleReply>("sample.echo")
            .to_server()
            .rate(4, 8)
            .max_bytes(128)
            .server(
                |_cx: &mut RpcCx, req: SampleRequest| -> Result<SampleReply, RpcError> {
                    Ok(SampleReply {
                        text: req.text,
                        accepted: true,
                    })
                },
            );
        engine
            .net()
            .rpc::<SampleRequest, ()>("sample.note")
            .to_server()
            .allow(Allow::permission("sample.note"))
            .server_script();
        engine
            .net()
            .rpc::<SampleNotice, ()>("sample.notice")
            .to_client()
            .delivery(Delivery::Unreliable)
            .max_bytes(64)
            .client_script();
    }
}

fn build(extra: Option<&Optional>) -> Result<Engine, String> {
    let mut engine = EngineBuilder::new();
    match extra.and_then(|extra| extra.group) {
        Some(group) => engine.add_plugins(group()),
        None => engine.add_plugins(DefaultPlugins),
    };
    // Torque's protocol whatever `TORQUE_NET_PROTOCOL` says: the committed reference is of this one.
    set_protocol(&mut engine, ProtocolKind::Tge);
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
    add_rpcs(&mut reference)?;
    Ok(reference)
}

/// The RPC natives and the sample's RPCs: an engine registers the natives only when it has an RPC.
pub(crate) fn add_rpcs(reference: &mut Reference) -> Result<(), String> {
    let mut engine = EngineBuilder::new();
    engine.add_plugins(DefaultPlugins);
    set_protocol(&mut engine, ProtocolKind::Tge);
    engine.add_plugins(SampleRpcPlugin);
    let engine = engine.build().map_err(|e| e.to_string())?;
    let with = read(&engine)?;
    for native in with.natives {
        if !reference.natives.iter().any(|n| same_native(n, &native)) {
            reference.natives.push(crate::model::Native {
                plugins: vec![crate::model::RPC_TAG.to_string()],
                ..native
            });
        }
    }
    reference.rpcs = crate::collect::rpcs(&engine)?;
    Ok(())
}

fn same_native(a: &crate::model::Native, b: &crate::model::Native) -> bool {
    a.namespace.eq_ignore_ascii_case(&b.namespace)
        && a.name.eq_ignore_ascii_case(&b.name)
        && a.arity == b.arity
        && a.usage == b.usage
}
