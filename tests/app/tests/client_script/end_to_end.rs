//! A realm's messages and a server's script functions through a whole server and client: the engine's own over the memory
//! transport (what a game runs), with the realm's guest scripted. A realm's `net.send` arrives in the server's
//! `clientScriptOnMessage`, the server's `clientScriptSend` arrives in the realm as an event, and the player's answer is
//! heard in `clientScriptOnStatus`, each as text the server's script reads and never runs.

#[path = "../../../../crates/torque-plugin-client-script/tests/common/mod.rs"]
mod common;

use std::cell::RefCell;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::rc::Rc;

use common::{
    Asked, FRAME, Files, GuestFactory, Script, TestPrompt, TestSource, send_flush, table, text,
};
use torque::app::client::{ClientNet, GameClient};
use torque::app::server::Listen;
use torque::server::net::channels::{ChannelHub, ClientChannels};
use torque::server::net::rpc::{ClientRpc, ServerRpc};
use torque_client_script_format::{Grants, Tier, Value, event};
use torque_client_script_host::GrantSet;
use torque_engine::{EngineBuilder, Plugin, PluginId};
use torque_example_minimal_engine::{builder, client, files, server};
use torque_gui::Gui;
use torque_plugin_client_script::messages::{OFFER, OfferedMod, ScriptOffer};
use torque_plugin_client_script::{
    Choices, ClientScriptsPlugin, ConsentAnswer, RealmState, Realms, Setting,
};

const SERVER: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_300);
const CLIENT: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 28_301);

/// The control classes of a realm, which a game's client has.
#[derive(PluginId)]
#[plugin(id = "test.controls")]
struct Controls;

impl Plugin for Controls {
    fn build(&self, engine: &mut EngineBuilder) {
        torque_gui::client_script::add_controls(engine);
    }
}

struct Rig {
    client: GameClient,
    gui: Gui,
    realms: Realms,
    script: Rc<Script>,
    asked: Rc<Asked>,
    got: Rc<RefCell<Vec<Value>>>,
    server_rpc: Rc<ServerRpc>,
    now_us: u64,
}

impl Rig {
    /// One client step (the server's with it) and one frame of the realm.
    fn step(&mut self) {
        self.client.step(32);
        self.now_us += 32_000;
        let vm = self.client.vm_mut();
        self.realms.frame(vm, &self.gui, self.now_us, FRAME);
    }

    fn steps_until(&mut self, what: &str, done: impl Fn(&Rig) -> bool) {
        for _ in 0..2_000 {
            if done(self) {
                return;
            }
            self.step();
        }
        panic!("{what} did not happen: {:?}", self.client.client_log);
    }

    fn server_eval(&mut self, code: &str) -> String {
        let vm = self.client.server.as_mut().expect("hosting").vm_mut();
        String::from_utf8_lossy(&vm.eval(code.as_bytes()).expect("the server's script runs"))
            .into_owned()
    }

    fn server_global(&self, name: &str) -> Option<String> {
        let vm = self.client.server.as_ref().expect("hosting").vm();
        vm.global(name)
            .map(|v| String::from_utf8_lossy(&v).into_owned())
    }

    /// Whether the realm was told of a message from the server that holds `needle`.
    fn realm_heard(&self, needle: &[u8]) -> bool {
        self.script.events.borrow().iter().any(|batch| {
            batch.contains(&event::net::MESSAGE) && batch.windows(needle.len()).any(|w| w == needle)
        })
    }
}

/// A server and a client of the engine's own example game, the client with a realm whose player has allowed `ui` and `net`.
fn start() -> Rig {
    let game_files = files().expect("the example's files");
    let mut b = builder(&game_files).expect("the builder");
    let got: Rc<RefCell<Vec<Value>>> = Rc::default();
    let sink = Rc::clone(&got);
    b.add_plugins((
        ClientScriptsPlugin::new().on_message(move |_vm, _conn, value| {
            sink.borrow_mut().push(value.clone());
        }),
        Controls,
    ));
    let engine = b.build().expect("the engine builds");
    let server = server(&engine, Listen::Memory(SERVER)).expect("the server starts");
    let server_rpc = server
        .vm()
        .extension::<ServerRpc>()
        .expect("the server has RPCs");
    let mut client = client(
        &engine,
        ClientNet::Memory {
            client: CLIENT,
            server: SERVER,
        },
    )
    .expect("the client");
    let rpc = client
        .vm()
        .extension::<ClientRpc>()
        .expect("the client has RPCs");
    let channels = client.vm().extension::<ClientChannels>().expect("channels");
    let gui = client.host.gui.clone();
    client.start_hosting(server);
    client
        .call("connectTo", &[&SERVER.to_string()])
        .expect("connecting");
    for _ in 0..2_000 {
        if channels.is_active() && rpc.is_ready() {
            break;
        }
        client.step(32);
    }
    assert!(rpc.is_ready(), "the client's RPC link starts");
    client.step(64);
    let conn = *client
        .server
        .as_ref()
        .expect("hosting")
        .vm()
        .extension::<ChannelHub>()
        .expect("the hub")
        .clients()
        .first()
        .expect("one client");

    let script = Rc::new(Script::default());
    let asked = Rc::new(Asked::default());
    let files_here = Rc::new(Files::default());
    files_here.here.set(true);
    let mut realms = Realms::new(
        Choices::in_memory(),
        Some(Box::new(GuestFactory(Rc::clone(&script)))),
    );
    realms.setting = Setting::Ask;
    realms.offerable = Grants::of(&[Tier::Ui, Tier::Net]);
    realms.set_prompt(Box::new(TestPrompt(Rc::clone(&asked))));
    realms.set_source(Box::new(TestSource(files_here)));
    realms.set_grant_set(|_, allowed| GrantSet::unreviewed(allowed));
    let mut rig = Rig {
        client,
        gui,
        realms,
        script,
        asked,
        got,
        server_rpc,
        now_us: 1_000_000,
    };

    // The server's script functions, as a game writes them.
    rig.server_eval(
        "function clientScriptOnMessage(%client, %text) { $Test::msg = %text; $Test::from = %client; $Test::count = $Test::count + 1; }\n\
         function clientScriptOnStatus(%client, %state, %tiers) { $Test::state = %state; $Test::tiers = %tiers; }",
    );
    // The offer, as the server's mods would make it, and the player's answer.
    let offer = rig
        .server_rpc
        .with_link(conn, |l| l.rpc::<ScriptOffer, ()>(OFFER))
        .expect("a link")
        .expect("the offer rpc");
    let mods = [OfferedMod {
        id: "hud".into(),
        hash: [7; 32],
        entry: "client/main.lua".into(),
        tiers: vec!["ui".into(), "net".into()],
    }];
    rig.server_rpc
        .notify(conn, &offer, ScriptOffer::of(&mods))
        .expect("queued");
    rig.steps_until("the question", |r| !r.asked.requests.borrow().is_empty());
    *rig.asked.answer.borrow_mut() = Some(ConsentAnswer::Allow {
        ticked: vec![(Tier::Ui, true), (Tier::Net, true)],
        remember: false,
    });
    rig.steps_until("the realm running", |r| {
        r.realms.state() == RealmState::Running
    });
    rig
}

#[test]
fn the_players_answer_is_heard_by_the_servers_script() {
    let mut rig = start();
    rig.steps_until("the status", |r| r.server_global("$Test::state").is_some());
    let tiers = rig.server_global("$Test::tiers").expect("the tiers");
    assert!(tiers.contains("ui") && tiers.contains("net"), "{tiers}");
}

#[test]
fn a_realms_message_arrives_as_text_in_the_servers_handler_and_runs_nothing() {
    let mut rig = start();
    let value = table(&[("hp", Value::Number(3.0)), ("who", text("me"))]);
    rig.script
        .flushes
        .borrow_mut()
        .push_back(send_flush(&value));
    rig.steps_until("the message", |r| r.server_global("$Test::msg").is_some());
    let msg = rig.server_global("$Test::msg").expect("the text");
    assert!(
        msg.contains("\"hp\":3") && msg.contains("\"who\":\"me\""),
        "a table arrives as JSON text: {msg}"
    );
    let from = rig.server_global("$Test::from").expect("the sender");
    assert_eq!(rig.server_eval("return ClientGroup.getObject(0);"), from);
    assert_eq!(rig.got.borrow().as_slice(), [value], "and the Rust handler");

    // A string made to look like script is still only the argument.
    let hostile = "\"); $Test::pwned = 1; (\"";
    rig.script
        .flushes
        .borrow_mut()
        .push_back(send_flush(&text(hostile)));
    rig.steps_until("the second message", |r| {
        r.server_global("$Test::count").as_deref() == Some("2")
    });
    assert_eq!(rig.server_global("$Test::msg").as_deref(), Some(hostile));
    assert_eq!(rig.server_global("$Test::pwned"), None, "nothing ran");
}

#[test]
fn the_servers_send_arrives_in_the_realm_as_an_event() {
    let mut rig = start();
    let answer =
        rig.server_eval("return clientScriptSend(ClientGroup.getObject(0), \"potion drunk\");");
    assert_eq!(answer, "", "queued without a complaint");
    rig.steps_until("the realm hearing it", |r| r.realm_heard(b"potion drunk"));
    // A client that is not one is told so; the realm hears nothing more.
    let before = rig.script.events.borrow().len();
    assert_eq!(
        rig.server_eval("return clientScriptSend(12345, \"x\");"),
        "no such client"
    );
    for _ in 0..10 {
        rig.step();
    }
    assert!(!rig.realm_heard(b"x\0") || rig.script.events.borrow().len() >= before);
}

#[test]
fn a_realm_that_does_not_hold_net_hears_nothing_and_its_message_ends_it() {
    let mut rig = start();
    rig.realms.disconnect(rig.client.vm_mut(), &rig.gui);
    assert_eq!(rig.realms.state(), RealmState::Idle);
    // With no realm, what the server sends down is dropped on arrival.
    let answer =
        rig.server_eval("return clientScriptSend(ClientGroup.getObject(0), \"nobody home\");");
    assert_eq!(answer, "");
    for _ in 0..20 {
        rig.step();
    }
    assert!(!rig.realm_heard(b"nobody home"));
    assert_eq!(rig.server_global("$Test::count"), None);
}
