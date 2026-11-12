//! A client that speaks only Torque's protocol (no plugin, no handshake extension) against a server whose engine has the props plugin:
//! it is never sent a prop, its packets read with TGE's own codec and its table of classes, and it sees what any TGE server would
//! send it. The same server's props do reach a client that has the plugin's classes. (A server whose engine has a networked plugin
//! refuses a client without it at the manifest, so this server is the host without that gate: how a game that takes both kinds of
//! client would run.)

use std::cell::RefCell;
use std::rc::Rc;

use torque_app::testing::{NetServer, TgeOnlyClient, connection_of, tge_eval, tge_run};
use torque_ghosts::plugin::PluginClasses;
use torque_net::classes::Kind;
use torque_plugin_props::{PropGhost, PropsPlugin, ServerProps};
use torque_server::net::NetShared;
use torque_server::net::torque::TorqueNet;
use torque_server::net::transport::MemoryEnd;

type Shared = Rc<RefCell<NetShared<TorqueNet>>>;

/// A server with the game classes, the props plugin's classes and natives, and a network host on a memory transport (no gate).
fn game_server() -> (NetServer, Shared, MemoryEnd, PluginClasses, ServerProps) {
    let (s, shared, end, engine) = torque_app::testing::game_server(|engine| {
        engine.add_plugins(PropsPlugin);
    });
    let props = engine
        .resource::<ServerProps>()
        .expect("the server's props")
        .clone();
    (s, shared, end, engine.net_classes(), props)
}

#[test]
fn a_client_that_is_torques_alone_is_never_sent_a_prop_and_one_with_the_plugin_is() {
    let (mut s, shared, end, plugins, _) = game_server();
    assert_eq!(
        plugins.names(Kind::Object).collect::<Vec<_>>(),
        ["Joint", "Prop"]
    );
    tge_eval(
        &mut s,
        "new SimGroup(ClientGroup);
datablock CameraData(EyeData) {};
datablock PropData(Crate) { size = \"1 1 1\"; mass = 20; };
new Camera(EyeA) { dataBlock = EyeData; };
new Camera(EyeB) { dataBlock = EyeData; };",
    );
    let prop = tge_eval(&mut s, "return propSpawn(Crate, \"1 2 3\");");
    assert!(prop.parse::<u32>().unwrap() > 0, "{prop}");
    let joint = tge_eval(
        &mut s,
        &format!("return jointTether({prop}, \"1 2 6\", 3);"),
    );
    assert!(joint.parse::<u32>().unwrap() > 0, "{joint}");
    let mut tge = TgeOnlyClient::new(50_001, 7, PluginClasses::default());
    tge.start(&end);
    tge_run(&mut s, &end, &mut [&mut tge], 10);
    assert!(tge.connected());
    let conn_tge = connection_of(&shared, tge.addr);
    tge_eval(
        &mut s,
        &format!("{conn_tge}.setControlObject(EyeA); {conn_tge}.activateGhosting();"),
    );
    tge_run(&mut s, &end, &mut [&mut tge], 30);

    // Torque's client has what a Torque server sends it: its cameras, no prop, no joint, no error.
    assert!(tge.errors.is_empty(), "{:?}", tge.errors);
    assert_eq!(tge.census(), ["Camera", "Camera"]);
    assert!(tge.plugin_records::<PropGhost>().is_empty());
    assert!(tge.packets > 5, "{} packets", tge.packets);

    // A server that says its clients are the engine's own sends them the prop: this client has the plugin's classes.
    tge_eval(&mut s, "$Server::EngineClasses = 1;");
    let mut own = TgeOnlyClient::new(50_002, 9, plugins);
    own.start(&end);
    tge_run(&mut s, &end, &mut [&mut tge, &mut own], 10);
    assert!(own.connected());
    let conn_own = connection_of(&shared, own.addr);
    tge_eval(
        &mut s,
        &format!("{conn_own}.setControlObject(EyeB); {conn_own}.activateGhosting();"),
    );
    tge_run(&mut s, &end, &mut [&mut tge, &mut own], 30);
    assert!(own.errors.is_empty(), "{:?}", own.errors);
    assert_eq!(own.census().iter().filter(|c| **c == "Prop").count(), 1);
    assert_eq!(own.census().iter().filter(|c| **c == "Joint").count(), 1);
    let props = own.plugin_records::<PropGhost>();
    let state = props[0].state.expect("a state").0;
    assert_eq!(state.position(), [1.0, 2.0, 3.0]);
}

#[test]
fn frozen_props_change_nothing_for_a_client_that_is_torques_alone_or_has_no_channel() {
    let (mut s, shared, end, plugins, props) = game_server();
    tge_eval(
        &mut s,
        "new SimGroup(ClientGroup);
datablock CameraData(EyeData) {};
datablock PropData(Crate) { size = \"0.4 0.4 0.4\"; mass = 20; };
new Camera(EyeA) { dataBlock = EyeData; };
new Camera(EyeB) { dataBlock = EyeData; };
$Pref::Props::FrozenGhosts = 0;",
    );
    // Every frozen prop would go over the channel, if any client had one.
    tge_eval(
        &mut s,
        "for (%i = 0; %i < 120; %i++) { %p = propSpawn(Crate, (%i % 12) SPC mFloor(%i / 12) SPC 3); propFreeze(%p); }",
    );
    let mut tge = TgeOnlyClient::new(50_001, 7, PluginClasses::default());
    tge.start(&end);
    tge_run(&mut s, &end, &mut [&mut tge], 10);
    let conn_tge = connection_of(&shared, tge.addr);
    tge_eval(
        &mut s,
        &format!("{conn_tge}.setControlObject(EyeA); {conn_tge}.activateGhosting();"),
    );
    tge_run(&mut s, &end, &mut [&mut tge], 60);
    // Torque's client has what a Torque server sends it: no prop, no error, nothing about a channel.
    assert!(tge.errors.is_empty(), "{:?}", tge.errors);
    assert_eq!(tge.census(), ["Camera", "Camera"]);
    // A client of the engine's own without the channel enabled is ghosted every prop (the channel is only for connections that
    // have it): it is never left short.
    tge_eval(&mut s, "$Server::EngineClasses = 1;");
    let mut own = TgeOnlyClient::new(50_002, 9, plugins);
    own.start(&end);
    tge_run(&mut s, &end, &mut [&mut tge, &mut own], 10);
    let conn_own = connection_of(&shared, own.addr);
    tge_eval(
        &mut s,
        &format!("{conn_own}.setControlObject(EyeB); {conn_own}.activateGhosting();"),
    );
    tge_run(&mut s, &end, &mut [&mut tge, &mut own], 400);
    assert!(own.errors.is_empty(), "{:?}", own.errors);
    assert_eq!(own.census().iter().filter(|c| **c == "Prop").count(), 120);
    // The server dealt no client any frozen prop and kept none out of any ghosts.
    assert!(props.frozen_counts().is_empty());
    assert_eq!(props.frozen_stats(), Default::default());
    let scope = torque_engine::scope::ScopeControl::of(&s.vm).expect("a scope control");
    assert_eq!(
        scope.hidden_count(torque_engine::scope::Audience::Everyone),
        0
    );
    assert_eq!(
        scope.hidden_count(torque_engine::scope::Audience::Connection(conn_own)),
        0
    );
}
