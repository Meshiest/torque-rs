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
    let (read, index, total) = read_datablock(
        &mut r,
        &tables.ctx(widths),
        &tables.classes,
        &PluginClasses::default(),
        &mut ghosts::free_charge(),
    )
    .unwrap()
    .expect("a modified datablock");
    assert_eq!((read.id, index, total), (40, 2, 5));
    assert_eq!(read.datablock.class, "StaticShapeData");
    assert_eq!(
        read.datablock.get("shapeFile", 0),
        Some(&Value::str("shapes/crate.dts"))
    );
    assert_eq!(read.datablock.get("mass", 0), Some(&Value::F32(4.0)));

    // Datablock events draw on their datagram's budget: what one spends, the next does not have.
    let spent = std::cell::Cell::new(None::<u64>);
    let mut probe = |cost: u64| {
        spent.set(Some(spent.get().unwrap_or(0) + cost));
        true
    };
    let mut r = BitReader::new(&bytes);
    r.enable_string_buffer();
    read_datablock(
        &mut r,
        &tables.ctx(widths),
        &tables.classes,
        &PluginClasses::default(),
        &mut probe,
    )
    .unwrap();
    let one = spent.get().expect("the event paid");
    let budget = std::cell::Cell::new(Some(one + one / 2));
    for (n, want_ok) in [(1, true), (2, false)] {
        let mut r = BitReader::new(&bytes);
        r.enable_string_buffer();
        let read = read_datablock(
            &mut r,
            &tables.ctx(widths),
            &tables.classes,
            &PluginClasses::default(),
            &mut ghosts::cell_charge(&budget),
        );
        assert_eq!(read.is_ok(), want_ok, "event {n} of the datagram");
    }

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
        read_datablock(
            &mut r,
            &tables.ctx(widths),
            &tables.classes,
            &PluginClasses::default(),
            &mut ghosts::free_charge()
        )
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

/// A plugin's datablock class: one struct as the console class (with a preload) and its record.
#[derive(
    Debug, Clone, PartialEq, Default, torque_engine::EngineClass, torque_engine::DatablockCodec,
)]
#[class(name = "PropData", parent = "SimDataBlock", preload = PropData::check)]
struct PropData {
    mass: f32,
    #[net(max_len = 32)]
    model: String,
    #[field(name = "partCount")]
    parts: i32,
}

impl PropData {
    /// The preload: refuses a prop with no model, and notes the ones it took.
    fn check(&self, vm: &mut Vm, id: ts_vm::sim::ObjId) -> Result<(), String> {
        if self.model.is_empty() {
            return Err("a prop needs a model".to_string());
        }
        let seen = vm.global("$preloaded").unwrap_or_default();
        let seen = String::from_utf8_lossy(&seen);
        vm.set_global(
            "$preloaded",
            &format!("{seen}{id}:{}:{} ", self.model, self.parts),
        );
        Ok(())
    }
}

#[derive(torque_engine::PluginId)]
#[plugin(id = "test.props")]
struct PropsPlugin;

impl torque_engine::Plugin for PropsPlugin {
    fn build(&self, engine: &mut torque_engine::EngineBuilder) {
        engine.script().class::<PropData>();
        engine.net().datablock::<PropData>("PropData");
    }
}

/// A PropData as the server's view answers it.
struct PropView(&'static str);

impl torque_server::net::datablock::DatablockView for PropView {
    fn get(&self, field: &str, _index: usize) -> torque_server::net::datablock::DbValue {
        use torque_server::net::datablock::DbValue;
        match field {
            "mass" => DbValue::F32(12.5),
            "model" => DbValue::Str(self.0.as_bytes().to_vec()),
            "partCount" => DbValue::Int(4),
            _ => DbValue::Null,
        }
    }
}

#[test]
fn a_plugins_datablock_goes_out_past_tges_class_ids_and_the_client_makes_and_preloads_it() {
    use torque_engine::{ClientStage, ServerStage};
    use torque_server::net::codecs::net_codecs;
    use torque_server::net::datablock::{DatablockBody, DatablockEvent};
    let engine = torque_engine::EngineBuilder::new()
        .add_plugins(PropsPlugin)
        .build()
        .expect("the engine builds");
    // The server: its codecs are the engine's own, with the plugin's class.
    let mut server = Vm::new();
    for &stage in ServerStage::ALL {
        engine.replay(stage, &mut server).unwrap();
    }
    let codecs = net_codecs(&server);
    assert_eq!(codecs.datablock_class("PropData"), Some("PropData"));
    let tables = TgeTables::tge().unwrap();
    let widths = NetWidths::engine_default();
    let event = |id: u32, model: &'static str| {
        let record = codecs
            .datablock_record(
                "PropData",
                id,
                &PropView(model),
                &torque_game::dbvalues::DbValues::default(),
            )
            .expect("the plugin's class has a record");
        // The class id past the TGE table's 35.
        assert_eq!(
            record
                .as_any()
                .downcast_ref::<torque_server::net::codecs::PluginDbRecord>()
                .map(|r| r.class_id),
            Some(35)
        );
        let event = DatablockEvent {
            body: Some(DatablockBody::new(
                id,
                "PropData",
                0,
                2,
                widths.datablock_bits,
                record,
            )),
            id_bits: widths.datablock_bits,
        };
        let mut w = net::bits::BitWriter::new();
        w.enable_string_buffer();
        event.pack(&mut w, &tables.table, &tables.classes).unwrap();
        w.into_bytes()
    };
    // The client: the same plugins.
    let mut client = Vm::new();
    for &stage in ClientStage::ALL {
        engine.replay(stage, &mut client).unwrap();
    }
    let plugins = torque_server::net::codecs::plugin_classes(&client).expect("the client's set");
    for (id, model) in [(40, "props/crate.glb"), (41, "")] {
        let bytes = event(id, model);
        let mut r = BitReader::new(&bytes);
        r.enable_string_buffer();
        let (read, _, _) = read_datablock(
            &mut r,
            &tables.ctx(widths),
            &tables.classes,
            &plugins,
            &mut ghosts::free_charge(),
        )
        .unwrap()
        .expect("a modified datablock");
        assert_eq!(read.datablock.class, "PropData");
        assert_eq!(read.datablock.get("partCount", 0), Some(&Value::Int(4)));
        let unpacked = unpack_datablock(FieldRules::default(), &read);
        let received = crate::datablocks::receive(&mut client, &unpacked, None).unwrap();
        if model.is_empty() {
            assert_eq!(
                received,
                crate::datablocks::Received::Refused,
                "the preload refused it"
            );
        } else {
            assert_eq!(received, crate::datablocks::Received::Added(40));
        }
    }
    assert_eq!(
        client.global("$preloaded").as_deref(),
        Some(&b"40:props/crate.glb:4 "[..])
    );
    assert_eq!(client.eval(b"return 40.mass;").unwrap(), b"12.5");
    assert_eq!(
        client.eval(b"return 40.model;").unwrap(),
        b"props/crate.glb"
    );

    // A client without the plugin refuses the class id cleanly.
    let bytes = event(42, "props/barrel.glb");
    let mut r = BitReader::new(&bytes);
    r.enable_string_buffer();
    assert_eq!(
        read_datablock(
            &mut r,
            &tables.ctx(widths),
            &tables.classes,
            &PluginClasses::default(),
            &mut ghosts::free_charge()
        )
        .unwrap_err(),
        NetError::Invalid("datablock class id")
    );
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

/// A plugin whose engine signs in as `name` and checks sign-ins, under `key`.
#[derive(Clone, Copy, Debug)]
struct SignInPlugin([u8; 16], &'static str);

impl torque_engine::Plugin for SignInPlugin {
    fn build(&self, engine: &mut torque_engine::EngineBuilder) {
        engine
            .net()
            .auth(net::connect::SharedSecret::new(self.0, self.1));
    }
}

impl torque_engine::PluginType for SignInPlugin {
    const ID: torque_engine::PluginId = torque_engine::PluginId::new("torque.sign-in");
}

fn sign_in_engine(key: [u8; 16], name: &'static str) -> Engine {
    torque_engine::EngineBuilder::new()
        .add_plugins(SignInPlugin(key, name))
        .build()
        .expect("an engine")
}

/// The address the test client dials.
fn dialed() -> std::net::SocketAddr {
    std::net::SocketAddr::from(([192, 168, 1, 20], 28_000))
}

/// Runs a client of `client_engine` against a bare handshake server of `server_engine`; the reason the client was refused with.
fn connect_between(client_engine: &Engine, server_engine: &Engine) -> Option<Vec<u8>> {
    connect_through(client_engine, server_engine, false).0
}

/// [`connect_between`] with a server that says it is at `address` (the client dials [`dialed`]).
fn connect_between_at(
    client_engine: &Engine,
    server_engine: &Engine,
    address: std::net::SocketAddr,
) -> Option<Vec<u8>> {
    connect_with(client_engine, server_engine, false, address).0
}

/// [`connect_between`] that can make the server's gate say a client's channels match even when it listed none (a gate that
/// lies, or an old one): the reason the client was refused with, and the lines its console printed.
fn connect_through(
    client_engine: &Engine,
    server_engine: &Engine,
    gate_says_channels: bool,
) -> (Option<Vec<u8>>, Vec<String>) {
    connect_with(client_engine, server_engine, gate_says_channels, dialed())
}

fn connect_with(
    client_engine: &Engine,
    server_engine: &Engine,
    gate_says_channels: bool,
    address: std::net::SocketAddr,
) -> (Option<Vec<u8>>, Vec<String>) {
    use net::handshake::{Server, ServerConfig};
    use std::net::{Ipv4Addr, SocketAddrV4};
    let (mut vm, client, sent, incoming) = networked_client(client_engine);
    vm.eval(
        b"function GameConnection::onConnectRequestRejected(%this, %why) { $Why = %why; } \
          new GameConnection(TgeTest); TgeTest.connect(\"192.168.1.20:28000\");",
    )
    .unwrap();
    let mut config = ServerConfig::new(8);
    config.address = Some(address);
    config.channels = gate_says_channels;
    config.gate = server_engine.connect_gate(&config.profile).map(|gate| {
        if gate_says_channels {
            std::sync::Arc::new(AlwaysChannels(gate)) as _
        } else {
            std::sync::Arc::new(gate) as _
        }
    });
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
    let lines = vm
        .log()
        .iter()
        .map(|l| String::from_utf8_lossy(&l.text).into_owned())
        .collect();
    (vm.global("$Why"), lines)
}

/// A gate that accepts what the engine's does and says the channels match whatever the client listed.
#[derive(Debug)]
struct AlwaysChannels(net::connect::ManifestGate);

impl net::connect::ConnectGate for AlwaysChannels {
    fn check(
        &self,
        from: std::net::SocketAddr,
        cx: &net::connect::AuthContext,
        fields: &net::handshake::ConnectFields,
    ) -> Result<Option<net::connect::Identity>, Vec<u8>> {
        self.0.check(from, cx, fields)
    }

    fn matches_channels(&self, _: &net::handshake::ConnectFields) -> bool {
        true
    }
}

#[test]
fn the_channels_word_sent_to_a_client_without_channels_is_ignored() {
    // The server's accept ends with the word; this client has no channels, and connects as it always did.
    let engine = rope_engine(2);
    let (why, lines) = connect_through(&engine, &engine, true);
    assert_eq!(why, None);
    assert!(
        lines.iter().any(|l| l == "Connection established"),
        "{lines:?}"
    );
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

#[test]
fn a_client_signs_in_for_the_server_it_dialed_and_the_challenge_it_was_given() {
    let key = [7; 16];
    let server = sign_in_engine(key, "");
    // The same secret, at the address the client dialed: accepted.
    assert_eq!(connect_between(&sign_in_engine(key, "ada"), &server), None);
    // A server that believes it is somewhere else is not the server the ticket was made for.
    let elsewhere = std::net::SocketAddr::from(([192, 168, 1, 21], 28_000));
    assert_eq!(
        connect_between_at(&sign_in_engine(key, "ada"), &server, elsewhere).as_deref(),
        Some(&b"your sign-in was not accepted"[..])
    );
    // Another secret is refused, and a client that is not signed in stops with its own reason.
    assert_eq!(
        connect_between(&sign_in_engine([8; 16], "ada"), &server).as_deref(),
        Some(&b"your sign-in was not accepted"[..])
    );
    assert_eq!(
        connect_between(&sign_in_engine(key, ""), &server).as_deref(),
        Some(&b"not signed in"[..])
    );
}

#[test]
fn a_mission_ending_keeps_the_plugin_classes_the_world_draws_as_shapes() {
    use ghosts::codec::{Codec, Layout, R, Xf};
    use ghosts::plugin::{
        DatablockRef, DrawnShapes, FieldSpec, GhostRecord, NetField, NetRecord, PluginClass,
        Replicate, ShapeDraw, ShapeFn,
    };
    use ghosts::{ObjectView, PackCtx};

    #[derive(Debug, Clone, PartialEq, Default)]
    struct Marker {
        pose: Xf,
        data: DatablockRef,
    }

    impl Layout for Marker {
        fn layout<C: Codec>(&mut self, c: &mut C) -> R {
            self.pose.layout(c, "transform", FieldSpec::DEFAULT)?;
            self.data.layout(c, "dataBlock", FieldSpec::DEFAULT)
        }
    }

    impl NetRecord for Marker {
        fn merge(&mut self, sent: &Marker) {
            *self = sent.clone();
        }
        fn datablock(&self) -> Option<u32> {
            self.data.datablock()
        }
        fn transform(&self) -> Option<Xf> {
            Some(self.pose)
        }
        fn script_fields(&self) -> Vec<(String, u32, Value)> {
            Vec::new()
        }
    }

    impl GhostRecord for Marker {
        fn pack(_: &dyn ObjectView, _: u32, _: &PackCtx) -> (Marker, u32) {
            (Marker::default(), 0)
        }
    }

    let class = PluginClass::ghost::<Marker>("Marker", Replicate::Interpolated);
    let mut world = ClientWorld::<TgeClientSet>::default();
    world.plugin_shapes.insert(
        class,
        ShapeFn::new::<Marker>(|_, _| Some(ShapeDraw::new("a.dts"))),
    );
    world.ghosts.insert(
        1,
        ghost(
            TgeClass::Plugin(class),
            TgeRecord::Plugin(class, Box::new(Marker::default())),
        ),
    );
    let kept = world.plugin_shapes.clone();
    end_ghosting(&mut world);
    assert!(world.ghosts.is_empty());
    assert_eq!(world.plugin_shapes, kept);
    assert_ne!(world.plugin_shapes, DrawnShapes::default());
}

#[test]
fn the_datablock_cap_is_the_pref_under_the_wire_range_and_the_modes_default_when_unset() {
    let mut vm = Vm::new();
    let widths = NetWidths::new(10, DEFAULT_DATABLOCK_BITS);
    let range = 1usize << DEFAULT_DATABLOCK_BITS;
    assert_eq!(
        max_datablocks(&vm, false, widths),
        DEFAULT_MAX_DATABLOCKS_TGE.min(range)
    );
    assert_eq!(
        max_datablocks(&vm, true, widths),
        DEFAULT_MAX_DATABLOCKS_ENGINE.min(range)
    );
    vm.set_global(MAX_DATABLOCKS_PREF, "64");
    assert_eq!(max_datablocks(&vm, false, widths), 64);
    assert_eq!(max_datablocks(&vm, true, widths), 64);
    // Over the wire's range, zero, negative and junk never widen or empty the table.
    for (text, want) in [
        ("1000000", range),
        ("0", 1),
        ("-5", 1),
        ("lots", 1),
        ("", DEFAULT_MAX_DATABLOCKS_TGE.min(range)),
    ] {
        vm.set_global(MAX_DATABLOCKS_PREF, text);
        assert_eq!(max_datablocks(&vm, false, widths), want, "{text:?}");
    }
}

#[test]
fn the_live_bucket_allows_a_burst_then_the_rate_and_two_seconds_of_refusals_are_a_flood() {
    let mut bucket = LiveBucket::new(1_000);
    let burst = LIVE_DATABLOCK_BURST as usize;
    assert_eq!((0..burst).filter(|_| bucket.take(1_000)).count(), burst);
    assert!(!bucket.take(1_000), "the burst is spent");
    assert!(!bucket.flooded(1_000));
    // A second later a second's worth is back, and the refusals are forgotten.
    assert_eq!(
        (0..burst).filter(|_| bucket.take(2_000)).count(),
        LIVE_DATABLOCKS_PER_S as usize
    );
    assert!(!bucket.flooded(2_000));
    // A sender that keeps going past the rate: a token every 1000 / 256 ms, never a second's worth, and after two seconds a flood.
    let mut now = 2_000;
    let mut taken = 0;
    while now < 4_000 {
        now += 4;
        if bucket.take(now) {
            taken += 1;
        }
        assert!(
            !bucket.flooded(now - 1) || now >= 4_000,
            "not before two seconds"
        );
    }
    assert!(bucket.flooded(now));
    assert!(
        taken <= 2 * LIVE_DATABLOCKS_PER_S as usize + 1,
        "{taken} in two seconds"
    );
}

#[test]
fn the_live_bucket_forgets_the_refusals_once_it_fills_back() {
    let mut bucket = LiveBucket::new(0);
    for _ in 0..LIVE_DATABLOCK_BURST {
        assert!(bucket.take(0));
    }
    assert!(!bucket.take(0));
    assert!(bucket.take(1_000), "a second's refill");
    assert!(!bucket.flooded(2_000), "the sender backed off: no flood");
}
