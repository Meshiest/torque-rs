use super::*;
use crate::manifest::{MAX_MANIFEST_BYTES, Manifest};

fn from() -> SocketAddr {
    SocketAddr::from(([10, 0, 0, 7], 4000))
}

fn cx() -> AuthContext {
    AuthContext {
        server: SocketAddr::from(([192, 0, 2, 1], 28_000)),
        sequence: 5,
        challenge: [1, 2, 3, 4],
    }
}

fn server_manifest() -> Manifest {
    Manifest::new(12, 12)
        .with_plugin("torque.rope", 2)
        .with_ghosts(["Rope"])
}

fn bytes(block: &ConnectBlock) -> Vec<u8> {
    let mut w = BitWriter::new();
    block.write(&mut w).expect("writes");
    w.into_bytes()
}

fn read(bytes: &[u8]) -> Result<Option<ConnectBlock>, BlockError> {
    ConnectBlock::read(&mut BitReader::new(bytes))
}

fn fields(block: Option<ConnectBlock>) -> ConnectFields {
    let mut f = ConnectFields::standard(b"", Vec::new());
    f.block = block.map(Box::new);
    f
}

#[test]
fn a_block_reads_back_as_it_was_written() {
    let block = ConnectBlock {
        manifest: Some(server_manifest()),
        ticket: Some(b"ticket".to_vec()),
        ..Default::default()
    };
    assert_eq!(read(&bytes(&block)).unwrap(), Some(block));
    let empty = ConnectBlock::default();
    assert_eq!(read(&bytes(&empty)).unwrap(), Some(empty));
}

#[test]
fn bytes_that_do_not_start_with_the_magic_are_no_block() {
    for data in [&[][..], &[1, 2, 3][..], &[0; 12][..], &[0xff; 12][..]] {
        let mut r = BitReader::new(data);
        assert_eq!(ConnectBlock::read(&mut r), Ok(None));
        assert_eq!(r.position(), 0, "the reader is left where it was");
    }
}

#[test]
fn the_magic_is_not_a_valid_argument_count() {
    assert!(BLOCK_MAGIC as usize > crate::cap::NET_MAX_CONNECT_ARGS);
}

#[test]
fn a_section_a_build_does_not_know_is_skipped() {
    let mut w = BitWriter::new();
    w.write_u32(BLOCK_MAGIC);
    w.write_u8(2);
    w.write_u8(77);
    w.write_bits(3, 16);
    w.write_bytes(&[1, 2, 3]);
    w.write_u8(TAG_TICKET);
    w.write_bits(2, 16);
    w.write_bytes(b"ok");
    let block = read(&w.into_bytes()).unwrap().unwrap();
    assert_eq!(block.ticket.as_deref(), Some(&b"ok"[..]));
    assert!(block.manifest.is_none());
}

#[test]
fn a_garbled_block_is_an_error_not_a_panic() {
    let good = bytes(&ConnectBlock {
        manifest: Some(server_manifest()),
        ticket: Some(b"abc".to_vec()),
        ..Default::default()
    });
    for len in 4..good.len() {
        assert!(read(&good[..len]).is_err(), "cut at {len}");
    }
    // The same section twice.
    let mut w = BitWriter::new();
    w.write_u32(BLOCK_MAGIC);
    w.write_u8(2);
    for _ in 0..2 {
        w.write_u8(TAG_TICKET);
        w.write_bits(1, 16);
        w.write_u8(9);
    }
    assert_eq!(read(&w.into_bytes()), Err(BlockError::Repeated(TAG_TICKET)));
    // More sections than the limit.
    let mut w = BitWriter::new();
    w.write_u32(BLOCK_MAGIC);
    w.write_u8(MAX_SECTIONS as u8 + 1);
    w.write_bytes(&[0; 64]);
    assert!(matches!(
        read(&w.into_bytes()),
        Err(BlockError::Stream(NetError::Cap { .. }))
    ));
    // A manifest over its limit, and a ticket over its limit.
    for (tag, len) in [
        (TAG_MANIFEST, MAX_MANIFEST_BYTES + 1),
        (TAG_TICKET, MAX_TICKET_BYTES + 1),
    ] {
        let mut w = BitWriter::new();
        w.write_u32(BLOCK_MAGIC);
        w.write_u8(1);
        w.write_u8(tag);
        w.write_bits(len as u32, 16);
        w.write_bytes(&vec![0; len]);
        assert!(
            matches!(
                read(&w.into_bytes()),
                Err(BlockError::Stream(NetError::Cap { .. }))
            ),
            "tag {tag}"
        );
    }
    // A length that runs past the datagram.
    let mut w = BitWriter::new();
    w.write_u32(BLOCK_MAGIC);
    w.write_u8(1);
    w.write_u8(TAG_TICKET);
    w.write_bits(200, 16);
    w.write_bytes(&[1, 2]);
    assert!(read(&w.into_bytes()).is_err());
}

#[test]
fn a_manifest_of_a_format_this_build_does_not_read_says_so() {
    let mut w = BitWriter::new();
    w.write_u32(BLOCK_MAGIC);
    w.write_u8(1);
    w.write_u8(TAG_MANIFEST);
    w.write_bits(1, 16);
    w.write_u8(9);
    let err = read(&w.into_bytes()).unwrap_err();
    assert_eq!(
        err.to_string(),
        "manifest format 9 is not supported (this side speaks 1 and 2)"
    );
}

#[test]
fn a_block_that_cannot_be_written_fails_validation() {
    let long = ConnectBlock {
        manifest: None,
        ticket: Some(vec![0; MAX_TICKET_BYTES + 1]),
        ..Default::default()
    };
    assert_eq!(
        long.validate(),
        Err(BlockError::Ticket(MAX_TICKET_BYTES + 1))
    );
    let bad = ConnectBlock {
        manifest: Some(Manifest::new(1, 1).with_plugin("a\u{202e}b", 1)),
        ticket: None,
        ..Default::default()
    };
    assert!(matches!(bad.validate(), Err(BlockError::Manifest(_))));
    let fine = ConnectBlock {
        manifest: Some(server_manifest()),
        ticket: Some(b"x".to_vec()),
        ..Default::default()
    };
    assert_eq!(fine.validate(), Ok(()));
}

#[test]
fn a_ticket_over_the_limit_is_not_written() {
    let block = ConnectBlock {
        manifest: None,
        ticket: Some(vec![0; MAX_TICKET_BYTES + 1]),
        ..Default::default()
    };
    let mut w = BitWriter::new();
    assert_eq!(
        block.write(&mut w),
        Err(BlockError::Ticket(MAX_TICKET_BYTES + 1))
    );
    assert_eq!(w.bits(), 0, "nothing was written");
}

#[test]
fn the_gate_accepts_a_matching_manifest_and_refuses_a_different_one() {
    let gate = ManifestGate::new(server_manifest());
    let ok = fields(Some(ConnectBlock {
        manifest: Some(server_manifest()),
        ticket: None,
        ..Default::default()
    }));
    assert_eq!(gate.check(from(), &cx(), &ok), Ok(None));
    let old = fields(Some(ConnectBlock {
        manifest: Some(
            Manifest::new(12, 12)
                .with_plugin("torque.rope", 1)
                .with_ghosts(["Rope"]),
        ),
        ticket: None,
        ..Default::default()
    }));
    assert_eq!(
        gate.check(from(), &cx(), &old),
        Err(b"server needs plugin torque.rope 2, you have 1".to_vec())
    );
}

#[test]
fn a_client_with_no_block_is_judged_as_one_with_nothing() {
    let gate = ManifestGate::new(server_manifest());
    assert_eq!(
        gate.check(from(), &cx(), &fields(None)),
        Err(b"server needs plugin torque.rope 2, you do not have it".to_vec())
    );
    // A server that needs nothing takes a plain client.
    let plain = ManifestGate::new(Manifest::new(12, 12));
    assert_eq!(plain.check(from(), &cx(), &fields(None)), Ok(None));
}

#[test]
fn the_ticket_is_checked_after_the_manifest() {
    let key = [3; 16];
    let gate = ManifestGate::new(server_manifest()).with_auth(Arc::new(SharedSecret::new(key, "")));
    let signed = SharedSecret::new(key, "ada").ticket(&cx()).unwrap();
    let with = |manifest: Manifest, ticket: Option<Vec<u8>>| {
        fields(Some(ConnectBlock {
            manifest: Some(manifest),
            ticket,
            ..Default::default()
        }))
    };
    assert_eq!(
        gate.check(
            from(),
            &cx(),
            &with(server_manifest(), Some(signed.clone()))
        ),
        Ok(Some(Identity::new("ada").unwrap()))
    );
    // A bad build is refused for its build even with a good ticket.
    let reason = gate
        .check(from(), &cx(), &with(Manifest::new(12, 12), Some(signed)))
        .unwrap_err();
    assert!(reason.starts_with(b"server needs plugin"));
    // A good build with no ticket, or a forged one.
    assert_eq!(
        gate.check(from(), &cx(), &with(server_manifest(), None)),
        Err(b"sign in to join this server".to_vec())
    );
    let forged = SharedSecret::new([4; 16], "ada").ticket(&cx()).unwrap();
    assert_eq!(
        gate.check(from(), &cx(), &with(server_manifest(), Some(forged))),
        Err(b"your sign-in was not accepted".to_vec())
    );
}

#[test]
fn a_reason_is_cut_at_a_character_boundary() {
    let text = "é".repeat(200);
    let cut = reason(&text);
    assert!(cut.len() <= NET_STRING_BYTES);
    assert!(String::from_utf8(cut).is_ok());
    assert_eq!(reason("short"), b"short");
}

#[test]
fn the_shared_secret_without_a_name_has_no_ticket() {
    assert_eq!(
        SharedSecret::new([1; 16], "").ticket(&cx()),
        Err("not signed in".to_string())
    );
}

#[test]
fn a_ticket_is_good_for_the_server_and_the_connection_it_was_made_for() {
    let key = [3; 16];
    let gate = ManifestGate::new(server_manifest()).with_auth(Arc::new(SharedSecret::new(key, "")));
    let ticket = SharedSecret::new(key, "ada").ticket(&cx()).unwrap();
    let judged = |at: &AuthContext| {
        let block = ConnectBlock {
            manifest: Some(server_manifest()),
            ticket: Some(ticket.clone()),
            ..Default::default()
        };
        gate.check(from(), at, &fields(Some(block)))
    };
    assert_eq!(judged(&cx()), Ok(Some(Identity::new("ada").unwrap())));
    let refused = Err(b"your sign-in was not accepted".to_vec());
    // Another server (another host, or the same host on another socket), another challenge, another sequence.
    let other = |change: fn(&mut AuthContext)| {
        let mut other = cx();
        change(&mut other);
        other
    };
    assert_eq!(
        judged(&other(
            |c| c.server = SocketAddr::from(([192, 0, 2, 2], 28_000))
        )),
        refused
    );
    assert_eq!(
        judged(&other(
            |c| c.server = SocketAddr::from(([192, 0, 2, 1], 28_001))
        )),
        refused
    );
    assert_eq!(judged(&other(|c| c.challenge[3] ^= 1)), refused);
    assert_eq!(judged(&other(|c| c.sequence += 1)), refused);
}

#[test]
fn a_server_that_does_not_know_its_own_address_checks_no_tickets() {
    let key = [3; 16];
    let unknown = AuthContext {
        server: SocketAddr::from(([0, 0, 0, 0], 0)),
        ..cx()
    };
    assert!(unknown.server_unknown());
    let ticket = SharedSecret::new(key, "ada").ticket(&unknown).unwrap();
    assert_eq!(
        SharedSecret::new(key, "").verify(from(), &unknown, &ticket),
        Err("this server cannot check sign-ins (it does not know its own address)".to_string())
    );
}

#[test]
fn contexts_that_differ_never_encode_alike() {
    let base = cx();
    let v6 = AuthContext {
        server: SocketAddr::from(([0u16, 0, 0, 0, 0, 0xffff, 0xc000, 0x201], 28_000)),
        ..base
    };
    assert_ne!(base.encode(), v6.encode());
    assert_eq!(base.encode(), cx().encode());
}

#[test]
fn a_block_with_a_mods_section_reads_back() {
    use crate::modsync::ModsSection;
    let with = ConnectBlock {
        manifest: Some(server_manifest()),
        ticket: None,
        mods: Some(ModsSection {
            max_bytes: 1 << 20,
            max_files: 50,
        }),
    };
    assert_eq!(read(&bytes(&with)).unwrap(), Some(with));
}

#[test]
fn a_block_without_a_mods_section_is_the_bytes_it_always_was() {
    // Golden bytes: the magic, the section count, then each section as a tag, a 16-bit length and its bytes. A client of an engine
    // that shares no mods writes exactly this, so a server that never heard of the mods section reads it as before.
    let ticket_only = ConnectBlock {
        manifest: None,
        ticket: Some(b"abc".to_vec()),
        mods: None,
    };
    assert_eq!(
        bytes(&ticket_only),
        [0x31, 0x54, 0x46, 0x4d, 1, 2, 3, 0, b'a', b'b', b'c']
    );
    assert_eq!(bytes(&ConnectBlock::default()), [0x31, 0x54, 0x46, 0x4d, 0]);
    let manifest = server_manifest().encode().unwrap();
    let both = ConnectBlock {
        manifest: Some(server_manifest()),
        ticket: Some(b"t".to_vec()),
        mods: None,
    };
    let mut want = vec![0x31, 0x54, 0x46, 0x4d, 2, 1];
    want.extend_from_slice(&(manifest.len() as u16).to_le_bytes());
    want.extend_from_slice(&manifest);
    want.extend_from_slice(&[2, 1, 0, b't']);
    assert_eq!(bytes(&both), want);
    // With the section it is those bytes and one more section after them.
    let with = ConnectBlock {
        mods: Some(crate::modsync::ModsSection::default()),
        ..both
    };
    let written = bytes(&with);
    assert_eq!(written[4], 3);
    assert_eq!(&written[5..want.len()], &want[5..]);
}

#[test]
fn the_gate_asks_only_that_a_client_can_take_the_mods_a_server_requires() {
    use crate::modsync::{ModsSection, RequiredMod, RequiredMods};
    let required = RequiredMods::default();
    let gate = ManifestGate::new(server_manifest()).with_required(required.clone());
    let client = |mods: Option<ModsSection>| {
        fields(Some(ConnectBlock {
            manifest: Some(server_manifest()),
            ticket: None,
            mods,
        }))
    };
    // No mods required: every client of this build passes, as before.
    assert_eq!(gate.check(from(), &cx(), &client(None)), Ok(None));
    required.set(vec![RequiredMod {
        id: "props".into(),
        version: "1.2".into(),
        hash: [9; 32],
        bytes: 4_096,
        files: 2,
        download: true,
    }]);
    // The server no longer asks what the client holds: a client with the section is accepted whatever it has.
    let can = client(Some(ModsSection {
        max_bytes: 0,
        max_files: 0,
    }));
    assert_eq!(gate.check(from(), &cx(), &can), Ok(None));
    assert_eq!(
        gate.mods_support(&can),
        Some(ModsSection {
            max_bytes: 0,
            max_files: 0
        })
    );
    // One that cannot take mods at all is told so.
    assert_eq!(
        gate.check(from(), &cx(), &client(None)),
        Err(b"server requires mods, and your client cannot download them".to_vec())
    );
    assert_eq!(gate.mods_support(&client(None)), None);
}

#[test]
fn an_identity_is_its_id_and_the_name_is_only_for_display() {
    let a = Identity::new("ada").unwrap().with_name("Ada L.");
    let b = Identity::new("ada").unwrap().with_name("someone else");
    assert_eq!(a, b);
    assert_ne!(a, Identity::new("adb").unwrap());
    assert_eq!(a.id(), b"ada");
    assert_eq!(a.name(), Some("Ada L."));
    assert_eq!(Identity::new("ada").unwrap().name(), None);
}

#[test]
fn an_identity_needs_an_id_of_a_sane_size() {
    assert_eq!(Identity::new(""), Err(IdentityError::Empty));
    assert!(Identity::new(vec![1; MAX_IDENTITY_BYTES]).is_ok());
    assert_eq!(
        Identity::new(vec![1; MAX_IDENTITY_BYTES + 1]),
        Err(IdentityError::TooLong(MAX_IDENTITY_BYTES + 1))
    );
}

#[test]
fn a_display_name_loses_control_characters_and_is_cut_on_a_character() {
    let id = || Identity::new("x").unwrap();
    assert_eq!(
        id().with_name("a\nb\u{0}c\u{202e}d\u{2066}").name(),
        Some("abcd")
    );
    assert_eq!(id().with_name("\n\t").name(), None);
    let long = "é".repeat(MAX_IDENTITY_NAME_BYTES);
    let cut = id().with_name(&long);
    let name = cut.name().unwrap();
    assert_eq!(name.len(), MAX_IDENTITY_NAME_BYTES);
    assert!(name.chars().all(|c| c == 'é'));
}
