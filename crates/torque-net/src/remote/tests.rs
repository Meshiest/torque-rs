use super::*;
use crate::cap::{NET_EVENT_QUEUE_ORDERED, NET_REMOTE_SLOTS};
use crate::events::{EventQueue, NetEvent, SectionParams};
use crate::testids::with_ctx;

/// A stand-in for the VM's tag table.
struct Tags(Vec<Vec<u8>>);

impl Tags {
    fn new(texts: &[&str]) -> Tags {
        Tags(texts.iter().map(|t| t.as_bytes().to_vec()).collect())
    }
}

impl TagLookup for Tags {
    fn tag_text(&self, id: u32) -> Option<Vec<u8>> {
        usize::try_from(id)
            .ok()
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| self.0.get(i).cloned())
    }

    fn table_size(&self) -> u32 {
        self.0.len() as u32
    }
}

fn tag_arg(id: u32) -> Vec<u8> {
    let mut v = vec![1u8];
    v.extend_from_slice(id.to_string().as_bytes());
    v
}

fn pack_unpack(e: &RemoteCommandEvent) -> (Result<RemoteCommandEvent, NetError>, usize) {
    with_ctx(|ctx| {
        let mut w = BitWriter::new();
        w.enable_string_buffer();
        e.pack(&mut w, ctx);
        let bits = w.bits();
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        r.enable_string_buffer();
        (RemoteCommandEvent::unpack(&mut r, ctx), bits)
    })
}

#[test]
fn atoi_is_strtol_on_32_bits() {
    let cases: [(&[u8], i32); 14] = [
        (b"", 0),
        (b"12", 12),
        (b"-12", -12),
        (b"+7", 7),
        (b"  42abc", 42),
        (b"abc", 0),
        (b"-", 0),
        (b"2147483647", i32::MAX),
        (b"2147483648", i32::MAX),
        (b"99999999999999999999", i32::MAX),
        (b"-2147483648", i32::MIN),
        (b"-99999999999999999999", i32::MIN),
        (b"007", 7),
        (b"\t\n 5", 5),
    ];
    for (s, want) in cases {
        assert_eq!(atoi(s), want, "{:?}", String::from_utf8_lossy(s));
    }
}

#[test]
fn string_arguments_are_classified_as_the_packer_does() {
    use RemoteArg::*;
    let text = |s: &str| Text(s.as_bytes().to_vec());
    assert_eq!(RemoteArg::classify(b""), Empty);
    assert_eq!(RemoteArg::classify(b"\x0123"), Tag(23));
    assert_eq!(RemoteArg::classify(b"\x0131"), Tag(31));
    // The tag number is 5 bits: the sender never makes a bigger one, a hostile argument is cut to its low bits.
    assert_eq!(RemoteArg::classify(b"\x0133"), Tag(1));
    assert_eq!(RemoteArg::classify(b"0"), Int(0));
    assert_eq!(RemoteArg::classify(b"-1"), Int(-1));
    assert_eq!(RemoteArg::classify(b"2147483647"), Int(i32::MAX));
    assert_eq!(RemoteArg::classify(b"-2147483647"), Int(-2147483647));
    // Text whose `%d` of `atoi` is not itself stays text.
    for s in [
        "007",
        "+5",
        "-0",
        "5 ",
        " 5",
        "1.5",
        "12a",
        "2147483648",
        "-",
        "--1",
        "0x10",
        "-007",
    ] {
        assert_eq!(RemoteArg::classify(s.as_bytes()), text(s), "{s}");
    }
    // `-2147483648` is an integer as in Torque and arrives as 0 (see `minus_2147483648_arrives_as_zero`).
    assert_eq!(RemoteArg::classify(b"-2147483648"), Int(i32::MIN));
    assert_eq!(RemoteArg::classify(b"hello"), text("hello"));
    // Cut at 255 bytes.
    assert_eq!(RemoteArg::classify(&[b'q'; 300]), Text(vec![b'q'; 255]));
}

#[test]
fn integers_take_7_15_or_31_bits_after_a_sign_flag() {
    with_ctx(|ctx| {
        for (v, value_bits) in [
            (0, 7),
            (127, 7),
            (-127, 7),
            (128, 15),
            (32767, 15),
            (-32767, 15),
            (32768, 31),
            (i32::MAX, 31),
            (i32::MIN + 1, 31),
        ] {
            let mut w = BitWriter::new();
            RemoteArg::Int(v).pack(&mut w, ctx);
            // kind 2 + sign 1 + size flags (1, 2 or 2) + value
            let flags = match value_bits {
                7 => 1,
                _ => 2,
            };
            assert_eq!(w.bits(), 2 + 1 + flags + value_bits, "{v}");
            let bytes = w.into_bytes();
            let mut r = BitReader::new(&bytes);
            assert_eq!(RemoteArg::unpack(&mut r, ctx), Ok(RemoteArg::Int(v)), "{v}");
        }
    });
}

#[test]
fn a_command_of_every_argument_kind_round_trips() {
    let e = RemoteCommandEvent {
        variant: 0,
        args: vec![
            RemoteArg::Tag(3),
            RemoteArg::Empty,
            RemoteArg::Text(b"some text".to_vec()),
            RemoteArg::Text(b"\x0199".to_vec()),
            RemoteArg::Tag(31),
            RemoteArg::Int(-5),
            RemoteArg::Int(300),
            RemoteArg::Int(70_000),
        ],
    };
    let (got, _) = pack_unpack(&e);
    assert_eq!(got, Ok(e));
    let one = RemoteCommandEvent {
        variant: 0,
        args: vec![RemoteArg::Tag(0)],
    };
    assert_eq!(pack_unpack(&one).0, Ok(one));
    let empty = RemoteCommandEvent {
        variant: 0,
        args: vec![],
    };
    assert_eq!(pack_unpack(&empty).0, Ok(empty));
}

#[test]
fn argc_up_to_31_is_read_whole_as_the_reader_reads_it() {
    with_ctx(|ctx| {
        for argc in 0..=31u32 {
            let mut w = BitWriter::new();
            w.write_bits(argc, 5);
            for _ in 0..argc {
                w.write_bits(0, 2);
            }
            let bytes = w.into_bytes();
            let got = RemoteCommandEvent::unpack(&mut BitReader::new(&bytes), ctx);
            assert_eq!(got.expect("accepted").args.len(), argc as usize);
        }
        // A count the stream cannot hold is refused before anything is reserved, whatever the cap.
        let mut w = BitWriter::new();
        w.write_bits(21, 5);
        w.write_bits(0, 2);
        let bytes = w.into_bytes();
        assert!(matches!(
            RemoteCommandEvent::unpack(&mut BitReader::with_bit_len(&bytes, 7), ctx),
            Err(NetError::Cap { .. })
        ));
    });
}

#[test]
fn every_truncation_of_every_argument_kind_is_an_error() {
    let e = RemoteCommandEvent {
        variant: 0,
        args: vec![
            RemoteArg::Tag(9),
            RemoteArg::Text(b"a longer string argument".to_vec()),
            RemoteArg::Empty,
            RemoteArg::Int(-100_000),
            RemoteArg::Int(1000),
            RemoteArg::Int(7),
        ],
    };
    with_ctx(|ctx| {
        let mut w = BitWriter::new();
        w.enable_string_buffer();
        e.pack(&mut w, ctx);
        let bits = w.bits();
        let bytes = w.into_bytes();
        for cut in 0..bits {
            let mut r = BitReader::with_bit_len(&bytes, cut);
            r.enable_string_buffer();
            assert!(
                RemoteCommandEvent::unpack(&mut r, ctx).is_err(),
                "cut {cut}"
            );
        }
    });
}

#[test]
fn the_command_tag_text_rule_is_letters_digits_underscore_and_hyphen() {
    for ok in ["", "ClientJoin", "a_b-c9", "-", "_"] {
        assert!(valid_command_text(ok.as_bytes()), "{ok}");
    }
    for bad in ["a b", "a.b", "a%", "a\x01", "é", "a/b"] {
        assert!(!valid_command_text(bad.as_bytes()), "{bad}");
    }
}

fn drain(queue: &mut EventQueue<NetEvent>) -> Vec<NetEvent> {
    // Write everything into big packets and read it back with a receiver, so the test sees the events in sending order.
    with_ctx(|ctx| {
        let mut out = Vec::new();
        for n in 0..64u32 {
            let mut w = BitWriter::with_limit(1023 * 8);
            w.enable_string_buffer();
            let before = queue.ordered_len();
            queue
                .write_section(
                    &mut w,
                    ctx,
                    n,
                    SectionParams {
                        ghosting: false,
                        hold_ordered: false,
                        packet_bits: 1000 * 8,
                        min_start_bits: 0,
                    },
                )
                .expect("fits");
            let bytes = w.into_bytes();
            let mut r = BitReader::new(&bytes);
            r.enable_string_buffer();
            let mut rx = crate::events::EventReceiver::new();
            let mut got = Vec::new();
            let mut log = Vec::new();
            rx.read_section(
                &mut r,
                ctx,
                Role::Server,
                &mut crate::events::NetEventUnpacker,
                &mut got,
                Some(&mut log),
            )
            .expect("reads back");
            out.extend(log.into_iter().map(|i| i.event));
            if queue.ordered_len() == before {
                break;
            }
        }
        out
    })
}

use crate::protocol::Role;

#[test]
fn a_command_posts_its_new_tags_first_in_argument_order_then_the_command() {
    let tags = Tags::new(&["ServerMessage", "MsgHello", "Arg two", "Other"]);
    let mut queue = EventQueue::new();
    let mut strings = SendStrings::new();
    let args = vec![
        tag_arg(1),
        tag_arg(2),
        b"plain".to_vec(),
        tag_arg(3),
        b"12".to_vec(),
    ];
    let sent = send_command(&mut queue, &mut strings, &tags, &args).expect("sent");
    assert_eq!(sent.netstrings, 3);
    assert_eq!(sent.acquired, vec![1, 2, 3]);
    assert!(sent.released.is_empty());
    let events = drain(&mut queue);
    assert_eq!(events.len(), 4);
    let names: Vec<(u8, &[u8])> = events[..3]
        .iter()
        .map(|e| match e {
            NetEvent::NetString(n) => (n.slot, n.text.as_slice()),
            other => panic!("expected a NetString, got {other:?}"),
        })
        .collect();
    assert_eq!(
        names,
        vec![
            (0u8, b"ServerMessage".as_slice()),
            (1, b"MsgHello"),
            (2, b"Arg two")
        ]
    );
    let NetEvent::Remote(c) = &events[3] else {
        panic!("expected the command last");
    };
    assert_eq!(
        c.args,
        vec![
            RemoteArg::Tag(0),
            RemoteArg::Tag(1),
            RemoteArg::Text(b"plain".to_vec()),
            RemoteArg::Tag(2),
            RemoteArg::Int(12)
        ]
    );
}

#[test]
fn a_known_tag_is_sent_by_slot_with_no_new_netstring() {
    let tags = Tags::new(&["Cmd", "X"]);
    let mut queue = EventQueue::new();
    let mut strings = SendStrings::new();
    let (q, s) = (&mut queue, &mut strings);
    send_command(q, s, &tags, &[tag_arg(1), tag_arg(2)]).expect("first");
    let again = send_command(q, s, &tags, &[tag_arg(1), tag_arg(2), tag_arg(2)]).expect("second");
    assert_eq!(again.netstrings, 0);
    let events = drain(&mut queue);
    let NetEvent::Remote(c) = events.last().expect("events") else {
        panic!("a command");
    };
    assert_eq!(
        c.args,
        vec![RemoteArg::Tag(0), RemoteArg::Tag(1), RemoteArg::Tag(1)]
    );
}

#[test]
fn the_sender_refuses_what_torque_refuses() {
    let tags = Tags::new(&["Cmd", "bad name", "ok"]);
    let mut queue = EventQueue::new();
    let mut strings = SendStrings::new();
    let mut send =
        |tags: &Tags, args: &[Vec<u8>]| send_command(&mut queue, &mut strings, tags, args);
    // The natives test the command tag's text first and are silent when it fails (a plain word is number 0: no such tag) ...
    assert_eq!(
        send(&tags, &[b"Cmd".to_vec()]),
        Err(SendError::BadCommandName)
    );
    assert_eq!(
        send(&tags, &[Vec::new(), tag_arg(1)]),
        Err(SendError::BadCommandName)
    );
    // ... and only then is a command that is not a tag refused (a plain number naming a good tag passes the first test).
    assert_eq!(send(&tags, &[b"1".to_vec()]), Err(SendError::NotATag));
    assert_eq!(send(&tags, &[b"1".to_vec()]), Err(SendError::NotATag));
    assert_eq!(send(&tags, &[]), Err(SendError::NotATag));
    // The command tag's text is not [A-Za-z0-9_-]*, for the plain natives too; an unknown tag has no text.
    assert_eq!(send(&tags, &[tag_arg(2)]), Err(SendError::BadCommandName));
    assert_eq!(send(&tags, &[tag_arg(2)]), Err(SendError::BadCommandName));
    assert_eq!(send(&tags, &[tag_arg(9)]), Err(SendError::BadCommandName));
    assert_eq!(send(&tags, &[tag_arg(0)]), Err(SendError::BadCommandName));
    // 21 arguments are over the senders' 20; trailing empty ones do not count.
    let mut args = vec![tag_arg(1)];
    args.extend((0..19).map(|i| format!("a{i}").into_bytes()));
    assert!(send(&tags, &args).is_ok());
    args.push(b"one too many".to_vec());
    assert_eq!(send(&tags, &args), Err(SendError::TooManyArgs));
    args.pop();
    args.extend([Vec::new(), Vec::new()]);
    assert!(send(&tags, &args).is_ok());
}

#[test]
fn trailing_empty_arguments_are_dropped_and_an_invalid_tag_takes_a_slot_with_an_empty_text() {
    let tags = Tags::new(&["Cmd"]);
    let mut queue = EventQueue::new();
    let mut strings = SendStrings::new();
    send_command(
        &mut queue,
        &mut strings,
        &tags,
        &[
            tag_arg(1),
            tag_arg(0),
            tag_arg(77),
            b"x".to_vec(),
            Vec::new(),
            Vec::new(),
        ],
    )
    .expect("sent");
    let events = drain(&mut queue);
    // validateSendString clamps the id to 0..=size and checks it anyway: 0 is no tag, so it takes slot 1 with an empty text and a
    // NetString event; 77 clamps to 1 (the command tag) and is a hit.
    assert_eq!(events.len(), 3);
    assert!(matches!(&events[1], NetEvent::NetString(n) if n.slot == 1 && n.text.is_empty()));
    let NetEvent::Remote(c) = events.last().expect("events") else {
        panic!("a command");
    };
    assert_eq!(
        c.args,
        vec![
            RemoteArg::Tag(0),
            RemoteArg::Tag(1),
            RemoteArg::Tag(0),
            RemoteArg::Text(b"x".to_vec())
        ]
    );
}

#[test]
fn a_full_queue_refuses_the_whole_batch_before_the_lru_is_touched() {
    let tags = Tags::new(&["Cmd", "A", "B"]);
    let mut queue = EventQueue::new();
    let mut strings = SendStrings::new();
    // Fill the ordered queue to within 2 events of its cap: a command with 2 new tags needs 3.
    for _ in 0..NET_EVENT_QUEUE_ORDERED - 2 {
        queue
            .post(NetEvent::NetString(crate::netstring::NetStringEvent {
                slot: 0,
                text: Vec::new(),
                tag: 0,
            }))
            .expect("room");
    }
    let before = strings.clone().release_all();
    assert_eq!(
        send_command(
            &mut queue,
            &mut strings,
            &tags,
            &[tag_arg(1), tag_arg(2), tag_arg(3)]
        ),
        Err(SendError::Backpressure)
    );
    assert_eq!(strings.release_all(), before, "no entry was re-keyed");
    assert_eq!(queue.room_ordered(), 2, "nothing was posted");
    assert!(send_command(&mut queue, &mut strings, &tags, &[tag_arg(1)]).is_ok());
    // A closed queue refuses too.
    queue.close();
    assert_eq!(
        send_command(&mut queue, &mut strings, &tags, &[tag_arg(1)]),
        Err(SendError::Closed)
    );
}

#[test]
fn more_than_32_tags_recycle_slots_and_report_the_displaced_tags() {
    let texts: Vec<String> = (0..40).map(|i| format!("T{i}")).collect();
    let refs: Vec<&str> = std::iter::once("Cmd")
        .chain(texts.iter().map(String::as_str))
        .collect();
    let tags = Tags::new(&refs);
    let mut queue = EventQueue::new();
    let mut strings = SendStrings::new();
    let mut released = Vec::new();
    for i in 0..40u32 {
        let sent = send_command(
            &mut queue,
            &mut strings,
            &tags,
            &[tag_arg(1), tag_arg(i + 2)],
        )
        .expect("sent");
        released.extend(sent.released);
        if i % 8 == 7 {
            drain(&mut queue);
        }
    }
    // 41 distinct tags (the command's and 40 more) through 32 slots: 9 were displaced, the oldest first.
    assert_eq!(released.len(), 9);
    assert_eq!(
        released[0], 2,
        "the first argument tag is the LRU once the command tag is kept fresh"
    );
    let _ = NET_REMOTE_SLOTS;
}

fn mapped(strings: &mut RemoteStrings, slot: u8, id: u32, text: &str) {
    strings.set(slot, id, text.as_bytes());
}

#[test]
fn a_command_resolves_its_name_and_expands_tag_arguments() {
    let mut strings = RemoteStrings::new();
    mapped(&mut strings, 0, 40, "ServerMessage");
    mapped(&mut strings, 1, 41, "MsgJoin");
    mapped(&mut strings, 2, 42, "%1 joined %2");
    mapped(&mut strings, 3, 43, "Bob");
    // commandToClient(..., 'ServerMessage', 'MsgJoin', 5, 'Bob'-tag): args = [tag0, tag1, "5", tag3]
    let args = vec![
        b"\x010".to_vec(),
        b"\x011".to_vec(),
        b"5".to_vec(),
        b"\x013".to_vec(),
    ];
    let got = resolve(&args, &strings).expect("a command");
    assert_eq!(got.name, b"ServerMessage");
    assert_eq!(
        got.args,
        vec![
            b"\x0141 MsgJoin".to_vec(),
            b"5".to_vec(),
            b"\x0143 Bob".to_vec()
        ]
    );
    // A tag whose text holds %1 and %2 takes the arguments after it; a later tag argument is reduced to its text.
    let args = vec![
        b"\x010".to_vec(),
        b"\x012".to_vec(),
        b"\x013".to_vec(),
        b"seven".to_vec(),
    ];
    let got = resolve(&args, &strings).expect("a command");
    assert_eq!(got.args[0], b"\x0142 Bob joined seven".to_vec());
    assert_eq!(got.args[1], b"\x0143 Bob".to_vec());
}

#[test]
fn percent_rules_follow_expand_string() {
    let one = |text: &str, args: &[&str]| {
        let mut s = RemoteStrings::new();
        s.set(0, 7, text.as_bytes());
        let following: Vec<Vec<u8>> = args.iter().map(|x| x.as_bytes().to_vec()).collect();
        expand(s.get(0), &following)
    };
    assert_eq!(one("a%1b", &["X"]), b"\x017 aXb".to_vec());
    assert_eq!(
        one("%1%2%3", &["1", "2"]),
        b"\x017 12".to_vec(),
        "%3 with two arguments prints nothing"
    );
    assert_eq!(
        one("100%", &[]),
        b"\x017 100".to_vec(),
        "a trailing percent ends the text"
    );
    assert_eq!(
        one("%0x", &[]),
        b"\x017 0x".to_vec(),
        "%0 drops the percent and keeps the 0"
    );
    assert_eq!(
        one("%%1", &["Z"]),
        b"\x017 %1".to_vec(),
        "%% keeps one percent, the 1 is plain"
    );
    assert_eq!(one("%a%b", &[]), b"\x017 ab".to_vec());
    assert_eq!(
        one("%9", &["1", "2", "3", "4", "5", "6", "7", "8", "9"]),
        b"\x017 9".to_vec()
    );
    // A tag argument after the tag is reduced to its text (what follows its first space).
    assert_eq!(
        one("<%1>", &["\x0145 hello world"]),
        b"\x017 <hello world>".to_vec()
    );
    assert_eq!(one("<%1>", &["\x0145"]), b"\x017 <>".to_vec());
    // An unmapped handle prints <NULL> after the prefix.
    assert_eq!(expand(None, &[]), b"\x010 <NULL>".to_vec());
}

#[test]
fn the_expansion_is_capped_at_1023_bytes() {
    let mut strings = RemoteStrings::new();
    strings.set(0, 5, b"%1%1%1%1%1%1%1%1");
    let big = vec![b'x'; 255];
    let got = resolve(
        &[b"\x010".to_vec(), big.clone(), big.clone(), big],
        &strings,
    )
    .expect("command");
    assert_eq!(
        got.name.len(),
        NET_EXPAND_BYTES - 1 - 3,
        "the prefix is \\x01 5 and a space"
    );
    let direct = expand(strings.get(0), &[vec![b'y'; 255]]);
    assert_eq!(direct.len(), NET_EXPAND_BYTES - 1);
    // Text that fills the buffer exactly stops there.
    strings.set(1, 6, &[b'z'; 255]);
    let lots = vec![b"%1".to_vec(); 1];
    assert!(expand(strings.get(1), &lots).len() < NET_EXPAND_BYTES);
}

#[test]
fn an_unmapped_command_tag_runs_nothing_it_is_named_null() {
    let strings = RemoteStrings::new();
    let got = resolve(&[b"\x010".to_vec(), b"arg".to_vec()], &strings).expect("a command");
    assert_eq!(got.name, b"<NULL>");
    assert_eq!(
        function_name(b"serverCmd", &got.name),
        b"serverCmd<NULL>".to_vec()
    );
}

#[test]
fn a_command_that_is_not_a_tag_is_silent() {
    let mut strings = RemoteStrings::new();
    mapped(&mut strings, 0, 1, "Cmd");
    assert_eq!(
        resolve(&[b"plain".to_vec(), b"\x010".to_vec()], &strings),
        None
    );
    assert_eq!(resolve(&[Vec::new()], &strings), None);
    assert_eq!(resolve(&[], &strings), None);
    assert_eq!(resolve(&[b"7".to_vec()], &strings), None);
}

#[test]
fn a_tag_index_outside_0_to_31_is_an_unmapped_handle() {
    let mut strings = RemoteStrings::new();
    for slot in 0..32u8 {
        strings.set(slot, 100 + u32::from(slot), b"Mapped");
    }
    for bad in [
        "32",
        "33",
        "100",
        "255",
        "-1",
        "4294967296",
        "99999999999",
        "x",
    ] {
        let mut arg = vec![1u8];
        arg.extend_from_slice(bad.as_bytes());
        let got = resolve(&[b"\x010".to_vec(), arg], &strings).expect("a command");
        let expected = if bad == "x" {
            b"\x01100 Mapped".to_vec()
        } else {
            b"\x010 <NULL>".to_vec()
        };
        assert_eq!(got.args[0], expected, "index {bad}");
    }
}

#[test]
fn a_plain_string_argument_starting_with_byte_1_is_read_as_a_tag_with_its_index_checked() {
    // Kind 1 on the wire (a string), but its first byte is 1: Torque hands it to the tag lookup with an unchecked index.
    let e = RemoteCommandEvent {
        variant: 0,
        args: vec![
            RemoteArg::Tag(0),
            RemoteArg::Text(b"\x0150000".to_vec()),
            RemoteArg::Text(b"\x011".to_vec()),
        ],
    };
    let (decoded, _) = pack_unpack(&e);
    let decoded = decoded.expect("decodes");
    let mut strings = RemoteStrings::new();
    strings.set(0, 9, b"Cmd");
    strings.set(1, 10, b"One");
    let got = resolve(&decoded.script_args(), &strings).expect("a command");
    assert_eq!(
        got.args,
        vec![b"\x010 <NULL>".to_vec(), b"\x0110 One".to_vec()]
    );
}

#[test]
fn function_names_are_capped_at_1023_bytes() {
    assert_eq!(
        function_name(b"serverCmd", b"Hello"),
        b"serverCmdHello".to_vec()
    );
    let long = vec![b'a'; 2000];
    let name = function_name(b"serverCmd", &long);
    assert_eq!(name.len(), NET_COMMAND_NAME_BYTES);
    assert!(name.starts_with(b"serverCmdaaa"));
    assert_eq!(function_name(&long, b"x").len(), NET_COMMAND_NAME_BYTES);
    assert_eq!(function_prefix(true), b"serverCmd");
    assert_eq!(function_prefix(false), b"clientCmd");
}

#[test]
fn minus_2147483648_arrives_as_zero() {
    with_ctx(|ctx| {
        let mut w = BitWriter::new();
        RemoteArg::classify(b"-2147483648").pack(&mut w, ctx);
        // kind 3, the sign flag, the 7-bit form (the packer compares the negated value as a signed int: it stays negative) and 7 zero bits.
        assert_eq!(w.bits(), 2 + 1 + 1 + 7);
        let bytes = w.into_bytes();
        assert_eq!(
            RemoteArg::unpack(&mut BitReader::new(&bytes), ctx),
            Ok(RemoteArg::Int(0))
        );
    });
    assert_eq!(RemoteArg::Int(0).to_script(), b"0".to_vec());
}

#[test]
fn a_tag_that_names_nothing_takes_no_reference_and_gives_none_back() {
    let tags = Tags::new(&["Cmd", "Real"]);
    let mut queue = EventQueue::new();
    let mut strings = SendStrings::new();
    // `Cmd`, then a byte-1 argument naming id 2 (exists) and one naming id 2000 (clamped to the table size, 2: also exists)...
    let ghost = |id: u32| {
        let mut v = vec![1u8];
        v.extend_from_slice(id.to_string().as_bytes());
        v
    };
    // The table has two tags; id 0 and the clamped ids name nothing (id 0) or the last tag.
    let sent =
        send_command(&mut queue, &mut strings, &tags, &[ghost(1), ghost(0)]).expect("posted");
    assert_eq!(sent.acquired, [1], "only the tag that exists is acquired");
    // 33 more distinct slots evict everything; only the owned one comes back.
    let many = Tags::new(&["a"; 40]);
    let mut released = Vec::new();
    for id in 1..=40u32 {
        if let Ok(s) = send_command(&mut queue, &mut strings, &many, &[ghost(1), ghost(id)]) {
            released.extend(s.released);
        }
    }
    assert!(
        !released.contains(&0),
        "the entry of the empty tag owned nothing"
    );
    let mut rest = strings.release_all();
    rest.sort_unstable();
    assert!(!rest.contains(&0));
}

/// A game's own command class: the same codec under another class id, sent through the generic sender.
#[derive(Debug, Clone, PartialEq, Eq)]
enum GameCmd {
    String(crate::netstring::NetStringEvent),
    Own(RemoteCommandEvent),
}

impl crate::events::WireEvent for GameCmd {
    fn class(&self, ids: &crate::events::EventIds) -> u32 {
        match self {
            GameCmd::String(_) => ids.net_string,
            GameCmd::Own(_) => crate::testids::ANY,
        }
    }

    fn pack(&self, w: &mut BitWriter, ctx: &crate::events::EventCtx<'_>) {
        match self {
            GameCmd::String(e) => e.pack(w, ctx),
            GameCmd::Own(e) => e.pack(w, ctx),
        }
    }
}

#[test]
fn a_game_sends_its_own_command_class_through_the_same_sender() {
    let tags = Tags::new(&["Cmd", "X"]);
    let mut queue: EventQueue<GameCmd> = EventQueue::new();
    let mut strings = SendStrings::new();
    let sent = send_command_with(
        &mut queue,
        &mut strings,
        &tags,
        &[tag_arg(1), tag_arg(2), b"7".to_vec()],
        GameCmd::String,
        GameCmd::Own,
    )
    .expect("posted");
    assert_eq!(sent.netstrings, 2);
    assert_eq!(queue.ordered_len(), 3);
    with_ctx(|ctx| {
        let mut w = BitWriter::with_limit(1023 * 8);
        w.enable_string_buffer();
        let params = crate::events::SectionParams {
            ghosting: false,
            hold_ordered: false,
            packet_bits: 1023 * 8,
            min_start_bits: 0,
        };
        let written = queue.write_section(&mut w, ctx, 1, params).unwrap();
        assert_eq!(written.ordered, 3);
        // The last event on the wire is the game's class, its body the command's codec.
        let mut r = BitReader::new(w.bytes());
        r.enable_string_buffer();
        let mut rx = crate::events::EventReceiver::new();
        let mut out = Vec::new();
        struct Unpack;
        impl crate::events::EventUnpacker for Unpack {
            type Event = (u32, Option<RemoteCommandEvent>);
            fn unpack(
                &mut self,
                class: u32,
                r: &mut BitReader,
                ctx: &crate::events::EventCtx<'_>,
            ) -> Result<Self::Event, NetError> {
                if class == crate::testids::ANY {
                    RemoteCommandEvent::unpack(r, ctx).map(|c| (class, Some(c)))
                } else {
                    crate::netstring::NetStringEvent::unpack(r, ctx).map(|_| (class, None))
                }
            }
        }
        rx.read_section(
            &mut r,
            ctx,
            crate::protocol::Role::Server,
            &mut Unpack,
            &mut out,
            None,
        )
        .unwrap();
        let classes: Vec<u32> = out.iter().map(|i| i.event.0).collect();
        assert_eq!(
            classes,
            vec![ctx.ids.net_string, ctx.ids.net_string, crate::testids::ANY]
        );
        let cmd = out[2].event.1.clone().unwrap();
        assert_eq!(
            cmd.args,
            vec![RemoteArg::Tag(0), RemoteArg::Tag(1), RemoteArg::Int(7)]
        );
    });
}
