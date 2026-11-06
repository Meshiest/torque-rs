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

#[test]
fn a_bind_on_the_iso_key_saves_and_loads_back_onto_it() {
    use crate::input::{Key, Taken};
    let (mut h, map) = host_with_map(
        "function iso(%v) { $iso = $iso @ %v; }
         TestMap.bind(keyboard, \"lessthan\", iso);",
    );
    assert_eq!(h.press(Key::IntlBackslash), Taken::Map(map));
    assert_ne!(h.press(Key::Backslash), Taken::Map(map));
    assert_eq!(h.server.vm().global("$iso").unwrap(), b"10");
    assert_eq!(
        h.server
            .vm_mut()
            .eval(b"return TestMap.getBinding(iso);")
            .unwrap(),
        b"keyboard\tlessthan"
    );
    // The map's lines as `save` prints them, run again on a new map of the same name.
    h.drain_log();
    h.server.vm_mut().eval(b"TestMap.save();").unwrap();
    let lines: Vec<u8> = h
        .drain_log()
        .into_iter()
        .flat_map(|l| l.text.into_iter().chain(*b"\n"))
        .collect();
    assert!(
        String::from_utf8_lossy(&lines).contains("\"lessthan\""),
        "{}",
        String::from_utf8_lossy(&lines)
    );
    let vm = h.server.vm_mut();
    vm.eval(b"TestMap.delete(); new ActionMap(TestMap);")
        .unwrap();
    vm.eval(&lines).unwrap();
    vm.eval(b"TestMap.push();").unwrap();
    let map = h.server.vm().objects().find(b"TestMap").unwrap();
    assert_eq!(h.press(Key::IntlBackslash), Taken::Map(map));
    assert_ne!(h.press(Key::Backslash), Taken::Map(map));
    assert_eq!(h.server.vm().global("$iso").unwrap(), b"1010");
    assert!(h.input_errors.is_empty(), "{:?}", h.input_errors);
}

/// A host that keeps TGE's key codes.
struct TgeKeys;

impl gui::GuiExtension for TgeKeys {
    fn into_any(self: Rc<Self>) -> Rc<dyn std::any::Any> {
        self
    }

    fn rule(&self, rule: gui::ext::GuiRule) -> bool {
        rule == gui::ext::GuiRule::TgeKeyCodes
    }
}

#[test]
fn pause_is_its_own_key_unless_the_gui_keeps_tge_s_key_codes() {
    use crate::input::{Key, Taken};
    let (mut h, map) = host_with_map(
        "function onPause(%v) { $pause = $pause @ %v; }
         function onNumLock(%v) { $numlock = $numlock @ %v; }
         TestMap.bind(keyboard, \"pause\", onPause);
         TestMap.bind(keyboard, \"numlock\", onNumLock);",
    );
    assert_eq!(h.press(Key::Pause), Taken::Map(map));
    assert_eq!(h.server.vm().global("$pause").unwrap(), b"10");
    h.gui.set_extension(Rc::new(TgeKeys));
    assert_eq!(h.press(Key::Pause), Taken::Map(map));
    assert_eq!(h.server.vm().global("$pause").unwrap(), b"10");
    assert_eq!(h.server.vm().global("$numlock").unwrap(), b"10");
    assert!(h.input_errors.is_empty(), "{:?}", h.input_errors);
}

/// A font whose printable characters are each 6 pixels wide.
struct EvenFonts;

impl fonts::Fallback for EvenFonts {
    fn make(&self, _face: &str, _size: i32) -> Option<fonts::Font> {
        let glyph = fonts::Glyph {
            x_increment: 6,
            ..fonts::Glyph::default()
        };
        Some(fonts::Font {
            height: 12,
            baseline: 10,
            ascent: 10,
            descent: 2,
            glyphs: vec![glyph; 95],
            sheets: Vec::new(),
            remap: fonts::Remap::new(32, (0..95).collect()),
            units: fonts::Units::Utf16,
            tab: fonts::Tab::Spaces,
        })
    }

    fn name(&self) -> &'static str {
        "even"
    }
}

/// Even fonts and the engine's edit keys; TGE's click count when `tge_count`.
struct EvenEdits {
    tge_count: bool,
}

impl gui::GuiExtension for EvenEdits {
    fn into_any(self: Rc<Self>) -> Rc<dyn std::any::Any> {
        self
    }

    fn fonts(&self) -> Option<Box<dyn gui::FontSource>> {
        Some(Box::new(fonts::FontCache::new(
            "fonts",
            Box::new(EvenFonts),
        )))
    }

    fn rule(&self, rule: gui::ext::GuiRule) -> bool {
        self.tge_count && rule == gui::ext::GuiRule::TgeClickCount
    }
}

/// A host with a text edit at (10, 10), 200 wide, holding "hello world" (6 pixels a character), run for a second.
fn host_with_edit(tge_count: bool) -> (Host, ts_vm::sim::ObjId) {
    let mut h = host();
    h.gui.set_extension(Rc::new(EvenEdits { tge_count }));
    h.server
        .vm_mut()
        .eval(
            b"new GuiControlProfile(EditProfile) { fontType = \"Arial\"; fontSize = 12; canKeyFocus = 1; tab = 1; };\
              Content.add(new GuiTextEditCtrl(Ed) { profile = \"EditProfile\"; position = \"10 10\"; extent = \"200 20\"; enabled = 1; text = \"hello world\"; });",
        )
        .unwrap();
    h.run(1000, |_| false);
    let ed = h.server.vm().objects().find(b"Ed").unwrap();
    (h, ed)
}

/// The edit's cursor and selection.
fn edit_state(h: &Host, ed: ts_vm::sim::ObjId) -> (i32, i32, i32) {
    h.gui
        .with(ed, |c| {
            let e = c.edit.as_ref().unwrap();
            (e.cursor, e.block_start, e.block_end)
        })
        .unwrap()
}

/// Where the gap before character `i` is drawn.
fn gap_x(h: &Host, ed: ts_vm::sim::ObjId, i: i32) -> i32 {
    let start = h
        .gui
        .with(ed, |c| c.edit.as_ref().and_then(|e| e.text_offset))
        .flatten()
        .unwrap_or(13);
    start + 6 * i
}

fn click(h: &mut Host, x: i32) {
    use crate::input::MouseButton;
    h.mouse_move((x, 15));
    h.mouse_button(MouseButton::Left, true, (x, 15));
    h.mouse_button(MouseButton::Left, false, (x, 15));
}

#[test]
fn a_click_in_a_selection_drops_it_and_places_the_cursor() {
    use crate::input::{Key, MouseButton};
    // Selected by a drag.
    let (mut h, ed) = host_with_edit(false);
    let (a, b) = (gap_x(&h, ed, 1), gap_x(&h, ed, 9));
    h.mouse_move((a, 15));
    h.mouse_button(MouseButton::Left, true, (a, 15));
    h.run(100, |_| false);
    h.mouse_move((b, 15));
    h.mouse_button(MouseButton::Left, false, (b, 15));
    assert_eq!(edit_state(&h, ed), (9, 1, 9));
    h.run(100, |_| false);
    let x = gap_x(&h, ed, 4);
    click(&mut h, x);
    assert_eq!(edit_state(&h, ed), (4, 0, 0), "after a drag");

    // Selected by Ctrl+A.
    h.run(100, |_| false);
    h.key(Key::ControlLeft, true);
    h.press(Key::A);
    h.key(Key::ControlLeft, false);
    assert_eq!(edit_state(&h, ed), (11, 0, 11));
    h.run(100, |_| false);
    let x = gap_x(&h, ed, 7);
    click(&mut h, x);
    assert_eq!(edit_state(&h, ed), (7, 0, 0), "after Ctrl+A");

    // Selected by a double click; a quick click elsewhere in the word is a click, not a triple click.
    h.run(800, |_| false);
    let x = gap_x(&h, ed, 8);
    click(&mut h, x);
    click(&mut h, x + 1);
    assert_eq!(
        edit_state(&h, ed),
        (11, 6, 11),
        "a double click selects the word"
    );
    h.run(100, |_| false);
    let x = gap_x(&h, ed, 10);
    click(&mut h, x);
    assert_eq!(edit_state(&h, ed), (10, 0, 0), "after a double click");

    // A drag from inside a selection selects from the press.
    h.run(800, |_| false);
    let x = gap_x(&h, ed, 2);
    click(&mut h, x);
    click(&mut h, x);
    assert_eq!(edit_state(&h, ed), (5, 0, 5));
    h.run(100, |_| false);
    let (a, b) = (gap_x(&h, ed, 3), gap_x(&h, ed, 8));
    h.mouse_move((a, 15));
    h.mouse_button(MouseButton::Left, true, (a, 15));
    h.mouse_move((b, 15));
    h.mouse_button(MouseButton::Left, false, (b, 15));
    assert_eq!(
        edit_state(&h, ed),
        (8, 3, 8),
        "a drag from inside a selection"
    );
    assert!(h.input_errors.is_empty(), "{:?}", h.input_errors);
}

#[test]
fn tge_s_click_count_takes_a_quick_press_anywhere_as_one_more_click() {
    let (mut h, ed) = host_with_edit(true);
    let x = gap_x(&h, ed, 8);
    click(&mut h, x);
    click(&mut h, x);
    assert_eq!(edit_state(&h, ed), (11, 6, 11));
    h.run(100, |_| false);
    let x = gap_x(&h, ed, 2);
    click(&mut h, x);
    assert_eq!(
        edit_state(&h, ed),
        (11, 0, 11),
        "a triple click 36 pixels away selects all"
    );
    assert!(h.input_errors.is_empty(), "{:?}", h.input_errors);
}
