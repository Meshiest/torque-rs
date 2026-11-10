use super::*;
use crate::testrng::TestRng;

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

fn tag(id: u32) -> Vec<u8> {
    format!("\u{1}{id}").into_bytes()
}

fn text(s: &str) -> Vec<u8> {
    s.as_bytes().to_vec()
}

#[test]
fn a_command_with_tags_text_and_numbers_makes_a_body_that_reads_back() {
    let tags = Tags::new(&["MsgHello", "Hello %1 of %2"]);
    let mut strings = SendStrings::new();
    let args = [
        tag(1),
        tag(2),
        text("bob"),
        text("-7"),
        Vec::new(),
        text("a b"),
    ];
    let bytes = encode_command(&mut strings, &tags, 0, &args).expect("a body");
    let body = CommandBody::decode(&bytes).expect("reads back");
    assert_eq!(body.variant, 0);
    assert_eq!(
        body.strings,
        [(0, text("MsgHello")), (1, text("Hello %1 of %2"))]
    );
    assert_eq!(
        body.args,
        [
            RemoteArg::Tag(0),
            RemoteArg::Tag(1),
            RemoteArg::Text(text("bob")),
            RemoteArg::Int(-7),
            RemoteArg::Empty,
            RemoteArg::Text(text("a b")),
        ]
    );
    assert_eq!(body.script_args()[0], b"\x010");
}

#[test]
fn a_tag_used_twice_travels_once_and_a_second_command_names_the_same_slot() {
    let tags = Tags::new(&["Cmd", "Other"]);
    let mut strings = SendStrings::new();
    let first = encode_command(&mut strings, &tags, 0, &[tag(1), tag(2), tag(2)]).unwrap();
    let first = CommandBody::decode(&first).unwrap();
    assert_eq!(first.strings.len(), 2, "one text for each distinct tag");
    let second = encode_command(&mut strings, &tags, 0, &[tag(1)]).unwrap();
    let second = CommandBody::decode(&second).unwrap();
    // The slot is the sender's table's, and the text still comes with the command: it needs no earlier one.
    assert_eq!(second.strings, [(first.strings[0].0, text("Cmd"))]);
}

#[test]
fn what_send_command_refuses_is_refused_here() {
    let tags = Tags::new(&["Cmd", "bad name"]);
    let mut strings = SendStrings::new();
    let err = |args: &[Vec<u8>], strings: &mut SendStrings| {
        encode_command(strings, &tags, 0, args).unwrap_err()
    };
    assert_eq!(
        err(&[text("1")], &mut strings),
        BodyError::Send(SendError::NotATag)
    );
    assert_eq!(
        err(&[tag(2)], &mut strings),
        BodyError::Send(SendError::BadCommandName)
    );
    let mut many = vec![tag(1)];
    many.extend((0..20).map(|n| text(&n.to_string())));
    assert_eq!(
        err(&many, &mut strings),
        BodyError::Send(SendError::TooManyArgs)
    );
}

#[test]
fn trailing_empty_arguments_are_dropped_and_long_text_is_cut_at_255_bytes() {
    let tags = Tags::new(&["Cmd"]);
    let mut strings = SendStrings::new();
    let long = vec![b'x'; 400];
    let bytes = encode_command(
        &mut strings,
        &tags,
        0,
        &[tag(1), long, Vec::new(), Vec::new()],
    )
    .unwrap();
    let body = CommandBody::decode(&bytes).unwrap();
    assert_eq!(body.args.len(), 2);
    assert_eq!(body.args[1], RemoteArg::Text(vec![b'x'; 255]));
}

#[test]
fn a_body_over_its_size_is_not_made() {
    let tags = Tags::new(&["Cmd"]);
    let mut strings = SendStrings::new();
    let mut args = vec![tag(1)];
    args.extend((0..10).map(|n| {
        let mut t = vec![b'a' + n as u8; 255];
        t[0] = b'z';
        t
    }));
    assert_eq!(
        encode_command(&mut strings, &tags, 0, &args).unwrap_err(),
        BodyError::TooLarge
    );
}

#[test]
fn the_integer_form_keeps_tges_quirk_for_the_smallest_number() {
    let tags = Tags::new(&["Cmd"]);
    let mut strings = SendStrings::new();
    let bytes = encode_command(&mut strings, &tags, 0, &[tag(1), text("-2147483648")]).unwrap();
    assert_eq!(
        CommandBody::decode(&bytes).unwrap().args[1],
        RemoteArg::Int(0)
    );
}

#[test]
fn a_body_that_is_not_one_is_an_error_and_never_a_panic() {
    let tags = Tags::new(&["Cmd", "Other"]);
    let mut strings = SendStrings::new();
    let good = encode_command(
        &mut strings,
        &tags,
        0,
        &[tag(1), tag(2), text("x"), text("5")],
    )
    .unwrap();
    // Every cut of a good body, and a byte left over.
    for cut in 0..good.len() {
        assert!(CommandBody::decode(&good[..cut]).is_err(), "cut at {cut}");
    }
    let mut extra = good.clone();
    extra.push(0);
    assert!(CommandBody::decode(&extra).is_err());
    // Counts, slots and kinds past their caps.
    assert!(CommandBody::decode(&[0, 21]).is_err());
    assert!(CommandBody::decode(&[0, 0, 21]).is_err());
    assert!(CommandBody::decode(&[0, 1, 32, 0, 0]).is_err());
    assert!(CommandBody::decode(&[0, 0, 1, 2, 32]).is_err());
    assert!(CommandBody::decode(&[0, 0, 1, 9]).is_err());
    assert!(CommandBody::decode(&vec![0; super::MAX_BODY_BYTES + 1]).is_err());
    // Random bytes and flipped bits of a good body.
    let mut rng = TestRng::new(0xb0d1);
    for _ in 0..20_000 {
        let len = rng.index(80);
        let bytes: Vec<u8> = (0..len).map(|_| rng.byte()).collect();
        let _ = CommandBody::decode(&bytes);
        let mut flipped = good.clone();
        let at = rng.index(flipped.len());
        flipped[at] ^= 1 << rng.below(8);
        let _ = CommandBody::decode(&flipped);
    }
}

#[test]
fn a_command_that_names_a_tag_the_sender_lacks_is_not_made() {
    let tags = Tags::new(&["Cmd"]);
    let mut strings = SendStrings::new();
    assert_eq!(
        encode_command(&mut strings, &tags, 0, &[tag(1), tag(0)]).unwrap_err(),
        BodyError::UnknownTag
    );
}

#[test]
fn the_strings_of_a_body_are_exactly_its_tags_each_slot_once() {
    let one = |strings: &[(u8, &str)], args: &[RemoteArg]| {
        CommandBody {
            variant: 0,
            strings: strings.iter().map(|(s, t)| (*s, text(t))).collect(),
            args: args.to_vec(),
        }
        .encode()
        .map(|bytes| CommandBody::decode(&bytes))
        .unwrap()
    };
    assert!(one(&[(0, "Cmd")], &[RemoteArg::Tag(0)]).is_ok());
    // A slot given twice, a string no argument names, a tag with no string.
    assert!(one(&[(0, "a"), (0, "b")], &[RemoteArg::Tag(0)]).is_err());
    assert!(one(&[(0, "Cmd"), (1, "x")], &[RemoteArg::Tag(0)]).is_err());
    assert!(one(&[], &[RemoteArg::Tag(0)]).is_err());
    assert!(one(&[(0, "Cmd")], &[RemoteArg::Text(text("x"))]).is_err());
}
