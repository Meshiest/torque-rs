use super::*;

#[test]
fn the_default_handshake_is_tges() {
    let p = HandshakeProfile::default();
    assert_eq!(p.types, HandshakeTypes::TORQUE);
    let t = p.types;
    assert_eq!(
        [
            t.challenge_request,
            t.challenge_reject,
            t.challenge_response,
            t.connect_request,
            t.connect_reject,
            t.connect_accept,
            t.disconnect
        ],
        [26, 28, 30, 32, 34, 36, 38]
    );
    assert_eq!(p.game, b"Torque Game Engine Demo");
    assert_eq!((p.protocol_current, p.protocol_min), (12, 12));
    assert!(p.form.default_request().is_empty());
    assert!(p.extension.is_none());
    let r = &p.rejects;
    assert_eq!(r.invalid, b"CHR_INVALID");
    assert_eq!(r.game, b"CHR_GAME");
    assert_eq!(r.protocol_less, b"CHR_PROTOCOL_LESS");
    assert_eq!(r.protocol_greater, b"CHR_PROTOCOL_GREATER");
    assert_eq!(r.password, b"CHR_PASSWORD");
    assert_eq!(r.invalid_args, b"CR_INVALID_ARGS");
    assert_eq!(r.server_full, b"CR_SERVERFULL");
    assert_eq!(r.protocol, b"CHR_PROTOCOL");
    // TGE's ghost table: `GhostIdBitSize` 12, `MaxGhostCount` 4,096; datablock ids of 10 bits.
    assert_eq!(DEFAULT_GHOST_LIMIT, 4_096);
    assert_eq!(p.ghost_bits(DEFAULT_GHOST_LIMIT), 12);
    assert_eq!(DEFAULT_DATABLOCK_BITS, 10);
}

#[test]
fn tges_form_writes_nothing_and_reads_nothing() {
    let form = TorqueForm;
    let mut w = BitWriter::new();
    form.write_request(&mut w, b"ignored");
    form.write_accept(&mut w, 12, 12);
    assert!(w.into_bytes().is_empty());
    let mut r = BitReader::new(&[]);
    assert_eq!(form.read_request(&mut r), Ok(Vec::new()));
    assert_eq!(form.read_accept(&mut r, 12), Ok(None));
}

/// A form made up for the tests: widths 15 to 20.
#[derive(Debug)]
struct Narrow(u8, u8);

impl HandshakeForm for Narrow {
    fn ghost_bits_range(&self) -> RangeInclusive<u8> {
        self.0..=self.1
    }
}

#[test]
fn the_ghost_width_stays_within_the_forms_range() {
    let p = HandshakeProfile {
        form: Arc::new(Narrow(15, 20)),
        ..HandshakeProfile::default()
    };
    assert_eq!(p.ghost_bits(1), 15);
    assert_eq!(p.ghost_bits(1 << 16), 16);
    assert_eq!(p.ghost_bits(u32::MAX), 20);
    // A range past the crate's bound is held to it.
    let wide = HandshakeProfile {
        form: Arc::new(Narrow(1, 40)),
        ..HandshakeProfile::default()
    };
    assert_eq!(wide.ghost_bits(u32::MAX), NET_MAX_GHOST_BITS);
    assert_eq!(wide.ghost_bits_bounds(), (1, NET_MAX_GHOST_BITS));
}

#[test]
fn the_extension_raises_the_width_only_for_a_peer_that_asked() {
    let mut p = HandshakeProfile {
        form: Arc::new(Narrow(1, 20)),
        ..HandshakeProfile::default()
    };
    let limit = 1 << 18;
    assert_eq!(p.accept_ghost_bits(limit, true), 18);
    let mut args = vec![b"name".to_vec(), b"ext/1".to_vec()];
    assert!(
        !p.take_extension_arg(&mut args),
        "no extension: nothing taken"
    );
    assert_eq!(args.len(), 2);
    p.extension = Some(HandshakeExtension {
        arg: b"ext/1".to_vec(),
        accept_word: 0x1234_5678,
        ghost_bits: Some(22),
        packet_bytes: Some(1_400),
        wide_limit: None,
    });
    assert_eq!(p.accept_ghost_bits(limit, false), 18);
    assert_eq!(p.accept_ghost_bits(limit, true), 22);
    // Never narrower than the standard width, never past the crate's bound.
    assert_eq!(p.accept_ghost_bits(1 << 20, true), 22);
    if let Some(ext) = p.extension.as_mut() {
        ext.ghost_bits = Some(60);
    }
    assert_eq!(p.accept_ghost_bits(1, true), NET_MAX_GHOST_BITS);
    // The argument is taken off only when it is the last one.
    assert!(p.take_extension_arg(&mut args));
    assert_eq!(args, vec![b"name".to_vec()]);
    let mut args = vec![b"ext/1".to_vec(), b"name".to_vec()];
    assert!(!p.take_extension_arg(&mut args));
    assert_eq!(args.len(), 2);
}

#[test]
fn the_default_query_profile_follows_the_handshake() {
    let q = QueryProfile::default();
    assert_eq!(q.version_string, b"VER1");
    assert_eq!((q.protocol_current, q.protocol_min, q.version), (12, 12, 0));
    assert_eq!(q.info_type_max, 22);
    let game = HandshakeProfile {
        protocol_current: 20,
        protocol_min: 17,
        ..HandshakeProfile::default()
    };
    let q = QueryProfile::for_handshake(&game);
    assert_eq!((q.protocol_current, q.protocol_min), (20, 17));
}
