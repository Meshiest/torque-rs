use super::*;
use crate::package::ModLimits;

fn files() -> Vec<(String, Vec<u8>)> {
    vec![
        ("art/rock.png".to_string(), b"rock pixels".to_vec()),
        ("Sounds/boom.ogg".to_string(), vec![7; 300]),
        ("props/crate.txt".to_string(), Vec::new()),
    ]
}

fn caps() -> ContentCaps {
    ContentCaps {
        files: 100,
        file_bytes: 1 << 20,
        total_bytes: 8 << 20,
    }
}

#[test]
fn the_hash_ignores_order_and_the_case_of_paths_and_follows_the_bytes() {
    let a = summarize_files(&files());
    let mut reordered = files();
    reordered.reverse();
    assert_eq!(summarize_files(&reordered), a);
    let mut respelled = files();
    respelled[0].0 = "ART/Rock.PNG".to_string();
    assert_eq!(summarize_files(&respelled).hash, a.hash);
    let mut changed = files();
    changed[0].1.push(1);
    assert_ne!(summarize_files(&changed).hash, a.hash);
    let mut renamed = files();
    renamed[0].0 = "art/rock2.png".to_string();
    assert_ne!(summarize_files(&renamed).hash, a.hash);
    assert_eq!(a.files, 3);
    assert_eq!(a.bytes, 11 + 300);
    // Two files that trade bytes are not the same content.
    let ab = summarize_files(&[("a".into(), b"xy".to_vec()), ("b".into(), b"z".to_vec())]);
    let ba = summarize_files(&[("a".into(), b"x".to_vec()), ("b".into(), b"yz".to_vec())]);
    assert_ne!(ab.hash, ba.hash);
}

#[test]
fn the_hash_text_round_trips_and_refuses_anything_else() {
    let hash = summarize_files(&files()).hash;
    assert_eq!(from_hex(&hex(&hash)), Some(hash));
    assert_eq!(hex(&hash).len(), 64);
    assert_eq!(from_hex(&hex(&hash).to_uppercase()), None);
    assert_eq!(from_hex(&hex(&hash)[1..]), None);
    assert_eq!(from_hex(&format!("{}g", &hex(&hash)[..63])), None);
}

#[test]
fn a_packages_content_leaves_out_scripts_and_what_the_layout_hid() {
    let package = ModPackage::from_files(
        vec![
            ("mod.toml".into(), b"id = \"a\"".to_vec()),
            ("server.cs".into(), b"echo(1);".to_vec()),
            ("lib/util.CS".into(), b"echo(2);".to_vec()),
            ("gui/menu.gui".into(), b"x".to_vec()),
            ("maps/one.mis".into(), b"x".to_vec()),
            ("art/rock.png".into(), b"png".to_vec()),
            ("Art/Tree.png".into(), b"png".to_vec()),
        ],
        &ModLimits::default(),
    )
    .unwrap();
    let hidden = vec!["mod.toml".to_string(), "server.cs".to_string()];
    let names = content_names(&package, &hidden);
    assert_eq!(names, ["art/rock.png", "Art/Tree.png"]);
    let summary = summarize(&package, &hidden).unwrap();
    // The same content under another folder spelling and with other scripts is the same content.
    let other = ModPackage::from_files(
        vec![
            ("mod.toml".into(), b"id = \"b\"".to_vec()),
            ("client.cs".into(), b"different".to_vec()),
            ("ART/ROCK.png".into(), b"png".to_vec()),
            ("art/tree.png".into(), b"png".to_vec()),
        ],
        &ModLimits::default(),
    )
    .unwrap();
    assert_eq!(
        summarize(&other, &["mod.toml".to_string(), "client.cs".to_string()]),
        Some(summary)
    );
}

#[test]
fn messages_read_back_as_they_were_written() {
    let hash = summarize_files(&files()).hash;
    let message = encode_content("props", "1.2", &hash, &files());
    let content = decode_content(&message, &caps()).unwrap();
    assert_eq!(content.id, "props");
    assert_eq!(content.version, "1.2");
    assert_eq!(content.hash, hash);
    assert_eq!(content.files, files());
    assert_eq!(
        peek_head(&message),
        Some(("props".to_string(), "1.2".to_string(), hash))
    );
    assert_eq!(
        peek_head(&message[..HEAD_BYTES.min(message.len())]),
        peek_head(&message)
    );
}

#[test]
fn a_script_in_a_content_message_is_refused_in_every_spelling() {
    let hash = summarize_files(&files()).hash;
    for path in [
        "main.cs",
        "dir/Server.CS",
        "x.dso",
        "x.DSO",
        "x.cs.dso",
        "ui/menu.gui",
        "maps/a.MIS",
        "tools/x.ed",
        "tools/x.ed.cs",
        "a.cs.png",
    ] {
        let mut with = files();
        with.push((path.to_string(), b"echo(1);".to_vec()));
        let message = encode_content("props", "1", &hash, &with);
        assert!(
            matches!(
                decode_content(&message, &caps()),
                Err(MessageError::Script(_))
            ),
            "{path}"
        );
    }
}

#[test]
fn a_content_message_over_the_caps_is_refused_before_it_is_read() {
    let hash = summarize_files(&files()).hash;
    let message = encode_content("props", "1", &hash, &files());
    let few = ContentCaps { files: 2, ..caps() };
    assert_eq!(
        decode_content(&message, &few).unwrap_err(),
        MessageError::TooLarge("files")
    );
    let small = ContentCaps {
        file_bytes: 100,
        ..caps()
    };
    assert_eq!(
        decode_content(&message, &small).unwrap_err(),
        MessageError::TooLarge("file size")
    );
    let tight = ContentCaps {
        total_bytes: 200,
        ..caps()
    };
    assert_eq!(
        decode_content(&message, &tight).unwrap_err(),
        MessageError::TooLarge("total size")
    );
    // A claim of four billion files in a few bytes is not believed.
    let mut lie = message[..4 + 1 + 1 + 5 + 1 + 3 + 32].to_vec();
    lie.extend_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(
        decode_content(
            &lie,
            &ContentCaps {
                files: usize::MAX,
                ..caps()
            }
        ),
        Err(MessageError::TooLarge(_))
    ));
}

#[test]
fn every_cut_and_every_extra_byte_of_a_message_is_an_error() {
    let hash = summarize_files(&files()).hash;
    let message = encode_content("props", "1", &hash, &files());
    for cut in 0..message.len() {
        assert!(decode_content(&message[..cut], &caps()).is_err(), "{cut}");
    }
    let mut more = message.clone();
    more.push(0);
    assert!(decode_content(&more, &caps()).is_err());
}

#[test]
fn bad_paths_and_text_are_refused() {
    let hash = [1u8; 32];
    for path in ["", "a\u{0}b"] {
        let message = encode_content("p", "1", &hash, &[(path.to_string(), vec![1])]);
        assert!(decode_content(&message, &caps()).is_err(), "{path:?}");
    }
    let message = encode_content("p\u{7}q", "1", &hash, &[("a".to_string(), vec![1])]);
    assert!(decode_content(&message, &caps()).is_err());
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
fn hostile_messages_never_panic() {
    let mut rng = Rng(0x2f6e_2b1c_9d3a_5e47);
    let hash = summarize_files(&files()).hash;
    let good = encode_content("props", "1.2", &hash, &files());
    for case in 0..20_000 {
        let mut bytes = good.clone();
        match case % 5 {
            0 | 1 => {
                for _ in 0..1 + rng.next() % 5 {
                    let at = (rng.next() as usize) % bytes.len();
                    bytes[at] = rng.next() as u8;
                }
            }
            2 => bytes.truncate((rng.next() as usize) % (bytes.len() + 1)),
            3 => {
                bytes = (0..(rng.next() % 600) as usize)
                    .map(|_| rng.next() as u8)
                    .collect()
            }
            _ => {
                let at = (rng.next() as usize) % bytes.len();
                bytes.splice(at..at, (0..rng.next() % 40).map(|_| rng.next() as u8));
            }
        }
        let _ = decode_content(&bytes, &caps());
        let _ = peek_head(&bytes);
    }
}
