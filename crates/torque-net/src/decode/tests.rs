use super::*;
use crate::client::C2sHead;
use crate::events::{EventQueue, NetEvent, SectionParams, WireEvent, put_ordered, put_unordered};
use crate::netstring::NetStringEvent;
use crate::protocol::{Protocol, RateConfig};
use crate::remote::RemoteCommandEvent;
use crate::testids::{ids, table, with_ctx};

fn server() -> Protocol {
    Protocol::new(Role::Server, 1, RateConfig::standard(), 0)
}

fn client() -> Protocol {
    Protocol::new(Role::Client, 1, RateConfig::standard(), 0)
}

fn netstring(slot: u8, text: &str) -> NetEvent {
    NetEvent::NetString(NetStringEvent {
        slot,
        text: text.as_bytes().to_vec(),
        tag: 0,
    })
}

fn command(args: Vec<RemoteArg>) -> NetEvent {
    NetEvent::Remote(RemoteCommandEvent { variant: 0, args })
}

/// An event of the boundary class (a test class) with an arbitrary body: the decoder stops at its class id.
struct Boundary;

impl WireEvent for Boundary {
    fn class(&self, _: &EventIds) -> u32 {
        crate::testids::ANY
    }
    fn pack(&self, w: &mut BitWriter, _: &EventCtx<'_>) {
        w.write_bits(0xdead_beef, 32);
        w.write_bits(0xdead_beef, 32);
    }
}

/// A server data packet: the head of an idle server, the events the queue holds, the ghost flag.
fn server_packet(
    p: &mut Protocol,
    now: u32,
    q: &mut EventQueue<NetEvent>,
    extra_ghost_bits: u8,
) -> Vec<u8> {
    with_ctx(|ctx| {
        let mut w = p.begin_data_packet(now);
        w.enable_string_buffer();
        w.write_u32(0);
        for _ in 0..5 {
            w.write_flag(false);
        }
        q.write_section(
            &mut w,
            ctx,
            p.last_send_seq(),
            SectionParams {
                ghosting: false,
                hold_ordered: false,
                packet_bits: p.packet_limit_bits(),
                min_start_bits: 0,
            },
        )
        .expect("fits");
        w.write_flag(false);
        w.write_bits(0, extra_ghost_bits);
        w.into_bytes()
    })
}

fn client_packet(p: &mut Protocol, now: u32, moves: u8, q: &mut EventQueue<NetEvent>) -> Vec<u8> {
    with_ctx(|ctx| {
        let mut w = p.begin_data_packet(now);
        w.enable_string_buffer();
        C2sHead {
            camera_flag: false,
            control_checksum: 0,
            move_start: 0,
            move_count: moves,
        }
        .write(&mut w);
        for _ in 0..moves {
            // yaw present, pitch and roll absent, three axes, free look, six triggers
            w.write_flag(true);
            w.write_bits(0x1234, 16);
            w.write_flag(false);
            w.write_flag(false);
            w.write_bits(16, 6);
            w.write_bits(16, 6);
            w.write_bits(16, 6);
            for _ in 0..7 {
                w.write_flag(false);
            }
        }
        // No first-person update, then the fov.
        w.write_flag(false);
        w.write_flag(true);
        w.write_bits(90, 8);
        q.write_section(
            &mut w,
            ctx,
            p.last_send_seq(),
            SectionParams {
                ghosting: false,
                hold_ordered: false,
                packet_bits: p.packet_limit_bits(),
                min_start_bits: 0,
            },
        )
        .expect("fits");
        w.into_bytes()
    })
}

fn run(datagrams: &[(Dir, Vec<u8>)]) -> Report {
    let t = table();
    let ids = ids();
    let list: Vec<Datagram<'_>> = datagrams
        .iter()
        .enumerate()
        .map(|(i, (dir, bytes))| Datagram {
            dir: *dir,
            ms: i as u32 * 32,
            bytes,
        })
        .collect();
    let handshake = crate::profile::HandshakeProfile::default();
    let mut profile = DecodeProfile {
        handshake: &handshake,
        control: crate::control::ControlFormat::default(),
        reader: &mut crate::control::CameraReader,
        boundary: Some(crate::testids::ANY),
        datablock_bits: crate::profile::DEFAULT_DATABLOCK_BITS,
    };
    decode_stream(&list, &t, &ids, 18, &mut profile)
}

fn post_all(q: &mut EventQueue<NetEvent>, events: Vec<NetEvent>) {
    for e in events {
        q.post(e).expect("room");
    }
}

#[test]
fn a_list_decodes_up_to_the_first_boundary_event() {
    let mut sp = server();
    let mut cp = client();
    let mut sq = EventQueue::new();
    let mut cq = EventQueue::new();
    let mut datagrams = Vec::new();
    // The client speaks first: moves, a command with a new tag.
    post_all(
        &mut cq,
        vec![
            netstring(0, "RequestPing"),
            command(vec![RemoteArg::Tag(0), RemoteArg::Int(5)]),
        ],
    );
    datagrams.push((Dir::ToServer, client_packet(&mut cp, 0, 3, &mut cq)));
    // The server answers with an empty packet, then tagged strings and commands, a connection message and, last, the boundary event.
    datagrams.push((Dir::ToClient, server_packet(&mut sp, 32, &mut sq, 0)));
    post_all(
        &mut sq,
        vec![
            netstring(0, "ServerMessage"),
            netstring(1, "MsgHello"),
            command(vec![
                RemoteArg::Tag(0),
                RemoteArg::Tag(1),
                RemoteArg::Text(b"x".to_vec()),
            ]),
            NetEvent::ConnectionMessage(crate::events::ConnectionMessage {
                seq: 0,
                message: 4,
                count: 7,
            }),
        ],
    );
    datagrams.push((Dir::ToClient, server_packet(&mut sp, 64, &mut sq, 0)));
    // Handshake datagrams and acks are counted and skipped.
    datagrams.push((
        Dir::ToServer,
        crate::handshake::HandshakeTypes::TORQUE.challenge_request(1),
    ));
    datagrams.push((Dir::ToServer, cp.ack_packet(70)));
    // The boundary event, after one more event in the same datagram.
    let with_boundary = with_ctx(|ctx| {
        let mut w = sp.begin_data_packet(97);
        w.enable_string_buffer();
        w.write_u32(0);
        for _ in 0..5 {
            w.write_flag(false);
        }
        w.write_flag(false);
        let mut prev = 0xffff_fffeu32;
        put_ordered(&mut w, ctx, &mut prev, 4, &netstring(2, "Third"));
        put_ordered(&mut w, ctx, &mut prev, 5, &Boundary);
        w.write_flag(false);
        w.into_bytes()
    });
    datagrams.push((Dir::ToClient, with_boundary));
    datagrams.push((Dir::ToClient, server_packet(&mut sp, 128, &mut sq, 0)));

    let report = run(&datagrams);
    assert_eq!(report.records, 7);
    let b = report.boundary.expect("a boundary");
    assert_eq!((b.record, b.dir, b.events_before), (5, Dir::ToClient, 1));
    assert_eq!(report.errors(), 0, "{:?}", report.dirs);
    assert_eq!(report.failures(), 0);
    let to_server = &report.dirs[Dir::ToServer.index()];
    let to_client = &report.dirs[Dir::ToClient.index()];
    assert_eq!(
        (
            to_server.datagrams,
            to_server.data_packets,
            to_server.ack_packets,
            to_server.handshake_packets
        ),
        (3, 1, 1, 1)
    );
    assert_eq!((to_client.datagrams, to_client.data_packets), (2, 2));
    assert_eq!(to_server.events.get(&ids().net_string), Some(&1));
    assert_eq!(
        to_client.events.get(&ids().net_string),
        Some(&3),
        "two before, one in the boundary datagram"
    );
    assert_eq!(to_client.events.get(&ids().remote_command), Some(&1));
    assert_eq!(to_client.events.get(&ids().connection_message), Some(&1));
    assert_eq!(report.connection_messages, vec![(Dir::ToClient, 4, 0, 7)]);
    assert!(to_server.leftover_max < 8 && to_client.leftover_max < 8);
    assert_eq!((to_server.reencoded, to_server.reencode_mismatches), (1, 0));
    assert_eq!((to_client.reencoded, to_client.reencode_mismatches), (2, 0));
    assert_eq!(report.netstrings[Dir::ToClient.index()].len(), 3);
    let replay = &report.replay[Dir::ToClient.index()];
    assert_eq!(
        (
            replay.commands,
            replay.tag_args,
            replay.netstrings,
            replay.unused_netstrings
        ),
        (1, 2, 3, 1)
    );
    assert!(report.max_server_bytes_before_client == 0);
}

#[test]
fn the_boundary_datagrams_earlier_events_are_counted() {
    let mut sp = server();
    let datagram = with_ctx(|ctx| {
        let mut w = sp.begin_data_packet(0);
        w.enable_string_buffer();
        w.write_u32(0);
        for _ in 0..5 {
            w.write_flag(false);
        }
        w.write_flag(false);
        let mut prev = 0xffff_fffeu32;
        put_ordered(&mut w, ctx, &mut prev, 0, &netstring(0, "A"));
        put_ordered(&mut w, ctx, &mut prev, 1, &netstring(1, "B"));
        put_ordered(&mut w, ctx, &mut prev, 2, &Boundary);
        w.write_flag(false);
        w.into_bytes()
    });
    let report = run(&[(Dir::ToClient, datagram)]);
    let b = report.boundary.expect("boundary");
    assert_eq!((b.record, b.events_before), (0, 2));
    // The two events before it ran, in the datagram that holds the boundary.
    assert_eq!(report.netstrings[Dir::ToClient.index()].len(), 2);
    assert_eq!(report.dirs[Dir::ToClient.index()].datagrams, 0);
}

#[test]
fn a_list_without_a_boundary_event_has_no_boundary() {
    let mut sp = server();
    let mut sq = EventQueue::new();
    let report = run(&[(Dir::ToClient, server_packet(&mut sp, 0, &mut sq, 0))]);
    assert_eq!(report.boundary, None);
    assert_eq!(report.failures(), 0);
}

#[test]
fn a_command_that_names_a_different_slot_than_the_lru_would_is_a_difference() {
    let mut sp = server();
    let mut sq = EventQueue::new();
    // The first tag the LRU sees takes slot 0; the wire says slot 5.
    post_all(
        &mut sq,
        vec![netstring(5, "Cmd"), command(vec![RemoteArg::Tag(5)])],
    );
    let report = run(&[(Dir::ToClient, server_packet(&mut sp, 0, &mut sq, 0))]);
    let r = &report.replay[Dir::ToClient.index()];
    assert_eq!(r.slot_mismatches.len(), 1, "{r:?}");
    assert!(report.failures() > 0);
}

#[test]
fn a_net_string_the_lru_would_not_post_is_a_difference() {
    let mut sp = server();
    let mut sq = EventQueue::new();
    // Two commands: the second names the same tag, which is a hit, but the wire posts a NetString again for slot 1.
    post_all(
        &mut sq,
        vec![
            netstring(0, "Cmd"),
            command(vec![RemoteArg::Tag(0)]),
            netstring(1, "Cmd"),
            command(vec![RemoteArg::Tag(1)]),
        ],
    );
    let report = run(&[(Dir::ToClient, server_packet(&mut sp, 0, &mut sq, 0))]);
    let r = &report.replay[Dir::ToClient.index()];
    assert_eq!(r.slot_mismatches.len(), 1);
    assert_eq!(r.unused_netstrings, 1);
    assert!(report.failures() > 0);
}

#[test]
fn a_tag_argument_naming_an_unmapped_slot_is_a_difference() {
    let mut sp = server();
    let mut sq = EventQueue::new();
    post_all(&mut sq, vec![command(vec![RemoteArg::Tag(9)])]);
    let report = run(&[(Dir::ToClient, server_packet(&mut sp, 0, &mut sq, 0))]);
    assert_eq!(report.replay[Dir::ToClient.index()].unmapped.len(), 1);
    assert!(report.failures() > 0);
}

#[test]
fn leftover_bits_of_8_or_more_are_a_failure() {
    let mut sp = server();
    let mut sq = EventQueue::new();
    let clean = run(&[(Dir::ToClient, server_packet(&mut sp, 0, &mut sq, 0))]);
    let d = &clean.dirs[Dir::ToClient.index()];
    assert!(d.leftover_max < 8 && d.leftover_hist[LEFTOVER_LIMIT] == 0);
    assert_eq!(clean.failures(), 0);
    // 16 stray bits after the ghost flag: at least 16 are left, however the last byte is padded.
    let dirty = run(&[(Dir::ToClient, server_packet(&mut sp, 32, &mut sq, 16))]);
    let d = &dirty.dirs[Dir::ToClient.index()];
    assert_eq!((d.leftover_hist[LEFTOVER_LIMIT], dirty.failures()), (1, 1));
    assert!(d.leftover_max >= 8);
}

#[test]
fn an_event_whose_re_encoding_differs_is_a_difference() {
    // A NetString whose text is sent raw although the Huffman form is shorter: it decodes, but this writer would write it compressed.
    let text = "eeeeeeeeeeeeeeeeeeee";
    assert!(table().coded_bits(text.as_bytes()) < text.len() * 8);
    let mut sp = server();
    let datagram = with_ctx(|ctx| {
        let mut w = sp.begin_data_packet(0);
        w.enable_string_buffer();
        w.write_u32(0);
        for _ in 0..5 {
            w.write_flag(false);
        }
        put_unordered_raw(&mut w, ctx, text);
        w.write_flag(false);
        w.write_flag(false);
        w.write_flag(false);
        w.into_bytes()
    });
    let report = run(&[(Dir::ToClient, datagram)]);
    let d = &report.dirs[Dir::ToClient.index()];
    assert_eq!(d.errors.len(), 0, "{:?}", d.errors);
    assert_eq!((d.reencoded, d.reencode_mismatches), (1, 1));
    assert!(report.failures() > 0);
}

fn put_unordered_raw(w: &mut BitWriter, ctx: &EventCtx<'_>, text: &str) {
    w.write_flag(true);
    w.write_bits(ctx.ids.net_string, 5);
    w.write_bits(3, 5);
    w.write_flag(false); // no shared prefix
    w.write_flag(false); // not compressed
    w.write_bits(text.len() as u32, 8);
    w.write_bytes(text.as_bytes());
}

#[test]
fn a_class_of_the_wrong_direction_and_a_truncated_packet_are_errors_not_panics() {
    let mut sp = server();
    let wrong = with_ctx(|ctx| {
        let mut w = sp.begin_data_packet(0);
        w.enable_string_buffer();
        w.write_u32(0);
        for _ in 0..5 {
            w.write_flag(false);
        }
        // A client-to-server class in a server packet.
        w.write_flag(true);
        w.write_bits(crate::testids::TO_SERVER, 5);
        w.write_bits(0, 30);
        w.write_flag(false);
        w.write_flag(false);
        let _ = ctx;
        w.into_bytes()
    });
    let mut sq = EventQueue::new();
    post_all(&mut sq, vec![netstring(0, "Some longer text here")]);
    let good = server_packet(&mut sp, 32, &mut sq, 0);
    let mut datagrams = vec![
        (Dir::ToClient, wrong),
        (Dir::ToClient, good[..good.len() - 3].to_vec()),
    ];
    datagrams.push((Dir::ToServer, vec![0x01]));
    datagrams.push((Dir::ToServer, Vec::new()));
    let report = run(&datagrams);
    assert!(report.errors() >= 3, "{:?}", report.dirs);
    assert!(report.failures() >= 3);
}

#[test]
fn the_c2s_head_skips_the_moves_and_the_fov() {
    let mut cp = client();
    let mut cq = EventQueue::new();
    post_all(&mut cq, vec![netstring(0, "Ping")]);
    let with_moves = client_packet(&mut cp, 0, 5, &mut cq);
    let report = run(&[(Dir::ToServer, with_moves)]);
    assert_eq!(report.errors(), 0, "{:?}", report.dirs[0].errors);
    assert_eq!(
        report.dirs[Dir::ToServer.index()]
            .events
            .get(&ids().net_string),
        Some(&1)
    );
    assert!(report.dirs[0].leftover_max < 8);
}

#[test]
fn the_longest_server_datagram_before_the_clients_first_data_packet_is_reported() {
    let mut sp = server();
    let mut cp = client();
    let mut sq = EventQueue::new();
    post_all(
        &mut sq,
        vec![netstring(
            0,
            "A long string to make the packet bigger than the empty one",
        )],
    );
    let a = server_packet(&mut sp, 0, &mut sq, 0);
    let b = server_packet(&mut sp, 32, &mut sq, 0);
    let alen = a.len();
    let blen = b.len();
    let c = client_packet(&mut cp, 40, 0, &mut EventQueue::new());
    let late = server_packet(&mut sp, 64, &mut sq, 0);
    let report = run(&[
        (Dir::ToClient, a),
        (Dir::ToClient, b),
        (Dir::ToServer, c),
        (Dir::ToClient, late),
    ]);
    assert_eq!(report.max_server_bytes_before_client, alen.max(blen));
}

#[test]
fn byte_one_string_arguments_are_counted() {
    let mut sp = server();
    let mut sq = EventQueue::new();
    post_all(
        &mut sq,
        vec![
            netstring(0, "Cmd"),
            command(vec![
                RemoteArg::Tag(0),
                RemoteArg::Text(b"\x0150".to_vec()),
                RemoteArg::Text(b"plain".to_vec()),
            ]),
        ],
    );
    let report = run(&[(Dir::ToClient, server_packet(&mut sp, 0, &mut sq, 0))]);
    assert_eq!(report.dirs[Dir::ToClient.index()].plain_byte1_args, 1);
    let _ = put_unordered::<NetEvent>;
}

#[test]
fn a_set_ghost_flag_at_the_end_of_a_server_packet_is_read_and_counted() {
    let mut sp = server();
    let datagram = with_ctx(|ctx| {
        let mut w = sp.begin_data_packet(0);
        w.enable_string_buffer();
        w.write_u32(0);
        for _ in 0..5 {
            w.write_flag(false);
        }
        EventQueue::<NetEvent>::new()
            .write_section(
                &mut w,
                ctx,
                1,
                SectionParams {
                    ghosting: false,
                    hold_ordered: false,
                    packet_bits: 1600,
                    min_start_bits: 0,
                },
            )
            .expect("ok");
        // Ghosting has begun: the ghost part is a set flag (its data come later, so one bit is all this packet holds).
        w.write_flag(true);
        w.into_bytes()
    });
    let clean = server_packet(&mut sp, 32, &mut EventQueue::new(), 0);
    let report = run(&[(Dir::ToClient, datagram), (Dir::ToClient, clean)]);
    let d = &report.dirs[Dir::ToClient.index()];
    assert_eq!(d.ghost_flags_set, 1, "only the first packet's flag is set");
    assert_eq!(report.failures(), 0, "{d:?}");
}

#[test]
fn an_ack_or_ping_packet_with_8_or_more_bits_after_its_header_is_an_error() {
    let mut cp = client();
    let mut ack = cp.ack_packet(0);
    let ping = cp.ping_packet(1);
    let report = run(&[(Dir::ToServer, ack.clone()), (Dir::ToServer, ping.clone())]);
    assert_eq!(report.failures(), 0);
    assert!(report.dirs[0].non_data_leftover_max < 8);
    ack.extend_from_slice(&[0, 0]);
    let mut long_ping = ping;
    long_ping.push(0);
    let report = run(&[(Dir::ToServer, ack), (Dir::ToServer, long_ping)]);
    assert_eq!(report.dirs[0].errors.len(), 2);
    assert!(report.dirs[0].non_data_leftover_max >= 8);
}
