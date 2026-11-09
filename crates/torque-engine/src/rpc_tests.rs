use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use torque_ghosts::codec::NetWidths;
use torque_net::handshake::ConnId;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use super::link::Side;
use super::*;
use crate::permissions::{NoPermissions, PermissionView};
use crate::{EngineBuilder, NetMessage, Plugin, PluginId};

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
struct Echo {
    #[net(max_len = 32)]
    text: String,
    #[net(bits = 6)]
    count: u8,
}

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
struct Echoed {
    #[net(max_len = 40)]
    text: String,
}

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
struct Ping {
    #[net(finite)]
    at: [f32; 3],
}

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
struct Target {
    who: torque_ghosts::GhostRef,
}

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
struct Blob {
    #[net(max_len = 600)]
    bytes: Vec<u8>,
}

/// What the handlers of a test engine did.
#[derive(Default)]
struct Log {
    echoes: Cell<u32>,
    pings: RefCell<Vec<[f32; 3]>>,
    targets: Cell<u32>,
    notes: Cell<u32>,
    checked: RefCell<Vec<Option<u32>>>,
    looked: RefCell<Vec<Option<u32>>>,
}

#[derive(PluginId)]
#[plugin(id = "test.rpc")]
struct Suite {
    log: Rc<Log>,
}

impl Plugin for Suite {
    fn build(&self, engine: &mut EngineBuilder) {
        let log = Rc::clone(&self.log);
        engine
            .net()
            .rpc::<Echo, Echoed>("echo")
            .to_server()
            .rate(4, 8)
            .server(move |_cx: &mut RpcCx, req: Echo| {
                log.echoes.set(log.echoes.get() + 1);
                if req.text == "fail" {
                    return Err(RpcError::Handler("x".repeat(400)));
                }
                if req.text == "refuse" {
                    return Err(RpcError::Refused);
                }
                Ok(Echoed {
                    text: req.text.repeat(usize::from(req.count.max(1))),
                })
            });
        let log = Rc::clone(&self.log);
        engine.net().rpc::<Ping, ()>("ping").to_client().client(
            move |_cx: &mut RpcCx, req: Ping| {
                log.pings.borrow_mut().push(req.at);
                Ok(())
            },
        );
        let log = Rc::clone(&self.log);
        engine.net().rpc::<Target, ()>("target").to_server().server(
            move |cx: &mut RpcCx, req: Target| {
                log.targets.set(log.targets.get() + 1);
                log.checked.borrow_mut().push(cx.resolve(req.who));
                Ok(())
            },
        );
        let log = Rc::clone(&self.log);
        engine.net().rpc::<Target, ()>("look").to_client().client(
            move |cx: &mut RpcCx, req: Target| {
                log.looked.borrow_mut().push(cx.resolve(req.who));
                Ok(())
            },
        );
        let log = Rc::clone(&self.log);
        engine
            .net()
            .rpc::<Ping, ()>("note")
            .to_client()
            .delivery(Delivery::Unreliable)
            .client(move |_cx: &mut RpcCx, _req: Ping| {
                log.notes.set(log.notes.get() + 1);
                Ok(())
            });
        engine
            .net()
            .rpc::<Blob, ()>("blob")
            .to_server()
            .max_bytes(1024)
            .rate(1000, 1000)
            .server(|_cx: &mut RpcCx, _req: Blob| Ok(()));
        engine
            .net()
            .rpc::<Target, ()>("scripted")
            .to_server()
            .server_script();
        engine
            .net()
            .rpc::<Echo, ()>("small")
            .to_server()
            .max_bytes(8)
            .server(|_cx: &mut RpcCx, _req: Echo| Ok(()));
        engine
            .net()
            .rpc::<Echo, Echoed>("permitted")
            .to_server()
            .allow(Allow::permission("props.spawn"))
            .server(|_cx: &mut RpcCx, req: Echo| Ok(Echoed { text: req.text }));
        engine
            .net()
            .rpc::<Echo, Echoed>("defaulted")
            .to_server()
            .default_permission()
            .server(|_cx: &mut RpcCx, req: Echo| Ok(Echoed { text: req.text }));
        engine
            .net()
            .rpc::<Echo, Echoed>("owner")
            .to_server()
            .allow(Allow::check(|cx| {
                cx.caller() == Some(OWNER) || cx.has_permission("props.spawn")
            }))
            .server(|_cx: &mut RpcCx, req: Echo| Ok(Echoed { text: req.text }));
        engine
            .net()
            .rpc::<Echo, Echoed>("everyone")
            .to_server()
            .allow(Allow::Anyone)
            .server(|_cx: &mut RpcCx, req: Echo| Ok(Echoed { text: req.text }));
    }
}

/// What a test caller holds: the server's roles and the caller's subject.
#[derive(Default)]
struct Holder {
    roles: torque_permissions::Roles,
    subject: torque_permissions::Subject,
}

impl PermissionView for Holder {
    fn has_permission(&self, permission: &str) -> bool {
        self.subject.check_text(&self.roles, permission)
    }
}

const OWNER: ConnId = ConnId(1);
const STRANGER: ConnId = ConnId(2);

fn widths() -> NetWidths {
    NetWidths::engine_default()
}

fn engine() -> (crate::Engine, Rc<Log>) {
    let log = Rc::new(Log::default());
    let engine = EngineBuilder::new()
        .add_plugins(Suite {
            log: Rc::clone(&log),
        })
        .build()
        .expect("the engine builds");
    (engine, log)
}

/// A carrier that keeps what it is handed, with an unacked count a test sets.
#[derive(Default)]
struct Tap {
    sent: RefCell<Vec<(Vec<u8>, Delivery)>>,
    unacked: Cell<usize>,
    refuse: Cell<bool>,
    capacity: Cell<usize>,
}

impl RpcCarrier for Tap {
    fn unacked(&self) -> usize {
        self.unacked.get()
    }

    fn capacity(&self) -> usize {
        match self.capacity.get() {
            0 => DEFAULT_SEND_QUEUE,
            bytes => bytes,
        }
    }

    fn send(&self, batch: Vec<u8>, delivery: Delivery) -> Result<(), CarrierFull> {
        if self.refuse.get() {
            return Err(CarrierFull);
        }
        self.sent.borrow_mut().push((batch, delivery));
        Ok(())
    }
}

impl Tap {
    fn take(&self) -> Vec<(Vec<u8>, Delivery)> {
        std::mem::take(&mut *self.sent.borrow_mut())
    }
}

struct Ghosts(HashMap<u32, u32>);

impl GhostResolver for Ghosts {
    fn knows(&self, _conn: Option<ConnId>, index: u32) -> bool {
        self.0.contains_key(&index)
    }

    fn resolve(&self, _conn: Option<ConnId>, index: u32) -> Option<ObjId> {
        self.0.get(&index).copied()
    }

    fn index_of(&self, _conn: Option<ConnId>, object: ObjId) -> Option<u32> {
        self.0.iter().find(|(_, o)| **o == object).map(|(i, _)| *i)
    }
}

/// Both ends of one connection.
struct Pair {
    table: RpcTable,
    server: RpcLink,
    client: RpcLink,
    to_client: Rc<Tap>,
    to_server: Rc<Tap>,
    vm: Vm,
    tick: u64,
    ghosts: Ghosts,
    holder: Holder,
    caller: ConnId,
}

impl Pair {
    fn new(engine: &crate::Engine) -> Pair {
        Pair::as_caller(engine, OWNER, Holder::default())
    }

    fn as_caller(engine: &crate::Engine, caller: ConnId, holder: Holder) -> Pair {
        let table = engine.net_rpcs().expect("the table");
        let to_client = Rc::new(Tap::default());
        let to_server = Rc::new(Tap::default());
        let mut server = RpcLink::new(
            table.clone(),
            Side::Server,
            Some(caller),
            widths(),
            Rc::clone(&to_client) as Rc<dyn RpcCarrier>,
            0,
        );
        server.set_connection_rate(1_000_000, 1_000_000);
        let mut client = RpcLink::new(
            table.clone(),
            Side::Client,
            None,
            widths(),
            Rc::clone(&to_server) as Rc<dyn RpcCarrier>,
            0,
        );
        // The tests of the receiving side send what an honest peer would hold back; the sending limits have tests of their own.
        server.set_send_limits(false);
        client.set_send_limits(false);
        Pair {
            table,
            server,
            client,
            to_client,
            to_server,
            vm: Vm::new(),
            tick: 0,
            ghosts: Ghosts(HashMap::from([(5, 50)])),
            holder,
            caller,
        }
    }

    fn set_tick(&mut self, tick: u64) {
        self.tick = tick;
        self.server.tick(tick);
        self.client.tick(tick);
    }

    /// Everything the client queued reaches the server and runs there.
    fn client_to_server(&mut self) -> Result<(), NetError> {
        self.client.flush();
        for (batch, _) in self.to_server.take() {
            self.server.receive(&batch)?;
        }
        self.run_server();
        Ok(())
    }

    fn run_server(&mut self) {
        let mut hooks = NoHooks;
        let mut env = RpcEnv {
            vm: &mut self.vm,
            tick: self.tick,
            conn: Some(self.caller),
            perms: &self.holder,
            ghosts: &self.ghosts,
            hooks: &mut hooks,
        };
        self.server.dispatch(&mut env);
        self.server.flush();
    }

    /// Everything the server queued reaches the client and runs there.
    fn server_to_client(&mut self) -> Result<(), NetError> {
        self.server.flush();
        for (batch, _) in self.to_client.take() {
            self.client.receive(&batch)?;
        }
        let mut hooks = NoHooks;
        let mut env = RpcEnv {
            vm: &mut self.vm,
            tick: self.tick,
            conn: None,
            perms: &NoPermissions,
            ghosts: &NoGhosts,
            hooks: &mut hooks,
        };
        self.client.dispatch(&mut env);
        Ok(())
    }

    fn round(&mut self) {
        self.client_to_server().expect("server reads");
        self.server_to_client().expect("client reads");
    }
}

type Slot<T> = Rc<RefCell<Option<Result<T, RpcError>>>>;

fn slot<T>() -> Slot<T> {
    Rc::new(RefCell::new(None))
}

fn call_echo(pair: &mut Pair, name: &str, text: &str) -> Slot<Echoed> {
    let rpc = pair.client.rpc::<Echo, Echoed>(name).expect("the rpc");
    let out = slot();
    let sink = Rc::clone(&out);
    pair.client
        .call(
            &rpc,
            Echo {
                text: text.to_string(),
                count: 2,
            },
            move |r| *sink.borrow_mut() = Some(r),
        )
        .expect("queued");
    out
}

#[test]
fn rpc_round_trip_both_ways() {
    let (engine, log) = engine();
    let mut pair = Pair::new(&engine);
    // Client to server, with a reply.
    let out = call_echo(&mut pair, "echo", "ab");
    assert!(out.borrow().is_none(), "the reply comes in a later round");
    pair.round();
    assert_eq!(
        out.borrow().clone(),
        Some(Ok(Echoed {
            text: "abab".to_string()
        }))
    );
    assert_eq!(log.echoes.get(), 1);
    // Server to client, a notification.
    let ping = pair.server.rpc::<Ping, ()>("ping").expect("the rpc");
    pair.server
        .notify(
            &ping,
            Ping {
                at: [1.0, 2.0, 3.0],
            },
        )
        .expect("queued");
    pair.server_to_client().expect("client reads");
    assert_eq!(*log.pings.borrow(), vec![[1.0, 2.0, 3.0]]);
    // Nothing is outstanding afterwards, and a reply-bearing call can be made again.
    let again = call_echo(&mut pair, "echo", "c");
    pair.round();
    assert_eq!(again.borrow().clone().unwrap().unwrap().text, "cc");
}

#[test]
fn rpc_unreliable_notification_is_sent_apart_and_carried() {
    let (engine, log) = engine();
    let mut pair = Pair::new(&engine);
    let note = pair.server.rpc::<Ping, ()>("note").expect("the rpc");
    pair.server.notify(&note, Ping::default()).expect("queued");
    pair.server.flush();
    let sent = pair.to_client.take();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].1, Delivery::Unreliable);
    pair.client.receive(&sent[0].0).expect("reads");
    let mut vm = Vm::new();
    let mut hooks = NoHooks;
    let mut env = RpcEnv {
        vm: &mut vm,
        tick: 0,
        conn: None,
        perms: &NoPermissions,
        ghosts: &NoGhosts,
        hooks: &mut hooks,
    };
    pair.client.dispatch(&mut env);
    assert_eq!(log.notes.get(), 1);
}

#[test]
fn rpc_handler_errors_carry_only_their_text() {
    let (engine, _) = engine();
    let mut pair = Pair::new(&engine);
    let long = call_echo(&mut pair, "echo", "fail");
    let refused = call_echo(&mut pair, "echo", "refuse");
    pair.round();
    match long.borrow().clone() {
        Some(Err(RpcError::Handler(text))) => assert_eq!(text.len(), MAX_ERROR_TEXT),
        other => panic!("expected a handler error, got {other:?}"),
    }
    assert_eq!(refused.borrow().clone(), Some(Err(RpcError::Refused)));
}

#[test]
fn rpc_call_past_the_send_queue_cap_is_busy() {
    let (engine, _) = engine();
    let mut pair = Pair::new(&engine);
    pair.client.set_send_queue(200);
    let rpc = pair.client.rpc::<Blob, ()>("blob").expect("the rpc");
    let blob = Blob { bytes: vec![7; 80] };
    // Each call queues about 90 bytes: two fit, the third does not.
    let mut results = Vec::new();
    for _ in 0..3 {
        results.push(pair.client.notify(&rpc, blob.clone()));
    }
    assert_eq!(results[0], Ok(()));
    assert_eq!(results[1], Ok(()));
    assert_eq!(results[2], Err(RpcError::Busy));
    // What the carrier has not had acknowledged counts too: after the flush the queue is empty but the carrier is full.
    pair.client.flush();
    pair.to_server.take();
    pair.to_server.unacked.set(190);
    assert_eq!(pair.client.notify(&rpc, blob.clone()), Err(RpcError::Busy));
    pair.to_server.unacked.set(0);
    assert_eq!(pair.client.notify(&rpc, blob), Ok(()));
}

#[test]
fn rpc_outstanding_calls_are_capped_per_rpc() {
    let (engine, _) = engine();
    let mut pair = Pair::new(&engine);
    let rpc = pair.client.rpc::<Echo, Echoed>("echo").expect("the rpc");
    for _ in 0..MAX_OUTSTANDING {
        pair.client
            .call(&rpc, Echo::default(), |_| {})
            .expect("queued");
    }
    assert_eq!(
        pair.client.call(&rpc, Echo::default(), |_| {}),
        Err(RpcError::Busy)
    );
}

#[test]
fn rpc_oversize_message_is_too_large_locally() {
    let (engine, _) = engine();
    let mut pair = Pair::new(&engine);
    let small = pair.client.rpc::<Echo, ()>("small").expect("the rpc");
    let long = Echo {
        text: "far too long for eight bytes".to_string(),
        count: 1,
    };
    assert_eq!(pair.client.notify(&small, long), Err(RpcError::TooLarge));
    // Nothing was queued or counted outstanding.
    pair.client.flush();
    assert!(pair.to_server.take().is_empty());
    assert_eq!(
        pair.client.notify(
            &small,
            Echo {
                text: "a".into(),
                count: 1
            }
        ),
        Ok(())
    );
}

#[test]
fn rpc_burst_limits_the_handlers_run_in_one_tick() {
    let (engine, log) = engine();
    let mut pair = Pair::new(&engine);
    let rpc = pair.client.rpc::<Target, ()>("target").expect("the rpc");
    // `target` is 8 a second with a burst of 16: a thousand calls in one tick run 16 handlers.
    for _ in 0..1000 {
        let who = torque_ghosts::GhostRef(5);
        pair.client.notify(&rpc, Target { who }).expect("queued");
    }
    pair.round();
    assert_eq!(
        log.targets.get(),
        16,
        "one tick runs at most `burst` handlers"
    );
    assert_eq!(pair.server.drops("target"), 984);
}

#[test]
fn rpc_rate_refills_within_one_call_of_the_rate_over_600_ticks() {
    let (engine, log) = engine();
    let mut pair = Pair::new(&engine);
    let rpc = pair.client.rpc::<Echo, Echoed>("echo").expect("the rpc");
    // Keep asking every tick, so each token is spent as soon as it exists.
    for tick in 0..=600 {
        pair.set_tick(tick);
        for _ in 0..4 {
            let _ = pair.client.call(&rpc, Echo::default(), |_| {});
        }
        pair.round();
    }
    // 8 in the burst, then 4 a second for 600 ticks of 32 ms: 4 * 19.2 = 76.8.
    let expected = 8.0 + 4.0 * 600.0 * 32.0 / 1000.0;
    let handled = f64::from(log.echoes.get());
    assert!(
        (handled - expected).abs() <= 1.0,
        "{handled} handlers for {expected} tokens"
    );
}

#[test]
fn rpc_a_rate_limited_call_with_a_reply_waiting_gets_rate_limited() {
    let (engine, _) = engine();
    let mut pair = Pair::new(&engine);
    let results: Vec<Slot<Echoed>> = (0..12).map(|_| call_echo(&mut pair, "echo", "a")).collect();
    pair.round();
    let limited = results
        .iter()
        .filter(|r| r.borrow().clone() == Some(Err(RpcError::RateLimited)))
        .count();
    let served = results
        .iter()
        .filter(|r| matches!(r.borrow().clone(), Some(Ok(_))))
        .count();
    // One answer a tick for an RPC that is over its rate: the other callers wait for their timeout.
    assert_eq!((served, limited), (8, 1));
    assert_eq!(pair.server.drops("echo"), 4);
    let lines = pair.server.take_reports();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("echo") && lines[0].contains("4 call(s) dropped"));
    assert!(pair.server.take_reports().is_empty(), "one line a second");
}

/// What a test caller holds, by name.
fn holder(held: &str) -> Holder {
    let mut roles = torque_permissions::Roles::default();
    roles.define("builder", &["props.spawn"]).unwrap();
    roles.define("other", &["tools.paint"]).unwrap();
    let mut subject = torque_permissions::Subject::default();
    match held {
        "none" => {}
        "exact" => drop(subject.grant("props.spawn")),
        "wildcard" => drop(subject.grant("props.*")),
        "role" => drop(subject.add_role(&roles, "builder")),
        "unrelated" => {
            subject.grant("tools.paint").unwrap();
            subject.add_role(&roles, "other").unwrap();
        }
        other => panic!("{other}"),
    }
    Holder { roles, subject }
}

#[test]
fn rpc_permission_table() {
    let (engine, _) = engine();
    // Direction ToServer: {anyone, permission, default permission, check} x {nothing, an exact grant, a wildcard grant, a role's
    // grant, an unrelated grant}; the caller is a stranger. `check` also lets the owner in.
    let holds = |held: &str| matches!(held, "exact" | "wildcard" | "role");
    for held in ["none", "exact", "wildcard", "role", "unrelated"] {
        for (name, allowed) in [
            ("everyone", true),
            ("permitted", holds(held)),
            ("owner", holds(held)),
        ] {
            let mut pair = Pair::as_caller(&engine, STRANGER, holder(held));
            let out = call_echo(&mut pair, name, "x");
            pair.round();
            let got = out.borrow().clone();
            if allowed {
                assert!(matches!(got, Some(Ok(_))), "{name} holding {held}: {got:?}");
            } else {
                assert_eq!(got, Some(Err(RpcError::Refused)), "{name} holding {held}");
                assert_eq!(pair.server.refusals(name), 1);
            }
        }
    }
    // The owner passes the check while holding nothing.
    let mut pair = Pair::as_caller(&engine, OWNER, holder("none"));
    let out = call_echo(&mut pair, "owner", "x");
    pair.round();
    assert!(matches!(out.borrow().clone(), Some(Ok(_))));
}

#[test]
fn rpc_default_permission_is_named_after_the_plugin_and_the_rpc() {
    let (engine, _) = engine();
    let table = engine.net_rpcs().unwrap();
    let entry = table.get(table.id("defaulted").unwrap()).unwrap();
    assert!(
        matches!(&entry.allow, Allow::Permission(p) if p == "rpc.test_rpc.defaulted"),
        "{:?}",
        entry.allow
    );
    // Asking for it declares it, and a permission named in `allow` is declared too, so no check of one is undeclared.
    let declared: Vec<&str> = engine
        .declared_permissions()
        .iter()
        .map(|d| d.name.as_str())
        .collect();
    assert!(
        declared.contains(&"rpc.test_rpc.defaulted") && declared.contains(&"props.spawn"),
        "{declared:?}"
    );
    // The holder of exactly that permission is let in, a stranger is not.
    let mut roles = torque_permissions::Roles::default();
    roles.define("caller", &["rpc.test_rpc.*"]).unwrap();
    let mut subject = torque_permissions::Subject::default();
    subject.add_role(&roles, "caller").unwrap();
    let mut pair = Pair::as_caller(&engine, STRANGER, Holder { roles, subject });
    let ok = call_echo(&mut pair, "defaulted", "x");
    pair.round();
    assert!(matches!(ok.borrow().clone(), Some(Ok(_))));
    let mut pair = Pair::as_caller(&engine, STRANGER, holder("none"));
    let refused = call_echo(&mut pair, "defaulted", "x");
    pair.round();
    assert_eq!(refused.borrow().clone(), Some(Err(RpcError::Refused)));
}

#[test]
fn rpc_a_permission_named_in_allow_is_checked_at_registration() {
    struct Bad(&'static str);
    impl Plugin for Bad {
        fn build(&self, engine: &mut EngineBuilder) {
            engine
                .net()
                .rpc::<Echo, ()>("x")
                .to_server()
                .allow(Allow::permission(self.0))
                .server(|_cx: &mut RpcCx, _r: Echo| Ok(()));
        }
    }
    impl crate::PluginType for Bad {
        const ID: PluginId = PluginId::new("test.bad");
    }
    for bad in ["", "Props.Spawn", "props.*", "a b", "a..b"] {
        let error = EngineBuilder::new()
            .add_plugins(Bad(bad))
            .build()
            .unwrap_err()
            .to_string();
        assert!(error.contains("RPC `x`"), "{bad:?}: {error}");
    }
    assert!(
        EngineBuilder::new()
            .add_plugins(Bad("props.spawn"))
            .build()
            .is_ok()
    );
}

#[test]
fn rpc_direction_is_enforced_on_both_ends() {
    let (engine, _) = engine();
    // A client that sends a to_client RPC is a protocol error on the server.
    let mut pair = Pair::new(&engine);
    let ping = pair.table.id("ping").expect("ping");
    let frame = forged(&pair, 1, ping, None, &[]);
    assert!(
        pair.server.receive(&frame).is_err(),
        "a client sent a server-to-client RPC"
    );
    // A server that sends a to_server RPC is one on the client.
    let mut pair = Pair::new(&engine);
    let everyone = pair.table.id("everyone").expect("everyone");
    let frame = forged(&pair, 0, everyone, Some(0), &[0]);
    assert!(
        pair.client.receive(&frame).is_err(),
        "a server sent a client-to-server RPC"
    );
    // Neither end can call the other's RPC locally.
    let ping = pair.client.rpc::<Ping, ()>("ping").expect("the rpc");
    assert_eq!(
        pair.client.notify(&ping, Ping::default()),
        Err(RpcError::Unsupported)
    );
    let echo = pair.server.rpc::<Echo, Echoed>("echo").expect("the rpc");
    assert_eq!(
        pair.server.call(&echo, Echo::default(), |_| {}),
        Err(RpcError::Unsupported)
    );
}

/// A batch of one frame that an honest peer would not send.
fn forged(pair: &Pair, kind: u32, rpc: usize, call: Option<u16>, body: &[u8]) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.write_bits(kind, 2);
    w.write_bits(rpc as u32, pair.table.id_bits());
    if let Some(call) = call {
        w.write_bits(u32::from(call), 16);
    }
    w.write_bytes(body);
    let body = w.into_bytes();
    let mut batch = (body.len() as u16).to_le_bytes().to_vec();
    batch.extend(body);
    batch
}

#[test]
fn rpc_protocol_errors() {
    let (engine, _) = engine();
    let mut pair = Pair::new(&engine);
    let echo = pair.table.id("echo").expect("echo");
    // A reply to a call nobody made.
    let frame = forged(&pair, 2, echo, Some(9), &[0, 0]);
    assert!(pair.client.receive(&frame).is_err());
    // A reply at all, to the server, which never calls.
    assert!(pair.server.receive(&frame).is_err());
    // An RPC id the table does not have: the table of 12 RPCs has 4-bit ids.
    let frame = forged(&pair, 1, 15, None, &[]);
    assert!(pair.server.receive(&frame).is_err());
    // A message over its RPC's size (`echo` takes 256 bytes at most).
    let frame = forged(&pair, 0, echo, Some(1), &[0; 300]);
    assert!(pair.server.receive(&frame).is_err());
    // A truncated frame and a batch whose length lies.
    assert!(pair.server.receive(&[5]).is_err());
    assert!(pair.server.receive(&[200, 0, 1, 2]).is_err());
    // A notification sent as a call, and a call as a notification.
    let target = pair.table.id("target").expect("target");
    let frame = forged(&pair, 0, target, Some(0), &[0; 4]);
    assert!(pair.server.receive(&frame).is_err());
    let frame = forged(&pair, 1, echo, None, &[0; 4]);
    assert!(pair.server.receive(&frame).is_err());
    // A string over its field's cap: `text` takes 32 bytes, this one says 40 of them.
    let mut w = BitWriter::new();
    w.write_bits(0, 2);
    w.write_bits(echo as u32, pair.table.id_bits());
    w.write_bits(0, 16);
    let table = torque_net::huffman::Table::default();
    let mut c = torque_ghosts::codec::Enc::new(&mut w, &table, widths());
    let mut text = vec![b'a'; 40];
    torque_ghosts::codec::Codec::string(&mut c, "text", &mut text).unwrap();
    w.write_bits(1, 6);
    let body = w.into_bytes();
    let mut batch = (body.len() as u16).to_le_bytes().to_vec();
    batch.extend(body);
    assert!(pair.server.receive(&batch).is_err());
}

#[test]
fn rpc_a_float_that_is_not_finite_is_refused_on_receipt_and_never_sent() {
    let (engine, _) = engine();
    let mut pair = Pair::new(&engine);
    // A forged frame carrying a NaN: the receiver's decode refuses it.
    let ping_id = pair.table.id("ping").expect("ping");
    let mut w = BitWriter::new();
    w.write_bits(1, 2);
    w.write_bits(ping_id as u32, pair.table.id_bits());
    w.write_u32(f32::NAN.to_bits());
    w.write_u32(0);
    w.write_u32(0);
    let body = w.into_bytes();
    let mut batch = (body.len() as u16).to_le_bytes().to_vec();
    batch.extend(body);
    assert!(pair.client.receive(&batch).is_err());
    // This side's own sends are checked the same way: a message the other side must refuse is not sent.
    let ping = pair.server.rpc::<Ping, ()>("ping").expect("the rpc");
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let sent = pair.server.notify(
            &ping,
            Ping {
                at: [0.0, bad, 0.0],
            },
        );
        assert_eq!(sent, Err(RpcError::Invalid));
    }
    // A script's numbers that parse as infinity or NaN are the same.
    for text in ["1e999 0 0", "inf 0 0", "nan 0 0", "0 0 -1e999"] {
        let sent = pair
            .server
            .send_script("ping", &[text.as_bytes().to_vec()], &NoGhosts);
        assert_eq!(sent, Err(RpcError::Invalid), "{text}");
    }
    pair.server.flush();
    assert!(pair.to_client.take().is_empty(), "nothing was queued");
    assert!(
        pair.server
            .send_script("ping", &[b"1 2 3".to_vec()], &NoGhosts)
            .is_ok()
    );
}

#[test]
fn rpc_ghost_references_are_checked_before_the_handler() {
    let (engine, log) = engine();
    let mut pair = Pair::new(&engine);
    let rpc = pair.client.rpc::<Target, ()>("target").expect("the rpc");
    pair.client
        .notify(
            &rpc,
            Target {
                who: torque_ghosts::GhostRef(5),
            },
        )
        .expect("queued");
    pair.client
        .notify(
            &rpc,
            Target {
                who: torque_ghosts::GhostRef(6),
            },
        )
        .expect("queued");
    pair.round();
    assert_eq!(
        log.targets.get(),
        1,
        "the call that names an unknown ghost never ran"
    );
    assert_eq!(*log.checked.borrow(), vec![Some(50)]);
    assert_eq!(pair.server.refusals("target"), 1);
}

#[test]
fn rpc_late_replies_time_out_and_a_closed_link_disconnects() {
    let (engine, _) = engine();
    let mut pair = Pair::new(&engine);
    pair.client.set_timeout(10);
    let late = call_echo(&mut pair, "echo", "a");
    pair.client.flush();
    pair.to_server.take();
    pair.set_tick(9);
    pair.server_to_client().expect("nothing");
    assert!(late.borrow().is_none());
    pair.set_tick(10);
    pair.server_to_client().expect("nothing");
    assert_eq!(late.borrow().clone(), Some(Err(RpcError::Timeout)));
    // A reply that comes after the timeout is a reply to nothing.
    let waiting = call_echo(&mut pair, "echo", "b");
    pair.client.close();
    let mut vm = Vm::new();
    let mut hooks = NoHooks;
    let mut env = RpcEnv {
        vm: &mut vm,
        tick: 10,
        conn: None,
        perms: &NoPermissions,
        ghosts: &NoGhosts,
        hooks: &mut hooks,
    };
    pair.client.dispatch(&mut env);
    assert_eq!(waiting.borrow().clone(), Some(Err(RpcError::Disconnected)));
    let rpc = pair.client.rpc::<Echo, Echoed>("echo").expect("the rpc");
    assert_eq!(
        pair.client.call(&rpc, Echo::default(), |_| {}),
        Err(RpcError::Disconnected)
    );
}

#[test]
fn rpc_handles_check_the_layouts() {
    let (engine, _) = engine();
    let pair = Pair::new(&engine);
    assert!(pair.client.rpc::<Echo, Echoed>("echo").is_ok());
    assert_eq!(
        pair.client.rpc::<Ping, ()>("echo").map(|_| ()),
        Err(RpcError::Unsupported)
    );
    assert_eq!(
        pair.client.rpc::<Echo, Echoed>("nothing").map(|_| ()),
        Err(RpcError::Unsupported)
    );
}

#[test]
fn rpc_registration_is_checked() {
    struct Bad(fn(&mut EngineBuilder));
    impl Plugin for Bad {
        fn build(&self, engine: &mut EngineBuilder) {
            (self.0)(engine);
        }
    }
    impl crate::PluginType for Bad {
        const ID: PluginId = PluginId::new("test.bad");
    }
    let build = |body: fn(&mut EngineBuilder)| {
        EngineBuilder::new()
            .add_plugins(Bad(body))
            .build()
            .map(|_| ())
            .map_err(|e| e.to_string())
    };
    let twice = build(|e| {
        e.net()
            .rpc::<Echo, Echoed>("same")
            .to_server()
            .server(|_cx: &mut RpcCx, r: Echo| Ok(Echoed { text: r.text }));
        e.net()
            .rpc::<Echo, Echoed>("same")
            .to_server()
            .server(|_cx: &mut RpcCx, r: Echo| Ok(Echoed { text: r.text }));
    });
    assert!(twice.unwrap_err().contains("also registered"));
    let named = build(|e| {
        e.net()
            .rpc::<Echo, ()>("bad name")
            .to_server()
            .server(|_cx: &mut RpcCx, _r: Echo| Ok(()));
    });
    assert!(named.unwrap_err().contains("not an RPC name"));
    let big = build(|e| {
        e.net()
            .rpc::<Echo, ()>("big")
            .to_server()
            .max_bytes(2000)
            .server(|_cx: &mut RpcCx, _r: Echo| Ok(()));
    });
    assert!(big.unwrap_err().contains("max_bytes"));
    let unreliable = build(|e| {
        e.net()
            .rpc::<Echo, Echoed>("loose")
            .to_server()
            .delivery(Delivery::Unreliable)
            .server(|_cx: &mut RpcCx, r: Echo| Ok(Echoed { text: r.text }));
    });
    assert!(unreliable.unwrap_err().contains("unreliable"));
}

#[test]
fn rpc_an_engine_without_rpcs_says_nothing_new() {
    let engine = EngineBuilder::new().build().expect("builds");
    assert!(engine.net_rpcs().expect("table").is_empty());
    assert!(engine.net_channels().expect("channels").is_empty());
    assert!(!engine.is_networked());
    let (with, _) = engine_with_rpcs();
    assert!(with.is_networked());
    let channels = with.net_channels().expect("channels");
    assert_eq!(channels.names(), [RPC_CHANNEL]);
    assert_eq!(channels.up_names(), [RPC_CHANNEL]);
    let manifest = with.net_manifest(&torque_net::profile::HandshakeProfile::default());
    assert_eq!(manifest.rpcs.count(), 12);
    assert!(
        manifest
            .rpcs
            .names()
            .unwrap()
            .iter()
            .any(|n| n.starts_with("echo:s:"))
    );
    assert!(manifest.encode().expect("encodes").first() == Some(&2));
}

fn engine_with_rpcs() -> (crate::Engine, Rc<Log>) {
    engine()
}

#[test]
fn rpc_manifest_names_the_rpc_two_peers_disagree_on() {
    let (server, _) = engine();
    let profile = torque_net::profile::HandshakeProfile::default();
    let ours = server.net_manifest(&profile);
    // A client whose `echo` has another layout.
    #[derive(Debug, Clone, PartialEq, Default, NetMessage)]
    struct Other {
        #[net(max_len = 99)]
        text: String,
    }
    #[derive(PluginId)]
    #[plugin(id = "test.rpc")]
    struct Changed;
    impl Plugin for Changed {
        fn build(&self, engine: &mut EngineBuilder) {
            engine
                .net()
                .rpc::<Other, ()>("echo")
                .to_server()
                .server(|_cx: &mut RpcCx, _r: Other| Ok(()));
        }
    }
    let client = EngineBuilder::new()
        .add_plugins(Changed)
        .build()
        .expect("builds")
        .net_manifest(&profile);
    let mismatch = ours.mismatch(&client).expect("they disagree");
    assert!(
        mismatch.to_string().contains("disagree on RPC"),
        "{mismatch}"
    );
    // The same RPCs agree, and the wire round trip keeps them.
    let again =
        torque_net::manifest::Manifest::decode(&ours.encode().expect("encodes")).expect("decodes");
    assert_eq!(again.rpcs, ours.rpcs);
}

/// A small deterministic generator for the fuzz test.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Random and mutated batches into both ends: nothing panics, and what the links hold stays bounded.
/// `TORQUE_FUZZ_RPC=1000000` runs a million rounds (the default is a few thousand).
#[test]
fn rpc_fuzz_hostile_batches_never_panic() {
    let rounds: u64 = std::env::var("TORQUE_FUZZ_RPC")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5_000);
    let (engine, _) = engine();
    let mut pair = Pair::new(&engine);
    // A pool of honest batches to mutate.
    let echo = pair.client.rpc::<Echo, Echoed>("echo").expect("the rpc");
    pair.client
        .call(
            &echo,
            Echo {
                text: "hello".into(),
                count: 3,
            },
            |_| {},
        )
        .expect("queued");
    let target = pair.client.rpc::<Target, ()>("target").expect("the rpc");
    pair.client
        .notify(
            &target,
            Target {
                who: torque_ghosts::GhostRef(5),
            },
        )
        .expect("queued");
    pair.client.flush();
    let mut pool: Vec<Vec<u8>> = pair.to_server.take().into_iter().map(|(b, _)| b).collect();
    let ping = pair.server.rpc::<Ping, ()>("ping").expect("the rpc");
    pair.server
        .notify(
            &ping,
            Ping {
                at: [1.0, 2.0, 3.0],
            },
        )
        .expect("queued");
    pair.server.flush();
    pool.extend(pair.to_client.take().into_iter().map(|(b, _)| b));
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut hooks = NoHooks;
    let mut vm = Vm::new();
    for round in 0..rounds {
        let mut batch = if rng.below(4) == 0 {
            (0..rng.below(300))
                .map(|_| rng.next() as u8)
                .collect::<Vec<u8>>()
        } else {
            pool[rng.below(pool.len())].clone()
        };
        for _ in 0..rng.below(4) {
            if batch.is_empty() {
                break;
            }
            let at = rng.below(batch.len());
            match rng.below(3) {
                0 => batch[at] ^= 1 << rng.below(8),
                1 => batch[at] = rng.next() as u8,
                _ => batch.truncate(at),
            }
        }
        // Whatever the result, the link must stay usable and its queues bounded.
        let tick = round / 3;
        pair.server.tick(tick);
        pair.client.tick(tick);
        let _ = pair.server.receive(&batch);
        let _ = pair.client.receive(&batch);
        let mut env = RpcEnv {
            vm: &mut vm,
            tick,
            conn: Some(OWNER),
            perms: &NoPermissions,
            ghosts: &pair.ghosts,
            hooks: &mut hooks,
        };
        pair.server.dispatch(&mut env);
        let mut env = RpcEnv {
            vm: &mut vm,
            tick,
            conn: None,
            perms: &NoPermissions,
            ghosts: &NoGhosts,
            hooks: &mut hooks,
        };
        pair.client.dispatch(&mut env);
        pair.server.flush();
        pair.to_client.take();
        assert!(pair.server.send_queue_bytes() <= DEFAULT_SEND_QUEUE);
    }
}

#[test]
fn rpc_a_client_checks_the_ghosts_a_notification_names() {
    let (engine, log) = engine();
    let mut pair = Pair::new(&engine);
    let look = pair.server.rpc::<Target, ()>("look").expect("the rpc");
    for index in [5, 6] {
        pair.server
            .notify(
                &look,
                Target {
                    who: torque_ghosts::GhostRef(index),
                },
            )
            .expect("queued");
    }
    pair.server.flush();
    for (batch, _) in pair.to_client.take() {
        pair.client.receive(&batch).expect("client reads");
    }
    // The client's world has ghost 5 (its object is 50) and not ghost 6.
    let world = Ghosts(HashMap::from([(5, 50)]));
    let mut hooks = NoHooks;
    let mut env = RpcEnv {
        vm: &mut pair.vm,
        tick: 0,
        conn: None,
        perms: &NoPermissions,
        ghosts: &world,
        hooks: &mut hooks,
    };
    pair.client.dispatch(&mut env);
    assert_eq!(
        *log.looked.borrow(),
        vec![Some(50)],
        "the unknown ghost never reached the handler"
    );
    assert_eq!(pair.client.refusals("look"), 1);
}

#[test]
fn rpc_a_client_with_no_ghost_table_refuses_every_ghost_reference() {
    let (engine, log) = engine();
    let mut pair = Pair::new(&engine);
    let look = pair.server.rpc::<Target, ()>("look").expect("the rpc");
    pair.server
        .notify(
            &look,
            Target {
                who: torque_ghosts::GhostRef(5),
            },
        )
        .expect("queued");
    pair.server_to_client().expect("client reads");
    assert!(log.looked.borrow().is_empty());
    assert_eq!(pair.client.refusals("look"), 1);
}

#[test]
fn rpc_one_connection_cannot_crowd_out_the_others_inbox() {
    let mut channel = RpcChannel::default();
    let (flooder, honest) = (ConnId(1), ConnId(2));
    let batch = vec![0u8; 16 << 10];
    // The honest client sends a few batches; the flooder sends far more than its share.
    for _ in 0..3 {
        channel.receive_from(honest, Kind::Delta, &batch);
    }
    for _ in 0..40 {
        channel.receive_from(flooder, Kind::Delta, &batch);
    }
    channel.receive_from(honest, Kind::Delta, &batch);
    let got = channel.take();
    assert_eq!(got.over, [Some(flooder)], "only the flooder is reported");
    let kept = |who: ConnId| got.batches.iter().filter(|(c, _)| *c == Some(who)).count();
    assert_eq!(
        kept(honest),
        4,
        "nothing the honest client sent was dropped"
    );
    assert_eq!(
        kept(flooder) * batch.len(),
        CONN_INBOX_CAP,
        "the flooder keeps its share and no more"
    );
    // The next tick starts with fresh shares and no one reported.
    channel.receive_from(flooder, Kind::Delta, &batch);
    let got = channel.take();
    assert!(got.over.is_empty() && got.batches.len() == 1);
}

#[test]
fn rpc_a_client_inbox_over_its_share_is_reported_as_the_server() {
    let mut channel = RpcChannel::default();
    for _ in 0..20 {
        channel.receive(Kind::Delta, &vec![0u8; 16 << 10]);
    }
    assert_eq!(channel.take().over, [None]);
}

/// Hooks that keep what a script handler was called with.
#[derive(Default)]
struct Capture {
    calls: Vec<(String, Vec<Vec<u8>>)>,
}

impl RpcHooks for Capture {
    fn script(
        &mut self,
        _vm: &mut Vm,
        _caller: Option<ConnId>,
        function: &str,
        args: Vec<Vec<u8>>,
    ) -> Result<(), String> {
        self.calls.push((function.to_string(), args));
        Ok(())
    }
}

#[test]
fn rpc_a_script_gets_objects_not_ghost_indexes_both_ways() {
    let (engine, log) = engine();
    // Ghost 5 is object 50 on this connection.
    let mut pair = Pair::new(&engine);
    // In: the wire carries ghost 5, the script handler is called with object 50.
    let scripted = pair.client.rpc::<Target, ()>("scripted").expect("the rpc");
    pair.client
        .notify(
            &scripted,
            Target {
                who: torque_ghosts::GhostRef(5),
            },
        )
        .expect("queued");
    pair.client.flush();
    for (batch, _) in pair.to_server.take() {
        pair.server.receive(&batch).expect("server reads");
    }
    let mut capture = Capture::default();
    let mut env = RpcEnv {
        vm: &mut pair.vm,
        tick: 0,
        conn: Some(OWNER),
        perms: &pair.holder,
        ghosts: &pair.ghosts,
        hooks: &mut capture,
    };
    pair.server.dispatch(&mut env);
    assert_eq!(
        capture.calls,
        [("rpcServer_scripted".to_string(), vec![b"50".to_vec()])]
    );
    // Out: a script names object 50; the wire carries ghost 5, which the server resolves to 50 again.
    assert!(
        pair.client
            .send_script("target", &[b"50".to_vec()], &pair.ghosts)
            .is_ok()
    );
    pair.client_to_server().expect("server reads");
    assert_eq!(*log.checked.borrow(), vec![Some(50)]);
    assert_eq!(
        pair.server.refusals("target"),
        0,
        "the index on the wire was a ghost the server sent"
    );
    // An object the peer has no ghost of is not sent.
    let unseen = pair
        .client
        .send_script("target", &[b"77".to_vec()], &pair.ghosts);
    assert_eq!(unseen, Err(RpcError::Invalid));
    // A handler that is a script and a ghost the connection does not have is refused before the script runs.
    pair.client
        .notify(
            &scripted,
            Target {
                who: torque_ghosts::GhostRef(6),
            },
        )
        .expect("queued");
    pair.client.flush();
    for (batch, _) in pair.to_server.take() {
        pair.server.receive(&batch).expect("server reads");
    }
    let mut capture = Capture::default();
    let mut env = RpcEnv {
        vm: &mut pair.vm,
        tick: 0,
        conn: Some(OWNER),
        perms: &pair.holder,
        ghosts: &pair.ghosts,
        hooks: &mut capture,
    };
    pair.server.dispatch(&mut env);
    assert!(capture.calls.is_empty());
}

#[test]
fn rpc_a_late_reply_to_a_timed_out_call_is_ignored() {
    let (engine, _) = engine();
    let mut pair = Pair::new(&engine);
    pair.client.set_timeout(1);
    let fired = Rc::new(Cell::new(0u32));
    let rpc = pair.client.rpc::<Echo, Echoed>("echo").expect("the rpc");
    let seen = Rc::clone(&fired);
    pair.client
        .call(&rpc, Echo::default(), move |r| {
            assert_eq!(r, Err(RpcError::Timeout));
            seen.set(seen.get() + 1);
        })
        .expect("queued");
    pair.client.flush();
    let request = pair.to_server.take();
    pair.set_tick(1);
    pair.server_to_client().expect("the timeout runs");
    assert_eq!(fired.get(), 1);
    // The server answers late, and the answer reaches a client that has long forgotten the call: not an error.
    for (batch, _) in request {
        pair.server.receive(&batch).expect("server reads");
    }
    pair.run_server();
    pair.server_to_client()
        .expect("a late reply is not a protocol error");
    assert_eq!(fired.get(), 1, "the callback ran once");
    // The id is spent: the same late reply twice is a reply to nothing.
    let again = forged(&pair, 2, pair.table.id("echo").unwrap(), Some(0), &[0, 0]);
    assert!(pair.client.receive(&again).is_err());
    // A late reply of another RPC than the call's is an error.
    let mut other = Pair::new(&engine);
    other.client.set_timeout(1);
    let rpc = other.client.rpc::<Echo, Echoed>("echo").expect("the rpc");
    other
        .client
        .call(&rpc, Echo::default(), |_| {})
        .expect("queued");
    other.set_tick(1);
    other.server_to_client().expect("timeout");
    let wrong = forged(
        &other,
        2,
        other.table.id("everyone").unwrap(),
        Some(0),
        &[0, 0],
    );
    assert!(other.client.receive(&wrong).is_err());
}

#[test]
fn rpc_a_tombstoned_call_id_is_not_reused() {
    let (engine, _) = engine();
    let mut pair = Pair::new(&engine);
    pair.client.set_timeout(1);
    let rpc = pair.client.rpc::<Echo, Echoed>("echo").expect("the rpc");
    pair.client
        .call(&rpc, Echo::default(), |_| {})
        .expect("queued");
    assert_eq!(pair.client.pending_calls(), [0]);
    pair.set_tick(1);
    pair.server_to_client().expect("timeout");
    assert_eq!(pair.client.tombstoned_calls(), [0]);
    // The ids come round to 0 again: it is skipped while the late reply may still come.
    pair.client.set_next_call(0);
    pair.client.set_timeout(1_000);
    pair.client
        .call(&rpc, Echo::default(), |_| {})
        .expect("queued");
    assert_eq!(pair.client.pending_calls(), [1]);
    // Once its time has passed, the id is free.
    pair.set_tick(1 + 10 + 1);
    assert!(pair.client.tombstoned_calls().is_empty());
    pair.client.set_next_call(0);
    pair.client
        .call(&rpc, Echo::default(), |_| {})
        .expect("queued");
    let mut pending = pair.client.pending_calls();
    pending.sort_unstable();
    assert_eq!(pending, [0, 1]);
}

#[test]
fn rpc_two_rpcs_that_share_a_script_handler_do_not_register() {
    struct Clash(&'static str, &'static str);
    impl Plugin for Clash {
        fn build(&self, engine: &mut EngineBuilder) {
            for name in [self.0, self.1] {
                engine
                    .net()
                    .rpc::<Target, ()>(name)
                    .to_server()
                    .allow(Allow::permission("a.b"))
                    .server_script();
            }
        }
    }
    impl crate::PluginType for Clash {
        const ID: PluginId = PluginId::new("test.clash");
    }
    for (a, b) in [
        ("chat.note", "chat_note"),
        ("Chat.Note", "chat.note"),
        ("a-b", "a.b"),
    ] {
        let error = EngineBuilder::new()
            .add_plugins(Clash(a, b))
            .build()
            .unwrap_err()
            .to_string();
        assert!(error.contains("script function"), "{a} {b}: {error}");
    }
    assert!(
        EngineBuilder::new()
            .add_plugins(Clash("chat.note", "chat.talk"))
            .build()
            .is_ok()
    );
}

#[test]
fn rpc_two_rpcs_that_share_a_default_permission_do_not_register() {
    struct Ask(&'static [&'static str]);
    impl Plugin for Ask {
        fn build(&self, engine: &mut EngineBuilder) {
            for name in self.0 {
                engine
                    .net()
                    .rpc::<Echo, ()>(name)
                    .to_server()
                    .default_permission()
                    .server(|_cx: &mut RpcCx, _r: Echo| Ok(()));
            }
        }
    }
    impl crate::PluginType for Ask {
        const ID: PluginId = PluginId::new("test.ask");
    }
    macro_rules! plugin {
        ($ty:ident, $id:literal, $names:expr) => {{
            struct $ty;
            impl Plugin for $ty {
                fn build(&self, engine: &mut EngineBuilder) {
                    Ask($names).build(engine);
                }
            }
            impl crate::PluginType for $ty {
                const ID: PluginId = PluginId::new($id);
            }
            $ty
        }};
    }
    let refused = |builder: &mut EngineBuilder| builder.build().unwrap_err().to_string();

    // Plugin `a.b` with `c` and plugin `a` with `b.c` get different permissions (the plugin's id is one segment).
    let engine = EngineBuilder::new()
        .add_plugins(plugin!(Dotted, "a.b", &["c"]))
        .add_plugins(plugin!(Plain, "a", &["b.c"]))
        .build()
        .expect("distinct permissions");
    let table = engine.net_rpcs().unwrap();
    let permission = |name: &str| match &table.get(table.id(name).unwrap()).unwrap().allow {
        Allow::Permission(p) => p.clone(),
        _ => panic!("{name}: not a permission"),
    };
    assert_eq!(permission("c"), "rpc.a_b.c");
    assert_eq!(permission("b.c"), "rpc.a.b.c");
    // `a.b` and `a_b` clean to the same segment.
    let error = refused(
        EngineBuilder::new()
            .add_plugins(plugin!(Dotted2, "a.b", &["c"]))
            .add_plugins(plugin!(Under, "a_b", &["c"])),
    );
    assert!(error.contains("default permission"), "{error}");
    // Names that differ only in case or punctuation, in one plugin.
    let error = refused(EngineBuilder::new().add_plugins(Ask(&["Ping", "ping"])));
    assert!(error.contains("default permission"), "{error}");
    // Distinct names are fine, across plugins too.
    assert!(
        EngineBuilder::new()
            .add_plugins(plugin!(One, "a.b", &["c", "d"]))
            .add_plugins(plugin!(Two, "a", &["b.e"]))
            .build()
            .is_ok()
    );
}

#[test]
fn rpc_a_refused_batch_stays_queued_in_order_and_replies_count_against_the_queue() {
    let (engine, log) = engine();
    let mut pair = Pair::new(&engine);
    let target = pair.client.rpc::<Target, ()>("target").expect("the rpc");
    for _ in 0..3 {
        pair.client
            .notify(
                &target,
                Target {
                    who: torque_ghosts::GhostRef(5),
                },
            )
            .expect("queued");
    }
    pair.to_server.refuse.set(true);
    pair.client.flush();
    assert!(pair.to_server.take().is_empty());
    assert!(pair.client.send_queue_bytes() > 0, "the frames are kept");
    pair.set_tick(1);
    pair.to_server.refuse.set(false);
    pair.client_to_server().expect("server reads");
    assert_eq!(
        log.targets.get(),
        3,
        "all three arrived once the carrier took them"
    );
    assert_eq!(pair.client.send_queue_bytes(), 0);

    // The server's replies are queued and counted when the carrier refuses them, and arrive when it does not.
    let results: Vec<Slot<Echoed>> = (0..6).map(|_| call_echo(&mut pair, "echo", "a")).collect();
    pair.client.flush();
    for (batch, _) in pair.to_server.take() {
        pair.server.receive(&batch).expect("server reads");
    }
    pair.to_client.refuse.set(true);
    pair.run_server();
    assert!(
        pair.server.send_queue_bytes() > 0,
        "the replies count against the send queue"
    );
    assert!(results.iter().all(|r| r.borrow().is_none()));
    pair.to_client.refuse.set(false);
    pair.server.flush();
    let mut hooks = NoHooks;
    for (batch, _) in pair.to_client.take() {
        pair.client.receive(&batch).expect("client reads");
    }
    let mut env = RpcEnv {
        vm: &mut pair.vm,
        tick: 1,
        conn: None,
        perms: &NoPermissions,
        ghosts: &NoGhosts,
        hooks: &mut hooks,
    };
    pair.client.dispatch(&mut env);
    assert!(
        results
            .iter()
            .all(|r| matches!(r.borrow().clone(), Some(Ok(_))))
    );
}

#[test]
fn rpc_a_peer_that_never_takes_its_replies_is_overloaded() {
    let (engine, _) = engine();
    let mut pair = Pair::new(&engine);
    pair.server.set_send_queue(100);
    pair.to_client.refuse.set(true);
    for _ in 0..6 {
        let _ = call_echo(
            &mut pair,
            "echo",
            "a fairly long text so the replies add up",
        );
    }
    pair.client.flush();
    for (batch, _) in pair.to_server.take() {
        pair.server.receive(&batch).expect("server reads");
    }
    pair.run_server();
    assert!(pair.server.is_overloaded());
}

#[test]
fn rpc_the_send_queue_is_held_within_what_the_carrier_takes() {
    let (engine, _) = engine();
    let mut pair = Pair::new(&engine);
    pair.to_server.capacity.set(1_000);
    pair.client.set_send_queue(1_000_000);
    let blob = pair.client.rpc::<Blob, ()>("blob").expect("the rpc");
    let big = Blob {
        bytes: vec![1; 600],
    };
    assert_eq!(pair.client.notify(&blob, big.clone()), Ok(()));
    assert_eq!(
        pair.client.notify(&blob, big),
        Err(RpcError::Busy),
        "the carrier's 1,000 bytes, not the asked-for million"
    );
}

#[test]
fn rpc_what_a_side_sends_is_held_to_the_same_rates() {
    let (engine, _) = engine();
    let mut pair = Pair::new(&engine);
    pair.client.set_send_limits(true);
    pair.server.set_send_limits(true);
    let echo = pair.client.rpc::<Echo, Echoed>("echo").expect("the rpc");
    // `echo` is 4 a second with a burst of 8: the ninth call in a tick fails here, locally, and the server never saw it.
    let sent: Vec<_> = (0..12)
        .map(|_| pair.client.call(&echo, Echo::default(), |_| {}))
        .collect();
    assert_eq!(sent.iter().filter(|r| r.is_ok()).count(), 8);
    assert!(sent[8..].iter().all(|r| *r == Err(RpcError::RateLimited)));
    // A server's notifications to a client are held to the client's rates, so the client never drops one.
    let ping = pair.server.rpc::<Ping, ()>("ping").expect("the rpc");
    let sent: Vec<_> = (0..20)
        .map(|_| pair.server.notify(&ping, Ping::default()))
        .collect();
    assert_eq!(sent.iter().filter(|r| r.is_ok()).count(), 16);
    assert!(sent[16..].iter().all(|r| *r == Err(RpcError::RateLimited)));
    pair.round();
    assert_eq!(
        pair.client.drops("ping"),
        0,
        "nothing sent was dropped on arrival"
    );
    assert_eq!(pair.server.drops("echo"), 0);
    // Time refills it.
    pair.set_tick(100);
    assert!(pair.server.notify(&ping, Ping::default()).is_ok());
}

#[test]
fn rpc_the_connections_bucket_is_charged_only_when_the_rpcs_own_admits() {
    let (engine, log) = engine();
    // Sending: 10 tokens for the connection; `echo` has a burst of 8 and `target` of 16.
    let mut pair = Pair::new(&engine);
    pair.client.set_send_limits(true);
    pair.client.set_connection_rate(10, 10);
    let echo = pair.client.rpc::<Echo, Echoed>("echo").expect("the rpc");
    let target = pair.client.rpc::<Target, ()>("target").expect("the rpc");
    let calls: Vec<_> = (0..12)
        .map(|_| pair.client.call(&echo, Echo::default(), |_| {}))
        .collect();
    assert_eq!(calls.iter().filter(|r| r.is_ok()).count(), 8);
    // The four `echo` calls its own bucket refused took nothing from the connection's: two tokens are left.
    let ours = Target {
        who: torque_ghosts::GhostRef(5),
    };
    assert!(pair.client.notify(&target, ours.clone()).is_ok());
    assert!(pair.client.notify(&target, ours.clone()).is_ok());
    assert_eq!(
        pair.client.notify(&target, ours),
        Err(RpcError::RateLimited)
    );
    // Receiving, the same: a server with 10 connection tokens runs 8 echoes, and then 2 targets.
    let mut pair = Pair::new(&engine);
    pair.server.set_connection_rate(10, 10);
    let echo = pair.client.rpc::<Echo, Echoed>("echo").expect("the rpc");
    let target = pair.client.rpc::<Target, ()>("target").expect("the rpc");
    for _ in 0..12 {
        pair.client
            .call(&echo, Echo::default(), |_| {})
            .expect("queued");
    }
    for _ in 0..4 {
        pair.client
            .notify(
                &target,
                Target {
                    who: torque_ghosts::GhostRef(5),
                },
            )
            .expect("queued");
    }
    pair.round();
    assert_eq!(log.targets.get(), 2);
}

#[test]
fn rpc_the_kick_window_slides() {
    let (engine, _) = engine();
    let mut pair = Pair::new(&engine);
    pair.server.set_kick_drops(10);
    let target = pair.client.rpc::<Target, ()>("target").expect("the rpc");
    // 24 calls of `target` (a burst of 16, 8 a second) in one tick.
    let flood = |pair: &mut Pair, tick: u64| {
        pair.set_tick(tick);
        for _ in 0..24 {
            let who = torque_ghosts::GhostRef(5);
            pair.client.notify(&target, Target { who }).expect("queued");
        }
        pair.client_to_server().expect("server reads");
    };
    flood(&mut pair, 20);
    assert!(!pair.server.over_drop_limit(), "eight drops are under ten");
    flood(&mut pair, 40);
    assert!(pair.server.over_drop_limit());
    // 40 ticks after the first and 20 after the last: the window still holds the later drops. A window that restarted at a
    // fixed time would have forgotten them.
    pair.set_tick(60);
    assert!(pair.server.over_drop_limit());
    pair.set_tick(80);
    assert!(!pair.server.over_drop_limit(), "a second without a drop");
}

#[test]
fn rpc_a_default_permission_that_is_not_a_permission_says_why_at_registration() {
    struct Long(String);
    impl Plugin for Long {
        fn build(&self, engine: &mut EngineBuilder) {
            engine
                .net()
                .rpc::<Echo, ()>(&self.0)
                .to_server()
                .default_permission()
                .server(|_cx: &mut RpcCx, _r: Echo| Ok(()));
        }
    }
    impl crate::PluginType for Long {
        const ID: PluginId = PluginId::new("test.long");
    }
    let long = "a".repeat(MAX_RPC_NAME);
    let error = EngineBuilder::new()
        .add_plugins(Long(long))
        .build()
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("the default permission") && error.contains("shorten"),
        "{error}"
    );
    assert!(
        EngineBuilder::new()
            .add_plugins(Long("short".into()))
            .build()
            .is_ok()
    );
}

#[test]
fn rpc_the_tombstone_order_holds_each_live_id_once() {
    let (engine, _) = engine();
    let mut pair = Pair::new(&engine);
    pair.client.set_timeout(1);
    let rpc = pair.client.rpc::<Echo, Echoed>("echo").expect("the rpc");
    pair.client
        .call(&rpc, Echo::default(), |_| {})
        .expect("queued");
    pair.client.flush();
    let request = pair.to_server.take();
    pair.set_tick(1);
    pair.server_to_client().expect("timeout");
    assert_eq!(pair.client.tombstoned_calls(), [0]);
    // The late reply spends the tombstone, and with it its place in the order.
    for (batch, _) in request {
        pair.server.receive(&batch).expect("server reads");
    }
    pair.run_server();
    pair.server_to_client()
        .expect("a late reply is not an error");
    assert!(pair.client.tombstoned_calls().is_empty());
    assert_eq!(pair.client.tombstone_order_len(), 0);
    // The id is used again and times out again: one tombstone, one place in the order, so no eviction of the live one for
    // the sake of a stale entry.
    pair.client.set_next_call(0);
    pair.client
        .call(&rpc, Echo::default(), |_| {})
        .expect("queued");
    pair.set_tick(2);
    pair.server_to_client().expect("timeout");
    assert_eq!(pair.client.tombstoned_calls(), [0]);
    assert_eq!(pair.client.tombstone_order_len(), 1);
}
