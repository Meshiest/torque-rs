use super::*;
use crate::bits::BitReader;
use crate::handshake::{self, HandshakeTypes};
use crate::profile::{self, HandshakeProfile};

fn profile_with_extension() -> HandshakeProfile {
    HandshakeProfile {
        extension: Some(HandshakeExtension {
            arg: b"wide".to_vec(),
            accept_word: 0x1234_5678,
            ghost_bits: Some(14),
            packet_bytes: None,
            wide_limit: None,
        }),
        ..HandshakeProfile::default()
    }
}

#[test]
fn the_old_paths_name_the_same_items() {
    assert_eq!(handshake::CHANNELS_ACCEPT_WORD, CHANNELS_ACCEPT_WORD);
    let _: fn(&profile::HandshakeExtension) = |_: &HandshakeExtension| {};
    let _: Option<profile::WideLimit> = None::<WideLimit>;
    let p = profile_with_extension();
    assert_eq!(
        handshake::connect_accept_with(&p, 7, 12, 12, true, true),
        connect_accept_with(&p, 7, 12, 12, true, true)
    );
}

#[test]
fn an_accept_reads_back_its_tail() {
    let p = profile_with_extension();
    for (extension, channels) in [(false, false), (true, false), (false, true), (true, true)] {
        let bytes = connect_accept_with(&p, 9, 12, 12, extension, channels);
        let accept = handshake::parse_accept(&p, &bytes).unwrap();
        assert_eq!((accept.extension, accept.channels), (extension, channels));
    }
    // Torque's own profile has no extension: its accept can still end with the channels word.
    let plain = HandshakeProfile::default();
    assert_eq!(plain.types, HandshakeTypes::TORQUE);
    let bytes = connect_accept_with(&plain, 9, 12, 12, false, true);
    let mut r = BitReader::new(&bytes[5..]);
    r.read_u32().unwrap();
    assert_eq!(read_accept_tail(&plain, &mut r), (false, true));
}

#[test]
fn the_extension_argument_comes_off_the_end_once() {
    let p = profile_with_extension();
    let mut args = vec![b"a".to_vec(), b"wide".to_vec()];
    assert!(p.take_extension_arg(&mut args));
    assert_eq!(args, [b"a".to_vec()]);
    assert!(!p.take_extension_arg(&mut args));
    assert!(!HandshakeProfile::default().take_extension_arg(&mut vec![b"wide".to_vec()]));
}
