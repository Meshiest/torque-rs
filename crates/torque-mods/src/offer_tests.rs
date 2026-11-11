use super::*;

fn caps() -> OfferCaps {
    OfferCaps {
        files: 100,
        file_bytes: 4 << 20,
        total_bytes: 64 << 20,
    }
}

fn bytes_of(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| ((i * 31 + usize::from(seed)) % 251) as u8)
        .collect()
}

fn offer(download: bool) -> ModOffer {
    ModOffer {
        id: "props".to_string(),
        name: "Props Pack".to_string(),
        version: "1.2".to_string(),
        hash: [3; 32],
        download,
        files: vec![
            FileOffer::of("art/crate.png", &bytes_of(BLOCK_BYTES * 2 + 100, 1)).unwrap(),
            FileOffer::of("sounds/boom.ogg", &bytes_of(10, 2)).unwrap(),
            FileOffer::of("empty.txt", &[]).unwrap(),
        ],
    }
}

#[test]
fn blocks_are_counted_and_cut_the_same_way_everywhere() {
    assert_eq!(block_count(0), 0);
    assert_eq!(block_count(1), 1);
    assert_eq!(block_count(BLOCK_BYTES as u64), 1);
    assert_eq!(block_count(BLOCK_BYTES as u64 + 1), 2);
    let size = (BLOCK_BYTES * 2 + 100) as u64;
    assert_eq!(run_bytes(size, 0, 1), (0, BLOCK_BYTES as u64));
    assert_eq!(run_bytes(size, 2, 4), (2 * BLOCK_BYTES as u64, size));
    assert_eq!(run_bytes(size, 9, 1), (size, size));
    assert_eq!(run_bytes(size, u64::MAX, u64::MAX), (size, size));
    let f = FileOffer::of("a", &bytes_of(size as usize, 5)).unwrap();
    assert_eq!(f.blocks.len(), 3);
    assert_eq!(f.size as u64, size);
    assert_ne!(f.blocks[0], f.blocks[1]);
}

#[test]
fn an_offer_reads_back_and_a_mod_that_is_not_served_lists_no_files() {
    let mods = vec![
        offer(true),
        ModOffer {
            id: "base".into(),
            ..offer(false)
        },
    ];
    let back = decode_offer(&encode_offer(&mods), &caps()).unwrap();
    assert_eq!(back[0], mods[0]);
    assert_eq!(back[1].id, "base");
    assert!(
        back[1].files.is_empty(),
        "a mod that is not served has no file list"
    );
    assert!(!back[1].download);
    assert_eq!(decode_offer(&encode_offer(&[]), &caps()).unwrap(), []);
    assert_eq!(back[0].bytes(), (BLOCK_BYTES * 2 + 110) as u64);
}

#[test]
fn an_offer_with_a_script_a_bad_path_or_a_repeat_is_refused() {
    for path in [
        "main.cs", "x.DSO", "d.wasm", "e.cs.dso", "ui/x.gui", "x.dll", "y.SO",
    ] {
        let mut m = offer(true);
        m.files[1].path = path.to_string();
        assert!(
            matches!(
                decode_offer(&encode_offer(&[m]), &caps()),
                Err(MessageError::Script(_))
            ),
            "{path}"
        );
    }
    for path in ["", "../x.png", "/x.png", "a\\b.png", "a\u{0}b"] {
        let mut m = offer(true);
        m.files[1].path = path.to_string();
        assert!(
            decode_offer(&encode_offer(&[m]), &caps()).is_err(),
            "{path:?}"
        );
    }
    let mut m = offer(true);
    m.files[1].path = "ART/Crate.png".to_string();
    assert!(
        decode_offer(&encode_offer(&[m]), &caps()).is_err(),
        "two spellings of one path"
    );
    // A mod id or version that could reorder text on a screen.
    let mut m = offer(true);
    m.id = "pro\u{202e}ps".to_string();
    assert!(decode_offer(&encode_offer(&[m]), &caps()).is_err());
    let mut m = offer(true);
    m.version = "1.\u{200b}2".to_string();
    assert!(decode_offer(&encode_offer(&[m]), &caps()).is_err());
}

#[test]
fn an_offer_naming_one_id_twice_or_an_id_a_local_mod_could_not_have_is_refused() {
    let mut twice = offer(true);
    twice.id = "Props".to_string();
    twice.hash = [4; 32];
    assert!(matches!(
        decode_offer(&encode_offer(&[offer(true), twice]), &caps()),
        Err(MessageError::Malformed("repeated mod id"))
    ));
    for id in ["a/b", "a:b", "a\"b", ".hidden", "a b"] {
        let mut m = offer(true);
        m.id = id.to_string();
        assert!(
            matches!(
                decode_offer(&encode_offer(&[m]), &caps()),
                Err(MessageError::Malformed("mod id"))
            ),
            "{id}"
        );
    }
}

#[test]
fn a_mods_display_name_travels_in_the_offer_and_an_empty_one_is_its_id() {
    let back = decode_offer(&encode_offer(&[offer(true)]), &caps()).unwrap();
    assert_eq!(back[0].name, "Props Pack");
    let mut m = offer(true);
    m.name = String::new();
    let back = decode_offer(&encode_offer(&[m]), &caps()).unwrap();
    assert_eq!(back[0].name, "props");
}

#[test]
fn a_display_name_that_is_too_long_or_not_plain_text_is_refused() {
    let longest = "n".repeat(torque_net::modsync::MAX_DISPLAY_NAME_BYTES);
    let mut m = offer(true);
    m.name = longest.clone();
    assert_eq!(
        decode_offer(&encode_offer(&[m]), &caps()).unwrap()[0].name,
        longest
    );
    for name in [
        format!("{longest}n"),
        "n".repeat(300),
        "Pro\u{1}ps".to_string(),
        "Pro\nps".to_string(),
        "Pro\u{202e}ps".to_string(),
        "<b>Props</b>".to_string(),
        " Props".to_string(),
    ] {
        let mut m = offer(true);
        m.name = name.clone();
        assert!(
            matches!(
                decode_offer(&encode_offer(&[m]), &caps()),
                Err(MessageError::Malformed(_))
            ),
            "{name:?}"
        );
    }
}

#[test]
fn a_message_of_another_version_is_refused_with_the_versions() {
    let current = encode_offer(&[offer(true)]);
    let data = encode_data(
        &Request {
            mod_index: 0,
            file: 0,
            start: 0,
            count: 1,
        },
        b"x",
    );
    for other in [1u8, 3, 9] {
        let byte = b'0' + other;
        let mut old = current.clone();
        old[3] = byte;
        assert!(
            is_offer(&old),
            "an offer of any version is recognized, to be answered"
        );
        let found = decode_offer(&old, &caps());
        assert_eq!(found, Err(MessageError::Version(byte)));
        assert!(
            found
                .unwrap_err()
                .to_string()
                .contains(&format!("version {other}")),
            "the reason names the version"
        );
        let mut old_data = data.clone();
        old_data[3] = byte;
        assert_eq!(decode_data(&old_data), Err(MessageError::Version(byte)));
    }
    // A head byte that is no digit is shown as the byte, not as a number from subtracting a digit.
    let mut odd = current.clone();
    odd[3] = 0xff;
    let shown = decode_offer(&odd, &caps()).unwrap_err().to_string();
    assert!(
        shown.contains("unknown version") && shown.contains("0xff"),
        "{shown}"
    );
    odd[3] = b':';
    assert_eq!(
        decode_offer(&odd, &caps()),
        Err(MessageError::Version(b':'))
    );
    // Not a mods message at all stays a bad message.
    let mut other = current;
    other[0] = b'X';
    assert!(!is_offer(&other));
    assert_eq!(
        decode_offer(&other, &caps()),
        Err(MessageError::Malformed("magic"))
    );
}

#[test]
fn lua_files_are_content_in_an_offer() {
    let mut m = offer(true);
    m.files[1].path = "ui/panel.LUA".to_string();
    let back = decode_offer(&encode_offer(&[m]), &caps()).unwrap();
    assert_eq!(back[0].files[1].path, "ui/panel.LUA");
}

#[test]
fn an_offer_over_the_caps_is_refused_before_it_is_read() {
    let message = encode_offer(&[offer(true)]);
    let few = OfferCaps { files: 2, ..caps() };
    assert_eq!(
        decode_offer(&message, &few).unwrap_err(),
        MessageError::TooLarge("files")
    );
    let small = OfferCaps {
        file_bytes: 1_000,
        ..caps()
    };
    assert_eq!(
        decode_offer(&message, &small).unwrap_err(),
        MessageError::TooLarge("file size")
    );
    let tight = OfferCaps {
        total_bytes: 1_000,
        ..caps()
    };
    assert_eq!(
        decode_offer(&message, &tight).unwrap_err(),
        MessageError::TooLarge("total size")
    );
    // A claim of four billion files, or of a file whose blocks are not there, is not believed.
    let mut lie = message[..4 + 1 + 2 + 1 + 5 + 1 + 3 + 32 + 1].to_vec();
    lie.extend_from_slice(&u32::MAX.to_le_bytes());
    assert!(
        decode_offer(
            &lie,
            &OfferCaps {
                files: usize::MAX,
                ..caps()
            }
        )
        .is_err()
    );
    assert!(decode_offer(&vec![0; MAX_OFFER_BYTES + 1], &caps()).is_err());
}

#[test]
fn a_flood_of_block_hashes_is_bounded_by_what_the_offer_may_hold() {
    // A file of the largest size the caps allow has a block list that must be in the message.
    let m = ModOffer {
        files: vec![FileOffer {
            path: "big.bin".to_string(),
            size: (4 << 20) as u32,
            hash: [0; 32],
            blocks: Vec::new(),
        }],
        ..offer(true)
    };
    let message = encode_offer(&[m]);
    assert!(
        decode_offer(&message, &caps()).is_err(),
        "the 64 block hashes it claims are missing"
    );
}

#[test]
fn every_cut_and_extra_byte_of_an_offer_is_an_error() {
    let message = encode_offer(&[offer(true)]);
    for cut in 0..message.len() {
        assert!(decode_offer(&message[..cut], &caps()).is_err(), "{cut}");
    }
    let mut more = message;
    more.push(0);
    assert!(decode_offer(&more, &caps()).is_err());
}

#[test]
fn data_reads_back_as_the_request_it_answers() {
    let r = Request {
        mod_index: 2,
        file: 7,
        start: 3,
        count: 4,
    };
    let message = encode_data(&r, b"bytes");
    assert!(is_data(&message) && !is_offer(&message));
    let (back, bytes) = decode_data(&message).unwrap();
    assert_eq!((back, bytes), (r, &b"bytes"[..]));
    assert!(decode_data(&message[..10]).is_err());
    assert!(is_offer(&encode_offer(&[])));
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

#[test]
fn hostile_offers_and_data_never_panic() {
    let mut rng = Rng(0x51de_b10c_5eed_0001);
    let good = encode_offer(&[
        offer(true),
        ModOffer {
            id: "b".into(),
            ..offer(false)
        },
    ]);
    let data = encode_data(
        &Request {
            mod_index: 0,
            file: 0,
            start: 0,
            count: 1,
        },
        &[1, 2, 3],
    );
    for case in 0..20_000 {
        let mut bytes = if case % 3 == 0 {
            data.clone()
        } else {
            good.clone()
        };
        match rng.next() % 4 {
            0 => {
                for _ in 0..1 + rng.next() % 5 {
                    let at = (rng.next() as usize) % bytes.len();
                    bytes[at] = rng.next() as u8;
                }
            }
            1 => bytes.truncate((rng.next() as usize) % (bytes.len() + 1)),
            2 => {
                bytes = (0..(rng.next() % 500) as usize)
                    .map(|_| rng.next() as u8)
                    .collect()
            }
            _ => {
                let at = (rng.next() as usize) % bytes.len();
                bytes.splice(at..at, (0..rng.next() % 40).map(|_| rng.next() as u8));
            }
        }
        let _ = decode_offer(&bytes, &caps());
        let _ = decode_data(&bytes);
    }
}
