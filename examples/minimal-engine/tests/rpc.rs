//! Plugin RPCs through the engine's own server and client over the memory transport, in TGE mode (the reserved channel): a call
//! with a reply, a notification each way, the script natives and a script handler, the rate limit and its counter, and a
//! client that does not have the server's RPCs being refused.

use std::cell::RefCell;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::rc::Rc;

use torque::app::client::{ClientNet, GameClient};
use torque::app::server::Listen;
use torque::engine::rpc::{Allow, RpcCx, RpcError};
use torque::engine::{EngineBuilder, NetMessage, Plugin, PluginId};
use torque::server::net::channels::{ChunkChannel, ClientChannels};
use torque::server::net::rpc::{ClientRpc, ServerRpc};
use torque_example_minimal_engine::{builder, client, files, server};

const SERVER: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_100);
const CLIENT: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 28_101);

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
#[net(crate = torque::engine)]
struct Say {
    #[net(max_len = 64)]
    text: String,
}

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
#[net(crate = torque::engine)]
struct Said {
    #[net(max_len = 80)]
    text: String,
}

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
#[net(crate = torque::engine)]
struct Line {
    #[net(max_len = 64)]
    text: String,
}

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
#[net(crate = torque::engine)]
struct Move {
    #[net(finite)]
    to: [f32; 3],
}

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
#[net(crate = torque::engine)]
struct Look {
    who: torque::engine::net_classes::GhostRef,
}

/// What the handlers saw in this one process.
#[derive(Clone, Default)]
struct Seen {
    said: Rc<RefCell<Vec<String>>>,
    lines: Rc<RefCell<Vec<String>>>,
    looked: Rc<RefCell<Vec<Option<u32>>>>,
    moves: Rc<RefCell<Vec<[f32; 3]>>>,
}

/// A bulk channel that says nothing and moves so slowly it never empties: it holds the connection's shared budget.
#[derive(Default)]
struct Quiet;

impl ChunkChannel for Quiet {}

#[derive(PluginId)]
#[plugin(crate = torque::engine)]
struct Chat(Seen);

impl Plugin for Chat {
    fn build(&self, engine: &mut EngineBuilder) {
        let seen = self.0.clone();
        engine
            .net()
            .rpc::<Say, Said>("chat.say")
            .to_server()
            .rate(4, 8)
            .allow(Allow::Anyone)
            .server(move |_cx: &mut RpcCx, req: Say| {
                seen.said.borrow_mut().push(req.text.clone());
                Ok(Said {
                    text: format!("echo: {}", req.text),
                })
            });
        let seen = self.0.clone();
        engine
            .net()
            .rpc::<Line, ()>("chat.line")
            .to_client()
            .client(move |_cx: &mut RpcCx, req: Line| {
                seen.lines.borrow_mut().push(req.text);
                Ok(())
            });
        let seen = self.0.clone();
        engine
            .net()
            .rpc::<Move, ()>("chat.move")
            .to_client()
            .client(move |_cx: &mut RpcCx, req: Move| {
                seen.moves.borrow_mut().push(req.to);
                Ok(())
            });
        engine.net().channel::<Quiet>("fill.a", 16);
        engine.net().channel::<Quiet>("fill.b", 16);
        let seen = self.0.clone();
        engine
            .net()
            .rpc::<Look, ()>("chat.look")
            .to_client()
            .client(move |cx: &mut RpcCx, req: Look| {
                seen.looked.borrow_mut().push(cx.resolve(req.who));
                Ok(())
            });
        engine
            .net()
            .rpc::<Line, ()>("chat.note")
            .to_server()
            .rate(2, 4)
            .server_script();
        engine
            .net()
            .rpc::<Say, ()>("chat.secret")
            .to_server()
            .allow(Allow::permission("chat.admin"))
            .server(|_cx: &mut RpcCx, _req: Say| -> Result<(), RpcError> { Ok(()) });
    }
}

struct Rig {
    client: GameClient,
    rpc: Rc<ClientRpc>,
    server_rpc: Rc<ServerRpc>,
}

fn start(seen: &Seen) -> Rig {
    let files = files().unwrap();
    let mut b = builder(&files).unwrap();
    b.add_plugins(Chat(seen.clone()));
    let engine = b.build().unwrap();
    assert_eq!(engine.net_rpcs().unwrap().len(), 6);
    let server = server(&engine, Listen::Memory(SERVER)).expect("the server starts");
    let server_rpc = server
        .vm()
        .extension::<ServerRpc>()
        .expect("the server has RPCs");
    let mut client = client(
        &engine,
        ClientNet::Memory {
            client: CLIENT,
            server: SERVER,
        },
    )
    .expect("the client");
    let rpc = client
        .vm()
        .extension::<ClientRpc>()
        .expect("the client has RPCs");
    let channels = client.vm().extension::<ClientChannels>().expect("channels");
    client.start_hosting(server);
    client.call("connectTo", &[&SERVER.to_string()]).unwrap();
    let mut steps = 0;
    while !(channels.is_active() && rpc.is_ready()) && steps < 2_000 {
        client.step(32);
        steps += 1;
    }
    assert!(
        rpc.is_ready(),
        "the client's RPC link starts with the connection"
    );
    // One more tick so the server has a link too.
    client.step(64);
    // Both ends write a ghost reference as wide as the accept said.
    let conn = *client
        .server
        .as_ref()
        .unwrap()
        .vm()
        .extension::<torque::server::net::channels::ChannelHub>()
        .unwrap()
        .clients()
        .first()
        .unwrap();
    assert_eq!(
        server_rpc.with_link(conn, |l| l.widths()),
        rpc.with_link(|l| l.widths()),
        "the two links agree on the widths"
    );
    Rig {
        client,
        rpc,
        server_rpc,
    }
}

fn step_until(rig: &mut Rig, what: &str, done: impl Fn() -> bool) {
    let mut steps = 0;
    while !done() && steps < 2_000 {
        rig.client.step(32);
        steps += 1;
    }
    assert!(done(), "{what} did not happen: {:?}", rig.client.client_log);
}

#[test]
fn calls_and_notifications_cross_in_both_directions() {
    let seen = Seen::default();
    let mut rig = start(&seen);

    // A call with a reply.
    let reply: Rc<RefCell<Option<Result<Said, RpcError>>>> = Rc::default();
    {
        let sink = Rc::clone(&reply);
        let rpc = rig
            .rpc
            .with_link(|l| l.rpc::<Say, Said>("chat.say"))
            .unwrap()
            .unwrap();
        rig.rpc
            .with_link(|l| {
                l.call(&rpc, Say { text: "hi".into() }, move |r| {
                    *sink.borrow_mut() = Some(r);
                })
            })
            .unwrap()
            .expect("queued");
    }
    step_until(&mut rig, "the reply", || reply.borrow().is_some());
    assert_eq!(
        reply.borrow().clone(),
        Some(Ok(Said {
            text: "echo: hi".into()
        }))
    );
    assert_eq!(*seen.said.borrow(), ["hi"]);

    // A notification from the server, from Rust and from the server's script.
    let conn = *rig
        .client
        .server
        .as_ref()
        .unwrap()
        .vm()
        .extension::<torque::server::net::channels::ChannelHub>()
        .unwrap()
        .clients()
        .first()
        .unwrap();
    let line = rig
        .server_rpc
        .with_link(conn, |l| l.rpc::<Line, ()>("chat.line"))
        .unwrap()
        .unwrap();
    rig.server_rpc
        .notify(
            conn,
            &line,
            Line {
                text: "from rust".into(),
            },
        )
        .expect("queued");
    step_until(&mut rig, "the notification", || {
        seen.lines.borrow().len() == 1
    });
    let answer = rig
        .client
        .server
        .as_mut()
        .unwrap()
        .vm_mut()
        .eval(b"return rpcToClient(ClientGroup.getObject(0), \"chat.line\", \"from script\");")
        .unwrap();
    assert_eq!(answer, b"", "queued without a complaint");
    step_until(&mut rig, "the script's notification", || {
        seen.lines.borrow().len() == 2
    });
    assert_eq!(*seen.lines.borrow(), ["from rust", "from script"]);
}

#[test]
fn the_script_calls_and_handles_rpcs() {
    let seen = Seen::default();
    let mut rig = start(&seen);
    rig.client
        .server
        .as_mut()
        .unwrap()
        .vm_mut()
        .eval(
            b"function rpcServer_chat_note(%client, %text) { $Test::noted = %text; $Test::notedBy = %client; }\n\
              function onRpcCheck(%client, %name) { if (%name $= \"chat.secret\") { return 0; } return 1; }",
        )
        .unwrap();
    let answer = rig
        .client
        .vm_mut()
        .eval(b"return rpcToServer(\"chat.note\", \"hello\");")
        .unwrap();
    assert_eq!(answer, b"");
    let mut steps = 0;
    while rig
        .client
        .server
        .as_ref()
        .unwrap()
        .vm()
        .global("$Test::noted")
        .is_none()
        && steps < 2_000
    {
        rig.client.step(32);
        steps += 1;
    }
    let server = rig.client.server.as_ref().unwrap();
    assert_eq!(
        server.vm().global("$Test::noted").as_deref(),
        Some(&b"hello"[..])
    );
    assert!(server.vm().global("$Test::notedBy").is_some());
    // An unknown RPC and a call that is not this side's come back as a reason, not a panic.
    let unknown = rig
        .client
        .vm_mut()
        .eval(b"return rpcToServer(\"nothing\");")
        .unwrap();
    assert_eq!(unknown, b"no such call");
    let wrong_way = rig
        .client
        .vm_mut()
        .eval(b"return rpcToServer(\"chat.line\", \"x\");")
        .unwrap();
    assert_eq!(wrong_way, b"no such call");
}

#[test]
fn a_flood_is_dropped_and_counted() {
    let seen = Seen::default();
    let mut rig = start(&seen);
    rig.client
        .server
        .as_mut()
        .unwrap()
        .vm_mut()
        .eval(b"function rpcServer_chat_note(%client, %text) { $Test::notes = $Test::notes + 1; }")
        .unwrap();
    rig.client
        .server
        .as_mut()
        .unwrap()
        .vm_mut()
        .set_global("$Test::notes", "0");
    // `chat.note` is 2 a second with a burst of 4. The client holds its own calls to that: 36 of 40 in one tick are refused
    // here with a reason, and the server sees (and drops) none.
    let refused = rig
        .client
        .vm_mut()
        .eval(
            b"$refused = 0; for (%i = 0; %i < 40; %i++) { if (rpcToServer(\"chat.note\", \"x\") !$= \"\") { $refused++; } } return $refused;",
        )
        .unwrap();
    assert_eq!(refused, b"36");
    for _ in 0..40 {
        rig.client.step(32);
    }
    let server = rig.client.server.as_mut().unwrap();
    let ran: i32 = String::from_utf8_lossy(&server.vm().global("$Test::notes").unwrap())
        .parse()
        .unwrap();
    assert_eq!(ran, 4, "the burst ran");
    let drops = server
        .vm_mut()
        .eval(b"return rpcDrops(ClientGroup.getObject(0), \"chat.note\");")
        .unwrap();
    assert_eq!(
        drops, b"0",
        "an honest client sends nothing the server must drop"
    );
    assert!(
        rig.client
            .client_log
            .iter()
            .all(|l| !l.contains("malformed"))
    );
}

#[test]
fn a_permission_rpc_is_refused_to_a_player_and_runs_for_a_holder() {
    let seen = Seen::default();
    let mut rig = start(&seen);
    let reply: Rc<RefCell<Option<Result<(), RpcError>>>> = Rc::default();
    let call = |rig: &Rig, sink: &Rc<RefCell<Option<Result<(), RpcError>>>>| {
        let rpc = rig
            .rpc
            .with_link(|l| l.rpc::<Say, ()>("chat.secret"))
            .unwrap()
            .unwrap();
        let sink = Rc::clone(sink);
        // A notification: a refusal is counted on the server, and nothing comes back.
        let _ = sink;
        rig.rpc
            .with_link(|l| l.notify(&rpc, Say { text: "x".into() }))
            .unwrap()
            .expect("queued");
    };
    call(&rig, &reply);
    for _ in 0..20 {
        rig.client.step(32);
    }
    let conn = *rig
        .client
        .server
        .as_ref()
        .unwrap()
        .vm()
        .extension::<torque::server::net::channels::ChannelHub>()
        .unwrap()
        .clients()
        .first()
        .unwrap();
    let refused = rig
        .server_rpc
        .with_link(conn, |l| l.refusals("chat.secret"))
        .unwrap();
    assert_eq!(refused, 1, "a player is refused");
    // The same player, once the script grants a wildcard that covers it, is let in.
    rig.client
        .server
        .as_mut()
        .unwrap()
        .vm_mut()
        .eval(b"ClientGroup.getObject(0).grant(\"chat.*\");")
        .unwrap();
    call(&rig, &reply);
    for _ in 0..20 {
        rig.client.step(32);
    }
    let refused = rig
        .server_rpc
        .with_link(conn, |l| l.refusals("chat.secret"))
        .unwrap();
    assert_eq!(refused, 1, "a holder is not");
}

#[test]
fn the_servers_script_can_deny_a_call_that_was_allowed() {
    let seen = Seen::default();
    let mut rig = start(&seen);
    let server = rig.client.server.as_mut().unwrap();
    server
        .vm_mut()
        .eval(
            b"ClientGroup.getObject(0).grant(\"chat.*\");\n\
              function onRpcCheck(%client, %name) { return %name !$= \"chat.secret\"; }",
        )
        .unwrap();
    let conn = *server
        .vm()
        .extension::<torque::server::net::channels::ChannelHub>()
        .unwrap()
        .clients()
        .first()
        .unwrap();
    let rpc = rig
        .rpc
        .with_link(|l| l.rpc::<Say, ()>("chat.secret"))
        .unwrap()
        .unwrap();
    rig.rpc
        .with_link(|l| l.notify(&rpc, Say { text: "x".into() }))
        .unwrap()
        .expect("queued");
    for _ in 0..20 {
        rig.client.step(32);
    }
    let refused = rig
        .server_rpc
        .with_link(conn, |l| l.refusals("chat.secret"))
        .unwrap();
    assert_eq!(refused, 1, "a holder the script denies is refused");
}

fn first_conn(rig: &Rig) -> torque::net::handshake::ConnId {
    *rig.client
        .server
        .as_ref()
        .unwrap()
        .vm()
        .extension::<torque::server::net::channels::ChannelHub>()
        .unwrap()
        .clients()
        .first()
        .unwrap()
}

#[test]
fn a_client_resolves_the_ghosts_a_notification_names_and_refuses_the_rest() {
    let seen = Seen::default();
    let mut rig = start(&seen);
    // The world takes a few hundred ticks to ghost.
    for _ in 0..300 {
        rig.client.step(32);
    }
    let conn = first_conn(&rig);
    // A ghost the server really sent this client, and the object its mirror is in the client's world.
    let (index, ghosts) = {
        let server = rig.client.server.as_ref().unwrap();
        let shared = server.shared.borrow();
        let ghosts = shared.ghosts.as_ref().expect("ghosts").clone();
        let index = (0..1024u32)
            .find(|i| ghosts.object_at(conn, *i).is_some())
            .expect("the server ghosted something");
        (index, ghosts)
    };
    let _ = ghosts;
    let look = rig
        .server_rpc
        .with_link(conn, |l| l.rpc::<Look, ()>("chat.look"))
        .unwrap()
        .unwrap();
    let who = |i: u32| Look {
        who: torque::engine::net_classes::GhostRef(i),
    };
    rig.server_rpc
        .notify(conn, &look, who(index))
        .expect("queued");
    rig.server_rpc.notify(conn, &look, who(63)).expect("queued");
    step_until(&mut rig, "the checked notification", || {
        !seen.looked.borrow().is_empty()
    });
    for _ in 0..20 {
        rig.client.step(32);
    }
    let looked = seen.looked.borrow().clone();
    assert_eq!(
        looked.len(),
        1,
        "the unknown ghost never reached the handler: {looked:?}"
    );
    assert!(looked[0].is_some(), "the known ghost resolved to an object");
    assert_eq!(rig.rpc.with_link(|l| l.refusals("chat.look")), Some(1));
}

#[test]
fn a_malformed_batch_from_the_server_ends_the_connection_with_a_reason() {
    let seen = Seen::default();
    let mut rig = start(&seen);
    let conn = first_conn(&rig);
    let hub = rig
        .client
        .server
        .as_ref()
        .unwrap()
        .vm()
        .extension::<torque::server::net::channels::ChannelHub>()
        .unwrap();
    // Bytes no server of this engine sends: a frame that says it is longer than the batch, on the carrier the protocol uses
    // (the session's RPC lane in the engine's, the reserved channel in Torque's).
    let lanes = rig
        .client
        .server
        .as_ref()
        .unwrap()
        .vm()
        .extension::<torque::server::net::rpc::RpcLanes>();
    match lanes {
        Some(lanes) => lanes.give_back(
            Some(conn),
            vec![(vec![200, 0, 1], torque::engine::rpc::Delivery::Reliable)],
        ),
        None => hub
            .send_to(
                conn,
                "torque.rpc",
                torque::net::chunk::Kind::Delta,
                vec![200, 0, 1],
            )
            .expect("queued"),
    }
    let mut steps = 0;
    while !hub.clients().is_empty() && steps < 2_000 {
        rig.client.step(32);
        steps += 1;
    }
    assert!(hub.clients().is_empty(), "the client hung up");
    assert!(
        rig.client
            .client_log
            .iter()
            .any(|l| l.contains("malformed RPC message")),
        "the client says why: {:?}",
        rig.client.client_log
    );
}

#[test]
fn a_script_cannot_make_the_server_send_a_value_that_disconnects_every_client() {
    let seen = Seen::default();
    let mut rig = start(&seen);
    let hub = rig
        .client
        .server
        .as_ref()
        .unwrap()
        .vm()
        .extension::<torque::server::net::channels::ChannelHub>()
        .unwrap();
    let mut asked = Vec::new();
    for value in ["1e999 0 0", "inf 0 0", "nan 0 0", "1 2 3"] {
        let code =
            format!("return rpcToClient(ClientGroup.getObject(0), \"chat.move\", \"{value}\");");
        let answer = rig
            .client
            .server
            .as_mut()
            .unwrap()
            .vm_mut()
            .eval(code.as_bytes())
            .unwrap();
        asked.push(String::from_utf8_lossy(&answer).into_owned());
    }
    assert!(
        asked[..3].iter().all(|a| a.contains("does not allow")),
        "{asked:?}"
    );
    assert_eq!(asked[3], "");
    for _ in 0..40 {
        rig.client.step(32);
    }
    assert_eq!(hub.clients().len(), 1, "the client was not disconnected");
    assert_eq!(
        *seen.moves.borrow(),
        [[1.0, 2.0, 3.0]],
        "only the finite one arrived"
    );
    assert!(
        rig.client
            .client_log
            .iter()
            .all(|l| !l.contains("malformed"))
    );
}

#[test]
fn a_script_names_objects_and_the_client_handler_gets_the_mirror() {
    let seen = Seen::default();
    let mut rig = start(&seen);
    for _ in 0..300 {
        rig.client.step(32);
    }
    let conn = first_conn(&rig);
    let object = {
        let server = rig.client.server.as_ref().unwrap();
        let shared = server.shared.borrow();
        let ghosts = shared.ghosts.as_ref().expect("ghosts").clone();
        (0..1024u32)
            .find_map(|i| ghosts.object_at(conn, i))
            .expect("the server ghosted something")
    };
    let code = format!("return rpcToClient(ClientGroup.getObject(0), \"chat.look\", {object});");
    let answer = rig
        .client
        .server
        .as_mut()
        .unwrap()
        .vm_mut()
        .eval(code.as_bytes())
        .unwrap();
    assert_eq!(
        answer, b"",
        "an object the client has a ghost of is sent as its ghost index"
    );
    step_until(&mut rig, "the look", || !seen.looked.borrow().is_empty());
    assert!(
        seen.looked.borrow()[0].is_some(),
        "the client resolved the index to its mirror"
    );
    // An object the client has no ghost of is refused here, with a reason, and never sent.
    let answer = rig
        .client
        .server
        .as_mut()
        .unwrap()
        .vm_mut()
        .eval(b"return rpcToClient(ClientGroup.getObject(0), \"chat.look\", 1999999);")
        .unwrap();
    assert!(String::from_utf8_lossy(&answer).contains("does not allow"));
}

#[test]
fn a_notification_still_arrives_when_bulk_channels_fill_the_shared_budget() {
    let seen = Seen::default();
    let mut rig = start(&seen);
    let conn = first_conn(&rig);
    let hub = rig
        .client
        .server
        .as_ref()
        .unwrap()
        .vm()
        .extension::<torque::server::net::channels::ChannelHub>()
        .unwrap();
    // Fill what the bulk channels share (a mods-download-like load): 4 MiB messages until the budget refuses one.
    let mut queued = 0;
    for channel in [
        "fill.a", "fill.b", "fill.a", "fill.b", "fill.a", "fill.b", "fill.a", "fill.b",
    ] {
        let sent = hub.send_to(
            conn,
            channel,
            torque::net::chunk::Kind::Snapshot,
            vec![7; 4 << 20],
        );
        queued += usize::from(sent.is_ok());
    }
    assert!(
        (3..8).contains(&queued),
        "the shared budget filled after {queued} messages"
    );
    let refused = hub.send_to(
        conn,
        "fill.a",
        torque::net::chunk::Kind::Snapshot,
        vec![7; 4 << 20],
    );
    assert!(refused.is_err(), "the bulk channels have no room left");
    // A notification still goes out and arrives, in the packets' room the bulk data leaves.
    let line = rig
        .server_rpc
        .with_link(conn, |l| l.rpc::<Line, ()>("chat.line"))
        .unwrap()
        .unwrap();
    rig.server_rpc
        .notify(
            conn,
            &line,
            Line {
                text: "through the load".into(),
            },
        )
        .expect("queued");
    step_until(&mut rig, "the notification", || {
        !seen.lines.borrow().is_empty()
    });
    assert_eq!(*seen.lines.borrow(), ["through the load"]);
}
