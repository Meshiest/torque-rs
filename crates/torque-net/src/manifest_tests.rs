use super::*;

fn server() -> Manifest {
    Manifest::new(12, 13)
        .with_plugin("torque.rope", 2)
        .with_plugin("example.arena", 1)
        .with_ghosts(["Rope", "Arena"])
        .with_datablocks(["RopeData"])
        .with_channels(["terrain"])
        .with_mod("pack", [7; 32])
}

fn round_trip(m: &Manifest) -> Manifest {
    Manifest::decode(&m.encode().expect("encodes")).expect("decodes")
}

#[test]
fn a_manifest_reads_back_as_it_was_written() {
    let m = server();
    assert_eq!(round_trip(&m), m);
    // Plugins are kept sorted by id whatever the order they were added in.
    let ids: Vec<_> = m.plugins.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["example.arena", "torque.rope"]);
    assert_eq!(m.ghosts.names().unwrap(), ["Arena", "Rope"]);
}

#[test]
fn a_bare_manifest_is_small_and_round_trips() {
    let m = Manifest::new(12, 12);
    assert!(m.is_bare());
    let bytes = m.encode().unwrap();
    assert!(bytes.len() < 64, "{} bytes", bytes.len());
    assert_eq!(Manifest::decode(&bytes).unwrap(), m);
}

#[test]
fn one_manifest_has_one_encoding_whatever_order_it_was_built_in() {
    let a = Manifest::new(1, 1)
        .with_plugin("b", 1)
        .with_plugin("a", 2)
        .with_ghosts(["y", "x", "y"]);
    let b = Manifest::new(1, 1)
        .with_ghosts(["x", "y"])
        .with_plugin("a", 2)
        .with_plugin("b", 1);
    assert_eq!(a.encode().unwrap(), b.encode().unwrap());
}

#[test]
fn a_large_table_travels_as_its_count_and_digest() {
    let names: Vec<String> = (0..400).map(|i| format!("Class{i:04}")).collect();
    let m = Manifest::new(1, 1).with_ghosts(&names);
    assert_eq!(m.ghosts.count(), 400);
    assert!(m.ghosts.names().is_none(), "over the listing limit");
    let back = round_trip(&m);
    assert!(back.ghosts.same_as(&m.ghosts));
    // A table that lists but would not fit with the others is left unlisted, and the rest still are.
    let near: Vec<String> = (0..20)
        .map(|i| format!("a-name-of-some-length-{i:03}"))
        .collect();
    let m = Manifest::new(1, 1)
        .with_ghosts(&near)
        .with_datablocks(&near)
        .with_channels(["terrain"]);
    let bytes = m.encode().unwrap();
    assert!(bytes.len() <= MAX_MANIFEST_BYTES);
    let back = Manifest::decode(&bytes).unwrap();
    assert!(back.ghosts.names().is_some());
    assert!(back.datablocks.names().is_none());
    assert_eq!(back.channels.names().unwrap(), ["terrain"]);
    assert!(back.datablocks.same_as(&m.datablocks));
}

#[test]
fn a_manifest_that_cannot_fit_is_refused_to_write() {
    let mut m = Manifest::new(1, 1);
    for i in 0..MAX_PLUGINS {
        m = m.with_plugin(
            format!("a.very.long.plugin.identifier.that.fills.the.limit.{i:02}"),
            1,
        );
    }
    assert!(matches!(m.encode(), Err(ManifestError::TooLarge(_))));
    let m = (0..=MAX_PLUGINS).fold(Manifest::new(1, 1), |m, i| {
        m.with_plugin(format!("p{i}"), 1)
    });
    assert_eq!(m.encode(), Err(ManifestError::Invalid("too many plugins")));
    let m = Manifest::new(1, 1).with_plugin("bad\nid", 1);
    assert_eq!(m.encode(), Err(ManifestError::Invalid("plugin id")));
    let m = Manifest::new(1, 1).with_plugin("x".repeat(MAX_NAME_BYTES + 1), 1);
    assert_eq!(m.encode(), Err(ManifestError::Invalid("plugin id")));
}

#[test]
fn names_with_bidi_or_format_characters_are_refused() {
    for bad in [
        "rope\u{202e}gnp",
        "zero\u{200b}width",
        "iso\u{2066}late",
        "bom\u{feff}",
        "soft\u{ad}hyphen",
        "line\u{2028}break",
        "tag\u{e0041}",
    ] {
        let m = Manifest::new(1, 1).with_plugin(bad, 1);
        assert_eq!(
            m.encode(),
            Err(ManifestError::Invalid("plugin id")),
            "{bad:?}"
        );
        // A table that holds one is only counted, never listed, and a name with one off the wire is refused.
        assert!(NameTable::new([bad]).names().is_none(), "{bad:?}");
        let mut w = BitWriter::new();
        write_text(&mut w, bad);
        assert_eq!(
            read_text(&mut BitReader::new(&w.into_bytes())),
            Err(ManifestError::Invalid("name text")),
            "{bad:?}"
        );
    }
    // Ordinary non-ASCII names are fine.
    assert!(
        Manifest::new(1, 1)
            .with_plugin("seil.\u{fc}bung", 1)
            .encode()
            .is_ok()
    );
}

#[test]
fn garbled_manifests_are_errors() {
    let good = server().encode().unwrap();
    assert!(matches!(Manifest::decode(&[]), Err(ManifestError::Cap(_))));
    // An unknown format.
    let mut bytes = good.clone();
    bytes[0] = 9;
    assert_eq!(Manifest::decode(&bytes), Err(ManifestError::Format(9)));
    // Every truncation fails cleanly.
    for len in 0..good.len() {
        assert!(Manifest::decode(&good[..len]).is_err(), "cut at {len}");
    }
    // Trailing bytes.
    let mut bytes = good.clone();
    bytes.extend_from_slice(&[0, 0]);
    assert_eq!(
        Manifest::decode(&bytes),
        Err(ManifestError::Invalid("trailing bytes"))
    );
    // More than the limit is refused unread.
    assert!(matches!(
        Manifest::decode(&vec![1; MAX_MANIFEST_BYTES + 1]),
        Err(ManifestError::TooLarge(_))
    ));
}

#[test]
fn a_plugin_count_the_bytes_cannot_hold_is_refused() {
    let mut w = BitWriter::new();
    w.write_u8(MANIFEST_FORMAT);
    w.write_u32(1);
    w.write_u32(1);
    w.write_u8(MAX_PLUGINS as u8 + 1);
    let err = Manifest::decode(&w.into_bytes()).unwrap_err();
    assert!(
        matches!(err, ManifestError::Cap(NetError::Cap { .. })),
        "{err:?}"
    );
    let mut w = BitWriter::new();
    w.write_u8(MANIFEST_FORMAT);
    w.write_u32(1);
    w.write_u32(1);
    w.write_u8(MAX_PLUGINS as u8);
    let err = Manifest::decode(&w.into_bytes()).unwrap_err();
    assert!(matches!(err, ManifestError::Cap(_)), "{err:?}");
}

#[test]
fn unsorted_or_inconsistent_lists_are_refused() {
    // Plugins out of order.
    let mut w = BitWriter::new();
    w.write_u8(MANIFEST_FORMAT);
    w.write_u32(1);
    w.write_u32(1);
    w.write_u8(2);
    for id in ["b", "a"] {
        write_text(&mut w, id);
        w.write_u32(1);
    }
    for _ in 0..3 {
        w.write_u32(0);
        w.write_u32(0);
        w.write_u32(0);
        w.write_flag(false);
    }
    w.write_u8(0);
    assert_eq!(
        Manifest::decode(&w.into_bytes()),
        Err(ManifestError::Invalid("plugin order"))
    );
    // A listed table whose digest is not its names'.
    let bytes = Manifest::new(1, 1).with_ghosts(["x"]).encode().unwrap();
    let mut forged = bytes.clone();
    // format(1) + min(4) + max(4) + plugins(1) + count(4) = 14: the digest's first byte.
    forged[14] ^= 0xff;
    assert_eq!(
        Manifest::decode(&forged),
        Err(ManifestError::Invalid("name digest"))
    );
    // A claimed count above the table limit.
    let mut big = bytes;
    big[10..14].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(Manifest::decode(&big), Err(ManifestError::Cap(_))));
}

#[test]
fn matching_manifests_have_no_mismatch() {
    assert_eq!(server().mismatch(&server()), None);
    // A client may have more mods than the server requires.
    let client = server().with_mod("another", [1; 32]);
    assert_eq!(server().mismatch(&client), None);
}

fn reason(server: &Manifest, client: &Manifest) -> String {
    server.mismatch(client).expect("a mismatch").to_string()
}

#[test]
fn a_missing_plugin_is_named_with_the_version_the_server_needs() {
    let client = Manifest::new(12, 13).with_ghosts(["Rope", "Arena"]);
    assert_eq!(
        reason(&server(), &client),
        "server needs plugin example.arena 1, you do not have it"
    );
}

#[test]
fn a_different_plugin_version_names_both() {
    let client = Manifest::new(12, 13)
        .with_plugin("example.arena", 1)
        .with_plugin("torque.rope", 1);
    assert_eq!(
        reason(&server(), &client),
        "server needs plugin torque.rope 2, you have 1"
    );
}

#[test]
fn a_networked_plugin_the_server_lacks_is_a_mismatch() {
    let client = server().with_plugin("zzz.extra", 3);
    assert_eq!(
        reason(&server(), &client),
        "you have plugin zzz.extra 3, which this server does not"
    );
}

#[test]
fn protocol_ranges_must_meet() {
    let client = Manifest::new(14, 15);
    assert_eq!(
        reason(&server(), &client),
        "server speaks protocol 12 to 13, you speak 14 to 15"
    );
    assert!(
        Manifest::new(10, 12)
            .mismatch(&Manifest::new(12, 13))
            .is_none()
    );
}

#[test]
fn a_name_table_that_differs_points_at_the_name() {
    let base = || {
        Manifest::new(12, 13)
            .with_plugin("example.arena", 1)
            .with_plugin("torque.rope", 2)
            .with_mod("pack", [7; 32])
    };
    let s = base()
        .with_ghosts(["Rope", "Arena"])
        .with_datablocks(["RopeData"])
        .with_channels(["terrain"]);
    let lacks = base()
        .with_ghosts(["Arena"])
        .with_datablocks(["RopeData"])
        .with_channels(["terrain"]);
    assert_eq!(
        reason(&s, &lacks),
        "server has ghost class Rope, you do not"
    );
    let extra = base()
        .with_ghosts(["Arena", "Rope", "Zip"])
        .with_datablocks(["RopeData"])
        .with_channels(["terrain"]);
    assert_eq!(
        reason(&s, &extra),
        "you have ghost class Zip, which this server does not"
    );
    let blocks = base()
        .with_ghosts(["Arena", "Rope"])
        .with_datablocks(["RopeData", "Other"])
        .with_channels(["terrain"]);
    assert_eq!(
        reason(&s, &blocks),
        "you have datablock class Other, which this server does not"
    );
    let channels = base()
        .with_ghosts(["Arena", "Rope"])
        .with_datablocks(["RopeData"]);
    assert_eq!(
        reason(&s, &channels),
        "server has channel terrain, you do not"
    );
}

#[test]
fn tables_without_names_differ_by_count() {
    let names: Vec<String> = (0..300).map(|i| format!("n{i}")).collect();
    let s = Manifest::new(1, 1).with_ghosts(&names);
    let c = Manifest::new(1, 1).with_ghosts(&names[..299]);
    assert_eq!(
        reason(&s, &c),
        "ghost class tables differ (server 300, you 299)"
    );
    // The same count with one name changed is still caught, by the digest.
    let mut other = names.clone();
    other[0] = "renamed".into();
    let c = Manifest::new(1, 1).with_ghosts(&other);
    assert_eq!(
        reason(&s, &c),
        "ghost class tables differ (server 300, you 300)"
    );
}

#[test]
fn a_mod_the_client_lacks_or_changed_is_a_mismatch() {
    let base = || {
        Manifest::new(12, 13)
            .with_plugin("example.arena", 1)
            .with_plugin("torque.rope", 2)
            .with_ghosts(["Rope", "Arena"])
            .with_datablocks(["RopeData"])
            .with_channels(["terrain"])
    };
    assert_eq!(
        reason(&server(), &base()),
        "server needs mod pack, you do not have it"
    );
    assert_eq!(
        reason(&server(), &base().with_mod("pack", [8; 32])),
        "your mod pack is not the server's"
    );
}

#[test]
fn the_checks_run_protocol_then_plugins_then_tables_then_mods() {
    let client = Manifest::new(14, 15);
    assert!(matches!(
        server().mismatch(&client),
        Some(Mismatch::Protocol { .. })
    ));
    let client = Manifest::new(12, 13);
    assert!(matches!(
        server().mismatch(&client),
        Some(Mismatch::PluginMissing { .. })
    ));
}

#[test]
fn a_manifest_with_rpcs_is_format_two_and_one_without_stays_format_one() {
    let plain = server();
    assert_eq!(plain.encode().unwrap()[0], MANIFEST_FORMAT);
    let with = server().with_rpcs([
        rpc_entry("props.spawn", true, 0xAB),
        rpc_entry("props.spawned", false, 0xCD),
    ]);
    let bytes = with.encode().unwrap();
    assert_eq!(bytes[0], MANIFEST_FORMAT_RPC);
    assert_eq!(Manifest::decode(&bytes).unwrap(), with);
    assert!(!with.is_bare());
    // The RPC table follows the mods: everything before it is what the peer wrote before RPCs existed (but for the format byte and
    // the padding of the last byte).
    let plain_bytes = plain.encode().unwrap();
    let shared = plain_bytes.len() - 1;
    assert_eq!(&bytes[1..shared], &plain_bytes[1..shared]);
}

#[test]
fn a_format_two_manifest_with_no_rpcs_is_refused() {
    let mut bytes = server().encode().unwrap();
    bytes[0] = MANIFEST_FORMAT_RPC;
    // The table that format 2 promises is missing.
    assert!(Manifest::decode(&bytes).is_err());
}

#[test]
fn a_differing_rpc_is_named_without_its_digest() {
    let ours = server().with_rpcs([rpc_entry("props.spawn", true, 1)]);
    let theirs = server().with_rpcs([rpc_entry("props.spawn", true, 2)]);
    assert_eq!(
        reason(&ours, &theirs),
        "server and client disagree on RPC props.spawn"
    );
    // A client with no RPCs at all, and one with an extra.
    assert_eq!(
        reason(&ours, &server()),
        "server and client disagree on RPC props.spawn"
    );
    let more = server().with_rpcs([
        rpc_entry("props.spawn", true, 1),
        rpc_entry("props.extra", false, 3),
    ]);
    assert_eq!(
        reason(&ours, &more),
        "server and client disagree on RPC props.extra"
    );
    assert!(ours.mismatch(&ours).is_none());
}
