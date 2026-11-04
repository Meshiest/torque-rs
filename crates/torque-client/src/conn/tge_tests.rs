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
