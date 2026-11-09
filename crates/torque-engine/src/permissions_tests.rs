use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::*;
use crate::{EngineBuilder, Plugin, PluginType};

struct Props;

impl PluginType for Props {
    const ID: PluginId = PluginId::new("example.props");
}

impl Plugin for Props {
    fn build(&self, engine: &mut EngineBuilder) {
        let mut perms = engine.permissions();
        perms.declare("props.spawn", "Spawn a prop");
        perms.declare("props.delete", "Delete a prop");
        perms.declare("chat.mute", "Mute a player");
        perms.role("builder", &["props.*"]);
        perms.role("janitor", &["props.delete"]);
    }
}

fn engine() -> Engine {
    EngineBuilder::new()
        .add_plugins(Props)
        .build()
        .expect("builds")
}

const CONN: SubjectKey = SubjectKey::Connection(3);

#[test]
fn plugins_declare_and_define_and_the_engine_defines_nothing() {
    let bare = Permissions::new(&EngineBuilder::new().build().unwrap()).unwrap();
    assert!(bare.role_names().is_empty() && bare.declared().is_empty());
    let perms = Permissions::new(&engine()).unwrap();
    assert_eq!(perms.role_names(), ["builder", "janitor"]);
    assert_eq!(
        perms.declared(),
        [
            ("chat.mute".to_string(), "Mute a player".to_string()),
            ("props.delete".to_string(), "Delete a prop".to_string()),
            ("props.spawn".to_string(), "Spawn a prop".to_string()),
        ]
    );
    assert_eq!(engine().declared_permissions().len(), 3);
}

#[test]
fn roles_and_grants_decide_a_check() {
    let perms = Permissions::new(&engine()).unwrap();
    assert!(!perms.check(&CONN, "props.spawn"));
    assert!(perms.add_role(&CONN, "builder").unwrap());
    assert!(perms.check(&CONN, "props.spawn") && perms.check(&CONN, "props.delete"));
    assert!(!perms.check(&CONN, "chat.mute"));
    perms.grant(&CONN, "chat.*").unwrap();
    assert!(perms.check(&CONN, "chat.mute"));
    assert_eq!(perms.roles_of(&CONN), ["builder"]);
    assert_eq!(perms.grants_of(&CONN), ["chat.*"]);
    assert!(perms.remove_role(&CONN, "builder") && perms.revoke(&CONN, "chat.*"));
    assert!(!perms.check(&CONN, "props.spawn") && !perms.check(&CONN, "chat.mute"));
    assert!(
        perms.add_role(&CONN, "nobody").is_err(),
        "an undefined role"
    );
    assert!(perms.grant(&CONN, "bad grant").is_err());
    assert!(!perms.check(&CONN, "not a permission"));
    // Another player's grants are not this one's.
    perms
        .add_role(&SubjectKey::Connection(4), "janitor")
        .unwrap();
    assert!(!perms.check(&CONN, "props.delete"));
}

#[test]
fn a_script_can_define_roles_and_declare_permissions_within_the_caps() {
    let perms = Permissions::new(&engine()).unwrap();
    perms.define_role("muter", "chat.mute").unwrap();
    perms.declare("sandbox.reset", "Reset the sandbox").unwrap();
    perms.add_role(&CONN, "muter").unwrap();
    assert!(perms.check(&CONN, "chat.mute"));
    assert!(perms.define_role("Bad", "x").is_err());
    assert!(perms.define_role("ok", "a.*.b").is_err());
    assert!(perms.declare("a.*", "x").is_err());
    for i in 0..torque_permissions::MAX_SUBJECT_ROLES {
        perms.define_role(&format!("r{i}"), "").unwrap();
        perms.add_role(&CONN, &format!("r{i}")).ok();
    }
    perms.define_role("over", "").unwrap();
    assert!(
        perms.add_role(&CONN, "over").is_err(),
        "a player holds at most 32 roles"
    );
    assert!(perms.roles_of(&CONN).len() <= torque_permissions::MAX_SUBJECT_ROLES);
}

#[test]
fn connection_keyed_grants_end_with_the_connection_and_identity_keyed_ones_do_not() {
    let perms = Permissions::new(&engine()).unwrap();
    let signed_in = SubjectKey::Identity(b"account-9".to_vec());
    perms.add_role(&CONN, "builder").unwrap();
    perms.add_role(&signed_in, "builder").unwrap();
    perms.forget_connection(3);
    assert!(!perms.check(&CONN, "props.spawn"));
    assert!(
        perms.check(&signed_in, "props.spawn"),
        "the provider keeps an identity's grants"
    );
}

/// A provider a game backs with its own storage.
#[derive(Default)]
struct Database {
    rows: Rc<RefCell<HashMap<String, Subject>>>,
    forgotten: Rc<RefCell<Vec<u32>>>,
}

fn name_of(key: &SubjectKey) -> String {
    match key {
        SubjectKey::Identity(id) => format!("id:{}", String::from_utf8_lossy(id)),
        SubjectKey::Connection(c) => format!("conn:{c}"),
    }
}

impl PermissionProvider for Database {
    fn load(&self, key: &SubjectKey) -> Subject {
        self.rows
            .borrow()
            .get(&name_of(key))
            .cloned()
            .unwrap_or_default()
    }

    fn store(&self, key: &SubjectKey, subject: Subject) {
        self.rows.borrow_mut().insert(name_of(key), subject);
    }

    fn forget_connection(&self, conn: u32) {
        self.forgotten.borrow_mut().push(conn);
    }
}

struct Backed(Rc<RefCell<HashMap<String, Subject>>>, Rc<RefCell<Vec<u32>>>);

impl PluginType for Backed {
    const ID: PluginId = PluginId::new("example.database");
}

impl Plugin for Backed {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.permissions().provider(Database {
            rows: Rc::clone(&self.0),
            forgotten: Rc::clone(&self.1),
        });
    }
}

#[test]
fn a_provider_swap_changes_where_grants_live() {
    let (rows, forgotten) = (Rc::default(), Rc::default());
    let engine = EngineBuilder::new()
        .add_plugins((Props, Backed(Rc::clone(&rows), Rc::clone(&forgotten))))
        .build()
        .expect("builds");
    let perms = Permissions::new(&engine).unwrap();
    let who = SubjectKey::Identity(b"alice".to_vec());
    perms.add_role(&who, "janitor").unwrap();
    assert!(perms.check(&who, "props.delete"));
    assert!(
        rows.borrow().contains_key("id:alice"),
        "the game's storage holds it"
    );
    // A second server of the engine reads the same storage; the default would have started empty.
    assert!(
        Permissions::new(&engine)
            .unwrap()
            .check(&who, "props.delete")
    );
    perms.forget_connection(5);
    assert_eq!(
        *forgotten.borrow(),
        [5],
        "the provider is told a connection ended"
    );
}

#[test]
fn a_second_provider_fails_the_build_and_bad_declarations_are_rejected() {
    struct Other;
    impl PluginType for Other {
        const ID: PluginId = PluginId::new("example.other");
    }
    impl Plugin for Other {
        fn build(&self, engine: &mut EngineBuilder) {
            engine.permissions().provider(Database::default());
        }
    }
    let twice = EngineBuilder::new()
        .add_plugins((Backed(Rc::default(), Rc::default()), Other))
        .build();
    assert!(twice.is_err(), "one provider");

    struct Bad(fn(&mut EngineBuilder));
    impl PluginType for Bad {
        const ID: PluginId = PluginId::new("example.bad");
    }
    impl Plugin for Bad {
        fn build(&self, engine: &mut EngineBuilder) {
            (self.0)(engine);
        }
    }
    let build = |body: fn(&mut EngineBuilder)| {
        EngineBuilder::new()
            .add_plugins(Bad(body))
            .build()
            .map(|_| ())
    };
    assert!(
        build(|e| {
            e.permissions().declare("Props.Spawn", "x");
        })
        .is_err()
    );
    assert!(
        build(|e| {
            e.permissions().declare("props.*", "x");
        })
        .is_err()
    );
    assert!(
        build(|e| {
            e.permissions().role("Bad Role", &[]);
        })
        .is_err()
    );
    assert!(
        build(|e| {
            e.permissions().role("ok", &["a.*.b"]);
        })
        .is_err()
    );
    assert!(
        build(|e| {
            e.permissions().declare("props.spawn", "fine");
        })
        .is_ok()
    );
}

#[test]
fn checking_an_undeclared_permission_is_a_warning_in_every_build_and_never_a_panic() {
    let perms = Permissions::new(&engine()).unwrap();
    perms.add_role(&CONN, "builder").unwrap();
    assert!(
        perms.check(&CONN, "props.teleport"),
        "a wildcard still covers it"
    );
    assert!(!perms.check(&CONN, "chat.kick"));
    assert_eq!(
        perms.take_undeclared().names,
        ["chat.kick", "props.teleport"]
    );
    assert!(perms.take_undeclared().is_empty(), "reported once");
    // A name a peer made up is an answer too.
    assert!(!perms.check(&CONN, "Not A Permission"));
    assert!(perms.take_undeclared().is_empty());
}

#[test]
fn the_undeclared_names_kept_are_capped_with_a_count_of_the_rest() {
    let perms = Permissions::new(&engine()).unwrap();
    for i in 0..(MAX_UNDECLARED + 40) {
        perms.check(&CONN, &format!("made.up{i}"));
        perms.check(&CONN, &format!("made.up{i}"));
    }
    let report = perms.take_undeclared();
    assert_eq!(report.names.len(), MAX_UNDECLARED);
    assert_eq!(report.more, 80, "each check of a name past the cap counted");
    let lines = report.lines();
    assert_eq!(lines.len(), MAX_UNDECLARED + 1);
    assert_eq!(
        lines.last().unwrap(),
        "and 80 more checks of undeclared permissions"
    );
    assert!(perms.take_undeclared().is_empty());
}

#[test]
fn a_view_answers_for_one_player() {
    let perms = Permissions::new(&engine()).unwrap();
    perms.add_role(&CONN, "janitor").unwrap();
    let view = perms.view(CONN);
    assert!(view.has_permission("props.delete") && !view.has_permission("props.spawn"));
    assert!(!NoPermissions.has_permission("props.delete"));
}
