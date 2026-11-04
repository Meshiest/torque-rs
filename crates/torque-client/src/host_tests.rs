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
