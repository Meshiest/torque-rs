use std::cell::RefCell;
use std::rc::Rc;

use torque_server::server::Server;

use super::*;
use crate::conn::tge::TgeConn;
use crate::state::Queued;

type Host = ClientHost<TgeConn, Server>;

/// A client host over a bare server with a canvas, the screen natives and TGE's connection half.
fn host() -> Host {
    let mut vm = Vm::new();
    let gui = gui::register(&mut vm);
    let client = Rc::new(RefCell::new(ClientState::<TgeConn>::default()));
    screen::register(&mut vm, &gui, &client);
    vm.eval(b"createCanvas(\"test\"); new GuiControl(Content) { extent = \"640 480\"; }; Canvas.setContent(Content);")
        .unwrap();
    let cache = gui::DrawCache::new(Box::new(fonts::FontCache::new(
        "fonts",
        Box::new(fonts::NoFallback),
    )));
    ClientHost::new(Server::new(vm), gui, client, Rc::new(EnginePass), cache)
}

#[test]
fn the_clock_runs_the_queued_callbacks_before_each_tick() {
    let mut h = host();
    h.server
        .vm_mut()
        .eval(b"function Content::poke(%this, %n) { $poked = $poked + %n; }")
        .unwrap();
    let content = h.server.vm().objects().find(b"Content").unwrap();
    h.client.borrow_mut().queue.push_back(Queued {
        object: content,
        method: b"poke".to_vec(),
        args: vec![b"2".to_vec()],
    });
    h.run(64, |_| false);
    assert!(h.server.vm().sim_time() >= 64);
    assert_eq!(h.server.vm().global("$poked").unwrap(), b"2");
    assert!(h.client.borrow().queue.is_empty());
}

#[test]
fn a_resize_is_a_screen_mode_and_the_canvas_follows() {
    let mut h = host();
    h.set_windowed();
    h.window_resized((1024, 768));
    assert_eq!(h.gui.0.borrow().canvas_size, (1024, 768));
    let mode = h.screen_mode();
    assert_eq!((mode.width, mode.height), (1024, 768));
    assert!(
        h.server
            .vm()
            .global("$pref::Video::resolution")
            .unwrap()
            .starts_with(b"1024 768")
    );
    let draws = h.draw_frame();
    assert_eq!(draws.size, (1024, 768));
}

#[test]
fn the_log_names_the_objects_scripts_could_not_find() {
    let mut h = host();
    let _ = h.server.vm_mut().eval(b"NoSuchThing.doIt();");
    h.drain_log();
    assert_eq!(h.missing_objects, ["NoSuchThing.doIt"]);
    assert_eq!(
        missing_object(b"x: Unable to find object: 'A' attempting to call function 'b'").as_deref(),
        Some("A.b")
    );
}

#[test]
fn the_engine_pass_hands_a_server_query_answer_to_the_query() {
    use std::net::{Ipv4Addr, SocketAddrV4};

    use ::net::serverquery::{GAME_PING_REQUEST, GAME_PING_RESPONSE, request};

    use crate::net::{BROADCAST, DatagramQueue, MemoryDatagrams, NetPolicy};

    let mut h = host();
    let (sent, incoming) = (DatagramQueue::default(), DatagramQueue::default());
    {
        let st = h.client.borrow();
        let mut net = st.net.borrow_mut();
        net.policy = NetPolicy::Loopback;
        net.socket = Some(Box::new(MemoryDatagrams {
            sent: Rc::clone(&sent),
            incoming: Rc::clone(&incoming),
            local_port: 50_000,
        }));
    }
    crate::query::register(h.server.vm_mut(), &h.client);
    // A ping goes out when its last send is more than the time-out old: past the clock's start.
    h.run(1024, |_| false);
    h.server
        .vm_mut()
        .eval(b"queryLanServers(28000, 0, \"\", \"\", 0, 100, 0, 2, 0, 100, 0);")
        .unwrap();
    let pings: Vec<_> = sent.borrow_mut().drain(..).collect();
    assert!(
        pings
            .iter()
            .any(|(to, d)| *to == SocketAddrV4::new(BROADCAST, 28000) && d[0] == GAME_PING_REQUEST),
        "the LAN ping went to the broadcast address"
    );
    // A server on the loopback address answers the broadcast: the query pings it in turn on its next pass.
    let server = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 28000);
    incoming
        .borrow_mut()
        .push_back((server, request(GAME_PING_RESPONSE, 0, 1, 1)));
    h.run(64, |_| false);
    assert!(
        sent.borrow()
            .iter()
            .any(|(to, d)| *to == server && d[0] == GAME_PING_REQUEST),
        "the answer reached the query, which pinged the server"
    );
    assert_eq!(h.client.borrow().net.borrow().stats.stray, 0);
}

/// A host with ActionMap (the class is the game's to declare: TGE's is a plain SimObject), the global map pushed first and
/// `TestMap` on top of it, bound by `binds` (`TestMap.bind(...)` lines).
fn host_with_map(binds: &str) -> (Host, u32) {
    let mut h = host();
    crate::actionmap::register(h.server.vm_mut(), &h.client);
    let vm = h.server.vm_mut();
    vm.register_class("ActionMap", "SimObject", &[]);
    let script = format!(
        "new ActionMap(GlobalActionMap); GlobalActionMap.push(); new ActionMap(TestMap); {binds} TestMap.push();"
    );
    vm.eval(script.as_bytes()).unwrap();
    let map = h.server.vm().objects().find(b"TestMap").unwrap();
    (h, map)
}

#[test]
fn a_named_key_reaches_the_action_map_bound_to_it() {
    use crate::input::{Key, MouseButton, Taken};
    let (mut h, map) = host_with_map(
        "function moveForward(%v) { $fwd = $fwd @ %v; }
         function fire(%v) { $fire = $fire @ %v; }
         TestMap.bind(keyboard, \"w\", moveForward);
         TestMap.bind(mouse, \"button0\", fire);",
    );
    assert_eq!(h.press(Key::W), Taken::Map(map));
    assert_eq!(h.server.vm().global("$fwd").unwrap(), b"10");
    assert_eq!(h.key(Key::W, true), Taken::Map(map));
    assert_eq!(h.server.vm().global("$fwd").unwrap(), b"101");
    h.key(Key::W, false);
    // With the canvas's cursor off (in game) the buttons go past the canvas to the maps.
    h.gui.0.borrow_mut().cursor_on = false;
    assert_eq!(
        h.mouse_button(MouseButton::Left, true, (5, 5)),
        Taken::Map(map)
    );
    h.mouse_button(MouseButton::Left, false, (5, 5));
    assert_eq!(h.server.vm().global("$fire").unwrap(), b"10");
    assert!(h.input_errors.is_empty(), "{:?}", h.input_errors);
}

#[test]
fn typed_text_presses_the_keys_with_shift_where_needed() {
    let (mut h, _) = host_with_map(
        "function bigH(%v) { if (%v) $typed = $typed @ \"H\"; }
         function smallI(%v) { if (%v) $typed = $typed @ \"i\"; }
         function bang(%v) { if (%v) $typed = $typed @ \"bang\"; }
         TestMap.bind(keyboard, \"shift h\", bigH);
         TestMap.bind(keyboard, \"i\", smallI);
         TestMap.bind(keyboard, \"shift 1\", bang);",
    );
    assert_eq!(h.type_text("Hi!\u{e9}"), Err('\u{e9}'));
    assert_eq!(h.server.vm().global("$typed").unwrap_or_default(), b"");
    h.type_text("Hi!").unwrap();
    assert_eq!(h.server.vm().global("$typed").unwrap(), b"Hibang");
    assert_eq!(h.win32.modifiers, 0, "Shift let go");
    assert!(h.input_errors.is_empty(), "{:?}", h.input_errors);
}
