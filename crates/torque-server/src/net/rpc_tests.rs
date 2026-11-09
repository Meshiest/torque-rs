use std::collections::HashMap;

use torque_engine::permissions::SubjectKey;
use torque_engine::rpc::RpcHooks;
use torque_net::handshake::ConnId;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use super::*;

/// A host that knows the connections it was given and nothing else.
struct Host(HashMap<u32, ObjId>);

impl RpcServerHost for Host {
    fn ghost_bits(&self, _conn: ConnId) -> u8 {
        6
    }

    fn object_of(&self, conn: ConnId) -> Option<ObjId> {
        self.0.get(&conn.0).copied()
    }

    fn conn_of(&self, obj: ObjId) -> Option<ConnId> {
        self.0
            .iter()
            .find(|(_, o)| **o == obj)
            .map(|(c, _)| ConnId(*c))
    }

    fn ghost_object(&self, _conn: ConnId, _index: u32) -> Option<ObjId> {
        None
    }

    fn ghost_index(&self, _conn: ConnId, _object: ObjId) -> Option<u32> {
        None
    }

    fn subject_key(&self, conn: ConnId) -> SubjectKey {
        SubjectKey::Connection(conn.0)
    }

    fn drop_conn(&self, _vm: &mut Vm, _conn: ConnId, _reason: &[u8]) {}
}

fn host() -> Host {
    Host(HashMap::from([(1, 4242)]))
}

fn define(vm: &mut Vm, code: &str) {
    vm.eval(code.as_bytes()).expect("the script defines it");
}

#[test]
fn a_call_with_no_caller_object_is_refused_even_with_no_on_rpc_check() {
    let host = host();
    let mut hooks = ScriptHooks(&host);
    let mut vm = Vm::new();
    assert!(
        !hooks.allow(&mut vm, Some(ConnId(9)), "chat.say"),
        "a connection with no object"
    );
    assert!(
        !hooks.allow(&mut vm, None, "chat.say"),
        "no connection at all"
    );
    // The same refusal when a script handler is the target: it is never run without `%client`.
    define(
        &mut vm,
        "function rpcServer_chat_note(%client, %text) { $Test::ran = 1; }",
    );
    let refused = hooks.script(
        &mut vm,
        Some(ConnId(9)),
        "rpcServer_chat_note",
        vec![b"x".to_vec()],
    );
    assert!(refused.is_err());
    assert!(
        hooks
            .script(&mut vm, None, "rpcServer_chat_note", vec![])
            .is_err()
    );
    assert!(vm.global("$Test::ran").is_none(), "the handler never ran");
    assert!(
        hooks
            .script(
                &mut vm,
                Some(ConnId(1)),
                "rpcServer_chat_note",
                vec![b"x".to_vec()]
            )
            .is_ok()
    );
    assert_eq!(vm.global("$Test::ran").as_deref(), Some(&b"1"[..]));
}

#[test]
fn a_game_with_no_on_rpc_check_leaves_the_decision_to_allow() {
    let host = host();
    let mut hooks = ScriptHooks(&host);
    let mut vm = Vm::new();
    assert!(hooks.allow(&mut vm, Some(ConnId(1)), "chat.say"));
}

#[test]
fn on_rpc_check_allows_only_a_true_answer() {
    let host = host();
    let mut hooks = ScriptHooks(&host);
    let cases = [
        ("return 1;", true),
        ("return true;", true),
        ("return \"1\";", true),
        ("return 0;", false),
        ("return false;", false),
        ("return \"\";", false),
        ("return;", false),
        ("", false),
        ("return \"no\";", false),
        // A function that fails (it recurses until the script engine stops it) denies.
        ("return onRpcCheck(%client, %name);", false),
    ];
    for (body, allowed) in cases {
        let mut vm = Vm::new();
        define(
            &mut vm,
            &format!("function onRpcCheck(%client, %name) {{ {body} }}"),
        );
        assert_eq!(
            hooks.allow(&mut vm, Some(ConnId(1)), "chat.say"),
            allowed,
            "`{body}`"
        );
    }
}

#[test]
fn on_rpc_check_is_told_the_client_and_the_rpc() {
    let host = host();
    let mut hooks = ScriptHooks(&host);
    let mut vm = Vm::new();
    define(
        &mut vm,
        "function onRpcCheck(%client, %name) { $Test::seen = %client SPC %name; return 1; }",
    );
    assert!(hooks.allow(&mut vm, Some(ConnId(1)), "chat.say"));
    assert_eq!(
        vm.global("$Test::seen").as_deref(),
        Some(&b"4242 chat.say"[..])
    );
}
