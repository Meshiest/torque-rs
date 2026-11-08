use std::cell::RefCell;
use std::collections::VecDeque;
use std::io;
use std::rc::Rc;
use std::time::Duration;

use ghosts::sky::{Sky, Sun, SunUpdate};
use ghosts::{F32, TgeClass, TgeRecord};

use super::*;
use crate::net::{ClientNet, DatagramQueue, MemoryDatagrams};

/// A link whose sends are kept and that hears nothing.
struct Kept(Rc<RefCell<VecDeque<Vec<u8>>>>);

impl Link for Kept {
    fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.0.borrow_mut().push_back(bytes.to_vec());
        Ok(())
    }

    fn recv(&mut self, _wait: Duration) -> io::Result<Option<Vec<u8>>> {
        Ok(None)
    }
}

fn ghost(class: TgeClass, record: TgeRecord) -> Ghost<TgeClientSet> {
    Ghost {
        class: Some(class),
        serial: 1,
        always: true,
        record: Some(record),
        part: (),
        position: None,
        updates: 0,
    }
}

#[test]
fn a_datablocks_fields_print_as_the_console_prints_them() {
    let event = TgeDatablockEvent {
        id: 40,
        datablock: TgeDatablock {
            class: "ItemData".to_string(),
            fields: vec![
                ("shapeFile".to_string(), 0, Value::str("ball.dts")),
                ("mass".to_string(), 0, Value::F32(2.5)),
                ("sticky".to_string(), 0, Value::Bool(true)),
                ("maxInventory".to_string(), 0, Value::Int(3)),
                ("image".to_string(), 0, Value::Id(Some(41))),
                ("textures".to_string(), 0, Value::str("a")),
                ("textures".to_string(), 1, Value::str("b")),
                ("transform".to_string(), 0, Value::Xform(None)),
                ("unset".to_string(), 0, Value::Null),
            ],
        },
    };
    let d = unpack_datablock(FieldRules::default(), &event);
    assert_eq!((d.id, d.class.as_str()), (40, "ItemData"));
    type Field<'a> = (&'a [u8], &'a [u8], &'a [u8], Option<u32>);
    let fields: Vec<Field<'_>> = d
        .fields
        .iter()
        .map(|f| {
            (
                f.name.as_slice(),
                f.index.as_slice(),
                f.text.as_slice(),
                f.pointer,
            )
        })
        .collect();
    assert_eq!(
        fields,
        vec![
            (&b"shapeFile"[..], &b""[..], &b"ball.dts"[..], None),
            (&b"mass"[..], &b""[..], &b"2.5"[..], None),
            (&b"sticky"[..], &b""[..], &b"1"[..], None),
            (&b"maxInventory"[..], &b""[..], &b"3"[..], None),
            (&b"image"[..], &b""[..], &b""[..], Some(41)),
            (&b"textures"[..], &b"0"[..], &b"a"[..], None),
            (&b"textures"[..], &b"1"[..], &b"b"[..], None),
        ]
    );
    assert!(d.error.is_none() && d.emitter_ids.is_none());
}

#[test]
fn a_local_feed_is_connected_at_once_and_sends_its_packets() {
    let sent = Rc::new(RefCell::new(VecDeque::new()));
    let mut feed = TgeFeed::new(
        Box::new(Kept(Rc::clone(&sent))),
        b"",
        vec![Vec::new(); 5],
        0,
        Some((12, 14)),
    )
    .expect("TGE's tables");
    assert!(feed.take_events(usize::MAX).is_empty());
    assert_eq!(feed.protocol_version(), 12);
    assert_eq!(feed.ghost_bits(), 14);
    assert!(!feed.is_gone());
    let mut world = TgeWorld::default();
    feed.tick_start(&Vm::new(), 1_000);
    feed.pump(&mut world).unwrap();
    feed.send_tick(&mut world, &ClientBlock::default());
    assert_eq!(sent.borrow().len(), 1, "the tick's packet");
    feed.disconnect("bye");
    assert!(feed.is_gone());
    assert_eq!(sent.borrow().len(), 2, "the disconnect");
}

#[test]
fn a_network_feed_sends_its_challenge_again_and_gives_up_after_tges_tries() {
    let sent = Rc::new(RefCell::new(VecDeque::new()));
    let mut feed = TgeFeed::new(
        Box::new(Kept(Rc::clone(&sent))),
        b"",
        vec![Vec::new(); 5],
        7,
        None,
    )
    .expect("TGE's tables");
    let mut world = TgeWorld::default();
    let mut challenges = 0;
    let mut timed_out = false;
    let mut sends = Vec::new();
    for tick in 0..600u32 {
        let vm = Vm::new();
        feed.tick_start(&vm, tick * 32);
        feed.pump(&mut world).unwrap();
        for ev in feed.take_events(usize::MAX) {
            match ev {
                FeedEvent::ChallengeSent => {
                    challenges += 1;
                    sends.push(tick * 32);
                }
                FeedEvent::HandshakeTimedOut => timed_out = true,
                _ => panic!("no other event without a server"),
            }
        }
    }
    // TGE's `checkTimeouts`: looked at past every 1,500 ms, sent again past 2,500 ms, five sends before it gives up.
    assert_eq!(challenges, TGE_HANDSHAKE_TRIES + 1);
    assert_eq!(sends, [0, 3008, 6016, 9024, 12032]);
    assert_eq!(sent.borrow().len(), TGE_HANDSHAKE_TRIES as usize + 1);
    assert!(timed_out && feed.is_gone());
}

fn tge_client() -> (Vm, Rc<RefCell<ClientState<TgeConn>>>, DatagramQueue) {
    let mut vm = Vm::new();
    torque_game::specs::install(&mut vm);
    let gui = gui::register(&mut vm);
    let net = Rc::new(RefCell::new(ClientNet::default()));
    let mem = MemoryDatagrams::default();
    let sent = Rc::clone(&mem.sent);
    net.borrow_mut().socket = Some(Box::new(mem));
    let client = Rc::new(RefCell::new(ClientState::<TgeConn> {
        net,
        root: ".".into(),
        ..ClientState::default()
    }));
    crate::conn::register(&mut vm, &client, &gui);
    vm.set_step_limit(Some(50_000_000));
    vm.advance_time(1_000);
    (vm, client, sent)
}

#[test]
fn the_engines_connection_runs_over_torques_half() {
    let (mut vm, client, sent) = tge_client();
    vm.eval(
        b"function GameConnection::onConnectRequestTimedOut(%this) { $TimedOut = 1; } \
          new GameConnection(TgeTest); TgeTest.connect(\"192.168.1.20:28000\");",
    )
    .unwrap();
    assert!(client.borrow().conn.conn.is_some());
    assert_eq!(sent.borrow().len(), 1);
    for _ in 0..600 {
        vm.advance_time(32);
        crate::conn::tick(&mut vm, &client);
    }
    assert_eq!(vm.global("$TimedOut").as_deref(), Some(&b"1"[..]));
    assert!(client.borrow().conn.conn.is_none());
    assert_eq!(sent.borrow().len(), TGE_HANDSHAKE_TRIES as usize + 1);
}

#[test]
fn the_world_reads_the_sky_the_sun_and_the_names() {
    let mut world = TgeWorld::default();
    assert_eq!(
        world.view_distances(),
        (DEFAULT_VIEW_DISTANCE, DEFAULT_VIEW_DISTANCE)
    );
    world.ghosts.insert(
        1,
        ghost(
            TgeClass::Sky,
            TgeRecord::Sky(Sky {
                visibility: Some([F32::new(300.0), F32::new(200.0)]),
                ..Sky::default()
            }),
        ),
    );
    world.ghosts.insert(
        2,
        ghost(
            TgeClass::Sun,
            TgeRecord::Sun(Sun {
                update: Some(SunUpdate {
                    color: [F32::new(0.5), F32::new(0.25), F32::new(1.0), F32::new(1.0)],
                    ..SunUpdate::default()
                }),
            }),
        ),
    );
    assert_eq!(world.view_distances(), (300.0, 200.0));
    world.max_view_distance = Some(100.0);
    assert_eq!(world.view_distances(), (100.0, 100.0));
    assert_eq!(TgeConn::sun_color(&world), Some([0.5, 0.25, 1.0]));
    assert!(world.named_candidates().is_empty());
    assert!(!world.crouched(1));
    world.set_control_state(None);
    assert!(world.container_ghosts().len() == 2);
}

#[test]
fn the_prediction_runs_over_torques_world() {
    let mut world = TgeWorld::default();
    let mut p = crate::predict::Predictor::<TgeWorld>::new(".".into());
    p.update_view(&mut world);
    assert_eq!(p.checksum(), 0);
}

/// A StaticShapeData as a host's view answers it: its shape and its mass.
struct CrateView;

impl torque_server::net::datablock::DatablockView for CrateView {
    fn get(&self, field: &str, _index: usize) -> torque_server::net::datablock::DbValue {
        use torque_server::net::datablock::DbValue;
        match field {
            "shapeFile" => DbValue::Str(b"shapes/crate.dts".to_vec()),
            "mass" => DbValue::F32(4.0),
            _ => DbValue::Null,
        }
    }
}

#[test]
fn a_datablock_event_reads_as_the_server_wrote_it_with_its_script_fields() {
    use torque_server::net::codecs::{NetCodecs, TorqueCodecs};
    use torque_server::net::datablock::{DatablockBody, DatablockEvent};
    let tables = TgeTables::tge().unwrap();
    let widths = NetWidths::engine_default();
    let record = TorqueCodecs
        .datablock_record(
            "StaticShapeData",
            40,
            &CrateView,
            &torque_game::dbvalues::DbValues::default(),
        )
        .expect("TGE's StaticShapeData has a record");
    let event = DatablockEvent {
        body: Some(DatablockBody::new(
            40,
            "StaticShapeData",
            2,
            5,
            widths.datablock_bits,
            record,
        )),
        id_bits: widths.datablock_bits,
    };
    let mut w = net::bits::BitWriter::new();
    w.enable_string_buffer();
    event.pack(&mut w, &tables.table, &tables.classes).unwrap();
    let bytes = w.into_bytes();
    let mut r = BitReader::new(&bytes);
    r.enable_string_buffer();
    let (read, index, total) = read_datablock(&mut r, &tables.ctx(widths), &tables.classes)
        .unwrap()
        .expect("a modified datablock");
    assert_eq!((read.id, index, total), (40, 2, 5));
    assert_eq!(read.datablock.class, "StaticShapeData");
    assert_eq!(
        read.datablock.get("shapeFile", 0),
        Some(&Value::str("shapes/crate.dts"))
    );
    assert_eq!(read.datablock.get("mass", 0), Some(&Value::F32(4.0)));

    // The one-bit event of a datablock the client has: nothing.
    let mut w = net::bits::BitWriter::new();
    DatablockEvent {
        body: None,
        id_bits: widths.datablock_bits,
    }
    .pack(&mut w, &tables.table, &tables.classes)
    .unwrap();
    let bytes = w.into_bytes();
    let mut r = BitReader::new(&bytes);
    assert!(
        read_datablock(&mut r, &tables.ctx(widths), &tables.classes)
            .unwrap()
            .is_none()
    );
}

#[test]
fn the_servers_datablocks_done_is_answered_with_its_sequence() {
    let sent = Rc::new(RefCell::new(VecDeque::new()));
    let mut feed = TgeFeed::new(
        Box::new(Kept(Rc::clone(&sent))),
        b"",
        vec![Vec::new(); 5],
        0,
        Some((12, 14)),
    )
    .expect("TGE's tables");
    let mut world = TgeWorld::default();
    assert_eq!(feed.out.ordered_len(), 0);
    feed.message(
        &mut world,
        ConnectionMessage {
            seq: 3,
            message: DATABLOCKS_DONE,
            count: 0,
        },
    );
    assert_eq!(
        feed.out.ordered_len(),
        1,
        "DataBlocksDownloadDone is queued"
    );
}

#[test]
fn the_packet_rates_follow_the_client_preferences() {
    let mut vm = Vm::new();
    assert_eq!(client_rates(&vm), RateConfig::standard());
    vm.set_global("$pref::Net::PacketRateToClient", "32");
    vm.set_global("$pref::Net::PacketSize", "900");
    let rates = client_rates(&vm);
    assert_eq!(
        (
            rates.to_server_rate,
            rates.to_client_rate,
            rates.packet_size
        ),
        (32, 32, 450),
        "the size clamped as checkMaxRate clamps it"
    );
}

#[test]
fn a_packet_whose_compression_point_is_not_finite_is_refused() {
    let mut r = net::bits::BitReader::new(&[]);
    r.set_compression_point([1.0, 2.0, 3.0]);
    assert_eq!(stream_point(&r), Ok([1.0, 2.0, 3.0]));
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        r.set_compression_point([0.0, bad, 0.0]);
        assert!(stream_point(&r).is_err(), "{bad}");
    }
}

/// A plugin that gives the engine a network version.
#[derive(Clone, Copy, Debug)]
struct RopePlugin(u32);

impl torque_engine::Plugin for RopePlugin {
    fn net_version(&self) -> Option<u32> {
        Some(self.0)
    }

    fn build(&self, _engine: &mut torque_engine::EngineBuilder) {}
}

impl torque_engine::PluginType for RopePlugin {
    const ID: torque_engine::PluginId = torque_engine::PluginId::new("torque.rope");
}

fn rope_engine(version: u32) -> Engine {
    torque_engine::EngineBuilder::new()
        .add_plugins(RopePlugin(version))
        .build()
        .expect("an engine")
}

/// A client whose connections send `engine`'s hello, with the socket's two queues.
fn networked_client(
    engine: &Engine,
) -> (
    Vm,
    Rc<RefCell<ClientState<TgeConn>>>,
    DatagramQueue,
    DatagramQueue,
) {
    let mut vm = Vm::new();
    torque_game::specs::install(&mut vm);
    let gui = gui::register(&mut vm);
    let net = Rc::new(RefCell::new(ClientNet {
        policy: crate::net::NetPolicy::Lan,
        ..ClientNet::default()
    }));
    let mem = MemoryDatagrams::default();
    let (sent, incoming) = (Rc::clone(&mem.sent), Rc::clone(&mem.incoming));
    net.borrow_mut().socket = Some(Box::new(mem));
    let client = Rc::new(RefCell::new(ClientState::<TgeConn> {
        net,
        root: ".".into(),
        ..ClientState::default()
    }));
    crate::conn::register(&mut vm, &client, &gui);
    ConnectHello::install(&mut vm, engine);
    vm.set_step_limit(Some(50_000_000));
    vm.advance_time(1_000);
    (vm, client, sent, incoming)
}

/// Runs a client of `client_engine` against a bare handshake server of `server_engine`; the reason the client was refused with.
fn connect_between(client_engine: &Engine, server_engine: &Engine) -> Option<Vec<u8>> {
    use net::handshake::{Server, ServerConfig};
    use std::net::{Ipv4Addr, SocketAddrV4};
    let (mut vm, client, sent, incoming) = networked_client(client_engine);
    vm.eval(
        b"function GameConnection::onConnectRequestRejected(%this, %why) { $Why = %why; } \
          new GameConnection(TgeTest); TgeTest.connect(\"192.168.1.20:28000\");",
    )
    .unwrap();
    let mut config = ServerConfig::new(8);
    config.gate = server_engine
        .connect_gate(&config.profile)
        .map(|gate| std::sync::Arc::new(gate) as _);
    let mut server = Server::new(config, [3; 16]);
    let table = net::huffman::Table::torque();
    let peer = std::net::SocketAddr::from(([192, 168, 1, 5], 5000));
    let to = SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 20), 28_000);
    for _ in 0..4 {
        let datagrams: Vec<_> = sent.borrow_mut().drain(..).collect();
        for (_, bytes) in datagrams {
            match server.handle_datagram(&table, peer, &bytes, 0) {
                net::handshake::Handled::Reply(reply) => {
                    incoming.borrow_mut().push_back((to, reply));
                }
                net::handshake::Handled::Request(request) => {
                    // The host would run `onConnectRequest` here; this one accepts.
                    if let net::handshake::Finished::Accepted { reply, .. } =
                        server.finish_request(&table, *request, net::handshake::Decision::Accept, 0)
                    {
                        incoming.borrow_mut().push_back((to, reply));
                    }
                }
                _ => {}
            }
        }
        vm.advance_time(32);
        crate::host::ClientPass::net(&crate::host::EnginePass, &mut vm, &client);
        crate::host::run_queued(&mut vm, &client);
    }
    vm.global("$Why")
}

#[test]
fn a_client_shows_the_servers_reason_for_a_manifest_it_refused() {
    // The client has rope 1, the server wants 2: the script hears why, in the server's words.
    let reason = connect_between(&rope_engine(1), &rope_engine(2));
    assert_eq!(
        reason.as_deref(),
        Some(&b"server needs plugin torque.rope 2, you have 1"[..])
    );
    // Matching engines connect: nothing is refused.
    assert_eq!(connect_between(&rope_engine(2), &rope_engine(2)), None);
    // An engine with nothing on the wire sends a plain request, which a plain server accepts, and a gated server refuses.
    let plain = torque_engine::EngineBuilder::new().build().unwrap();
    assert_eq!(connect_between(&plain, &plain), None);
    assert_eq!(
        connect_between(&plain, &rope_engine(2)).as_deref(),
        Some(&b"server needs plugin torque.rope 2, you do not have it"[..])
    );
}
