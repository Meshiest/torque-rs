//! A server started with no network host (`Listen::Late`) whose host is attached afterwards (`GameServer::attach_net`), its
//! scripts run after the attach as a game's own start-up runs them: it has the natives of a server started on the memory link,
//! and a client connects to it and plays, writing the same datagrams as against that server, in either protocol.

use std::net::SocketAddr;

use torque_app_server::plugins::{MinimalPlugins, set_protocol};
use torque_app_server::server::{GameServer, Listen, ServerLink, ServerStart};
use torque_app_server::slots::HostSlots;
use torque_engine::{EngineBuilder, Plugin, PluginId};
use torque_trace_hash::rig::{self, Control, Mode, Rig};

/// The game's entry, which takes connections and runs no script: the scripts run once the host is attached.
#[derive(PluginId)]
struct OpenEntry;

impl Plugin for OpenEntry {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.server_entry(|entry| {
            entry.connections.allow(true);
            Ok(())
        });
    }
}

/// No plugin under test.
#[derive(PluginId)]
struct Nothing;

impl Plugin for Nothing {
    fn build(&self, _: &mut EngineBuilder) {}
}

/// A rig over a server started on `Listen::Late`, its host attached on the rig's link before `server.cs` and `startTrace` run.
fn late_rig(mode: Mode) -> Rig {
    let mut builder = EngineBuilder::new();
    builder.add_plugins(MinimalPlugins).add_plugins(OpenEntry);
    set_protocol(&mut builder, mode.protocol());
    let engine = builder.build().expect("the engine builds");
    let files = rig::files();
    let mut server = GameServer::start_with_net(
        &engine,
        files.clone(),
        &ServerStart {
            listen: Listen::Late,
            globals: &[("$Trace::Control", "figure")],
            start: "startTrace",
            args: &[],
        },
    )
    .expect("the server starts");
    assert!(server.shared().is_none(), "no host before the attach");
    let link = ServerLink::memory(SocketAddr::V4(rig::SERVER)).expect("the link binds");
    server.attach_net(link).expect("the host attaches");
    assert!(
        server
            .vm_mut()
            .exec_file(b"server.cs", false)
            .expect("server.cs runs")
    );
    server.call("startTrace", &[]).expect("the map starts");
    Rig::over_server(mode, engine, server, files, |_| Ok(())).expect("the client starts")
}

/// The datagrams of a session: settled, then a second of walking forward.
fn session(rig: &mut Rig) -> Vec<(bool, Vec<u8>)> {
    rig.record_wire();
    assert!(rig.settle(400), "{:?}: the client predicts", rig.mode);
    rig.set_global("$mvForwardAction", 1.0);
    for _ in 0..32 {
        rig.step();
    }
    assert!(
        rig.predicting(),
        "{:?}: the client still predicts",
        rig.mode
    );
    std::mem::take(&mut rig.wire)
}

#[test]
fn late_net_a_client_plays_on_a_late_host_as_on_a_server_started_on_the_link() {
    for mode in Mode::ALL {
        let mut late = late_rig(mode);
        let mut normal = Rig::in_mode(mode, Nothing, Control::Figure).expect("the rig starts");
        assert_eq!(
            late.server.vm().native_listing(),
            normal.server.vm().native_listing(),
            "{mode:?}: the natives"
        );
        let late_wire = session(&mut late);
        let normal_wire = session(&mut normal);
        assert!(!late_wire.is_empty());
        assert!(
            late_wire == normal_wire,
            "{mode:?}: the late host's session wrote other datagrams ({} against {})",
            late_wire.len(),
            normal_wire.len()
        );
    }
}
