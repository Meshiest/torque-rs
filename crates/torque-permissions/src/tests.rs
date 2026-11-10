use super::*;

fn p(text: &str) -> Permission {
    Permission::parse(text).expect("a permission")
}

fn g(text: &str) -> Grant {
    Grant::parse(text).expect("a grant")
}

#[test]
fn exact_and_wildcard_grants_match_what_they_say() {
    let cases = [
        ("props.spawn", "props.spawn", true),
        ("props.spawn", "props.delete", false),
        ("props.spawn", "props", false),
        ("props.spawn", "props.spawn.big", false),
        ("props.*", "props.spawn", true),
        ("props.*", "props.spawn.big", true),
        ("props.*", "props", false),
        ("props.*", "properties.spawn", false),
        ("props.spawn.*", "props.spawn", false),
        ("props.spawn.*", "props.spawn.big", true),
        ("*", "props", true),
        ("*", "a.b.c.d", true),
    ];
    for (grant, permission, expected) in cases {
        assert_eq!(
            g(grant).matches(&p(permission)),
            expected,
            "{grant} vs {permission}"
        );
    }
}

#[test]
fn names_and_grants_are_checked() {
    let long = "a".repeat(MAX_SEGMENT_BYTES);
    let too_long = "a".repeat(MAX_SEGMENT_BYTES + 1);
    let eight = ["a"; MAX_SEGMENTS].join(".");
    let nine = ["a"; MAX_SEGMENTS + 1].join(".");
    for ok in ["a", "props.spawn", "rpc.example.chat-room_2", &long, &eight] {
        assert!(Permission::parse(ok).is_ok(), "{ok}");
    }
    for bad in [
        "", ".", "a.", ".a", "a..b", "A", "a b", "a/b", "é", &too_long, &nine, "*", "a.*", "a*",
    ] {
        assert!(
            Permission::parse(bad).is_err(),
            "{bad:?} is not a permission"
        );
    }
    for ok in ["*", "a.*", &eight, &format!("{}.*", ["a"; 7].join("."))] {
        assert!(Grant::parse(ok).is_ok(), "{ok}");
    }
    for bad in [
        "",
        "*.a",
        "a.*.b",
        "a*",
        "*a",
        "**",
        "a.**",
        &nine,
        &format!("{}.*", eight),
    ] {
        assert!(Grant::parse(bad).is_err(), "{bad:?} is not a grant");
    }
    assert_eq!(Grant::parse("a.*.b"), Err(PermissionError::Wildcard));
}

#[test]
fn roles_are_flat_named_sets_the_game_defines() {
    let mut roles = Roles::default();
    assert!(roles.is_empty(), "the model defines none");
    roles.define("moderator", &["chat.*"]).unwrap();
    roles.define("admin", &["chat.*", "server.*"]).unwrap();
    let mut player = Subject::default();
    assert!(player.add_role(&roles, "admin").unwrap());
    assert!(!player.add_role(&roles, "admin").unwrap(), "already held");
    assert!(player.check_text(&roles, "server.restart"));
    // No inheritance: holding `admin` does not give `moderator`.
    assert!(!player.has_role("moderator"));
    assert_eq!(
        player.add_role(&roles, "ghost"),
        Err(PermissionError::UnknownRole("ghost".into()))
    );
    assert!(player.remove_role("admin"));
    assert!(!player.check_text(&roles, "server.restart"));
    // A role redefined changes what its holders have; a removed one grants nothing.
    player.add_role(&roles, "moderator").unwrap();
    assert!(!player.check_text(&roles, "server.restart"));
    roles.define("moderator", &["server.*"]).unwrap();
    assert!(player.check_text(&roles, "server.restart"));
    // A role is emptied by defining it with nothing, not removed: it cannot come back to hand its old holders grants.
    roles.define("moderator", &[]).unwrap();
    assert!(player.has_role("moderator") && !player.check_text(&roles, "server.restart"));
    assert!(roles.grants("moderator").is_some_and(<[Grant]>::is_empty));
}

#[test]
fn direct_grants_and_role_grants_both_count() {
    let mut roles = Roles::default();
    roles
        .define_text("builder", "props.* tools.paint,tools.fill")
        .unwrap();
    let mut a = Subject::default();
    a.grant("chat.mute").unwrap();
    a.add_role(&roles, "builder").unwrap();
    for yes in ["chat.mute", "props.spawn", "tools.paint", "tools.fill"] {
        assert!(a.check_text(&roles, yes), "{yes}");
    }
    for no in ["chat.kick", "tools.erase", "props", "not a permission", ""] {
        assert!(!a.check_text(&roles, no), "{no}");
    }
    assert!(a.revoke("chat.mute") && !a.revoke("chat.mute") && !a.revoke("***"));
    assert!(!a.check_text(&roles, "chat.mute"));
    assert_eq!(a.roles().collect::<Vec<_>>(), ["builder"]);
}

#[test]
fn every_cap_refuses_with_an_error_and_changes_nothing() {
    let mut roles = Roles::default();
    for i in 0..MAX_ROLES {
        roles.define(&format!("r{i}"), &[]).unwrap();
    }
    assert_eq!(
        roles.define("one-more", &[]),
        Err(PermissionError::TooManyRoles)
    );
    roles.define("r0", &["a.*"]).unwrap(); // replacing one is not a new role
    assert_eq!(roles.len(), MAX_ROLES);

    let mut roles = Roles::default();
    let many: Vec<String> = (0..=MAX_GRANTS_PER_ROLE)
        .map(|i| format!("p.x{i}"))
        .collect();
    let refs: Vec<&str> = many.iter().map(String::as_str).collect();
    assert_eq!(
        roles.define("big", &refs),
        Err(PermissionError::TooManyRoleGrants)
    );
    assert!(roles.is_empty());
    roles.define("fits", &refs[..MAX_GRANTS_PER_ROLE]).unwrap();
    assert_eq!(
        roles.define("bad", &["ok", "no good"]),
        Err(PermissionError::Segment("no good".into()))
    );
    assert!(roles.grants("bad").is_none(), "a bad grant defines nothing");
    assert!(matches!(
        roles.define("Bad Name", &[]),
        Err(PermissionError::RoleName(_))
    ));
    assert!(matches!(
        roles.define("", &[]),
        Err(PermissionError::RoleName(_))
    ));

    let mut roles = Roles::default();
    for i in 0..=MAX_SUBJECT_ROLES {
        roles.define(&format!("r{i}"), &[]).unwrap();
    }
    let mut s = Subject::default();
    for i in 0..MAX_SUBJECT_ROLES {
        s.add_role(&roles, &format!("r{i}")).unwrap();
    }
    assert_eq!(
        s.add_role(&roles, &format!("r{MAX_SUBJECT_ROLES}")),
        Err(PermissionError::TooManySubjectRoles)
    );
    assert_eq!(s.roles().count(), MAX_SUBJECT_ROLES);
    for i in 0..MAX_SUBJECT_GRANTS {
        s.grant(&format!("g.x{i}")).unwrap();
    }
    assert_eq!(
        s.grant("g.over"),
        Err(PermissionError::TooManySubjectGrants)
    );
    assert_eq!(
        s.grant("g.x0"),
        Ok(false),
        "a grant already held is not a new one"
    );
    assert_eq!(s.grants().count(), MAX_SUBJECT_GRANTS);

    let mut registry = Registry::default();
    for i in 0..MAX_DECLARED {
        registry.declare(&format!("d.x{i}"), "d").unwrap();
    }
    assert_eq!(
        registry.declare("d.over", "d"),
        Err(PermissionError::TooManyDeclared)
    );
    registry.declare("d.x1", "again").unwrap();
    assert_eq!(registry.description(&p("d.x1")), Some("again"));
}

#[test]
fn the_registry_lists_what_plugins_declared() {
    let mut r = Registry::default();
    r.declare("props.spawn", "Spawn a prop").unwrap();
    r.declare("chat.mute", "Mute a player").unwrap();
    assert!(r.contains(&p("chat.mute")) && !r.contains(&p("chat.kick")));
    let listed: Vec<_> = r.iter().map(|(p, d)| (p.as_str(), d)).collect();
    assert_eq!(
        listed,
        [
            ("chat.mute", "Mute a player"),
            ("props.spawn", "Spawn a prop")
        ]
    );
    assert_eq!(r.declare("props.*", "x"), Err(PermissionError::Wildcard));
    assert_eq!(
        r.declare("a.b", "two\nlines"),
        Err(PermissionError::Description)
    );
    assert_eq!(
        r.declare("a.b", &"x".repeat(MAX_DESCRIPTION_BYTES + 1)),
        Err(PermissionError::Description)
    );
    assert_eq!(r.len(), 2);
}

/// A small deterministic generator.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// Random text into every parser: nothing panics, whatever parses re-parses as itself, and a grant that parses matches only
/// what its text says. `TORQUE_FUZZ_PERMISSIONS=1000000` runs a million rounds.
#[test]
fn the_parsers_never_panic_and_agree_with_themselves() {
    let rounds: u64 = std::env::var("TORQUE_FUZZ_PERMISSIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_000);
    let alphabet: Vec<char> = "ab.*_-09AZ é\n\0.*.*".chars().collect();
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let mut roles = Roles::default();
    roles.define("r", &["a.*"]).unwrap();
    let mut subject = Subject::default();
    let mut registry = Registry::default();
    for _ in 0..rounds {
        let len = (rng.next() % 40) as usize;
        let text: String = (0..len)
            .map(|_| alphabet[(rng.next() % alphabet.len() as u64) as usize])
            .collect();
        if let Ok(permission) = Permission::parse(&text) {
            assert_eq!(
                Permission::parse(permission.as_str()),
                Ok(permission.clone())
            );
            assert!(
                Grant::parse(&text).is_ok_and(|g| g.matches(&permission)),
                "a permission is its own grant: {text}"
            );
            let _ = registry.declare(&text, "x");
        }
        if let Ok(grant) = Grant::parse(&text) {
            assert_eq!(Grant::parse(grant.as_str()), Ok(grant.clone()));
            let _ = subject.grant(&text);
        }
        let _ = roles.define_text(&text, &text);
        let _ = subject.add_role(&roles, &text);
        let _ = subject.check_text(&roles, &text);
        subject.revoke(&text);
        assert!(subject.grants().count() <= MAX_SUBJECT_GRANTS);
        assert!(roles.len() <= MAX_ROLES && registry.len() <= MAX_DECLARED);
    }
}
