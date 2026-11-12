//! A client that speaks only Torque's protocol (no plugin, no handshake extension) against a server whose engine has the rope plugin:
//! it is never sent a rope, its packets read with TGE's own codec and its table of classes, and it sees what any TGE server would
//! send it. The same server's ropes do reach a client that has the plugin's classes. (A server whose engine has a networked plugin
//! refuses a client without it at the manifest, so this server is the host without that gate: how a game that takes both kinds of
//! client would run.)

use std::cell::RefCell;
use std::rc::Rc;

use torque_app::testing::{NetServer, TgeOnlyClient, connection_of, tge_eval, tge_run};
use torque_ghosts::plugin::PluginClasses;
use torque_net::classes::Kind;
use torque_plugin_rope::RopePlugin;
use torque_plugin_rope::net::{AnchorWire, RopeGhost};
use torque_server::net::NetShared;
use torque_server::net::torque::TorqueNet;
use torque_server::net::transport::MemoryEnd;

type Shared = Rc<RefCell<NetShared<TorqueNet>>>;

/// A server with the game classes, the rope plugin's classes and natives, and a network host on a memory transport (no gate).
fn game_server() -> (NetServer, Shared, MemoryEnd, PluginClasses) {
    let (s, shared, end, engine) = torque_app::testing::game_server(|engine| {
        engine.add_plugins(RopePlugin::default());
    });
    (s, shared, end, engine.net_classes())
}

#[test]
fn a_client_that_is_torques_alone_is_never_sent_a_rope_and_one_with_the_plugin_is() {
    let (mut s, shared, end, plugins) = game_server();
    assert_eq!(plugins.names(Kind::Object).collect::<Vec<_>>(), ["Rope"]);
    tge_eval(
        &mut s,
        "new SimGroup(ClientGroup);
datablock CameraData(EyeData) {};
datablock RopeData(Cable) { segments = 8; };
new Camera(EyeA) { dataBlock = EyeData; };
new Camera(EyeB) { dataBlock = EyeData; };",
    );
    let rope = tge_eval(&mut s, "return ropeNetCreate(\"0 0 3\", \"6 0 3\", Cable);");
    assert!(rope.parse::<u32>().unwrap() > 0, "{rope}");
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

    // Torque's client has what a Torque server sends it: its cameras, no rope, no error.
    assert!(tge.errors.is_empty(), "{:?}", tge.errors);
    assert_eq!(tge.census(), ["Camera", "Camera"]);
    assert!(tge.plugin_records::<RopeGhost>().is_empty());
    assert!(tge.packets > 5, "{} packets", tge.packets);

    // A server that says its clients are the engine's own sends them the rope: this client has the plugin's classes.
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
    // The engine's own client has the rope.
    assert!(own.errors.is_empty(), "{:?}", own.errors);
    assert_eq!(own.census().iter().filter(|c| **c == "Rope").count(), 1);
    let ropes = own.plugin_records::<RopeGhost>();
    assert_eq!(ropes[0].start, Some(AnchorWire::Point([0.0, 0.0, 3.0])));
    assert_eq!(ropes[0].end, Some(AnchorWire::Point([6.0, 0.0, 3.0])));
}
