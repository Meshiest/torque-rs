use super::*;
use crate::screen::TgeScreen;

#[test]
fn full_screen_takes_the_monitors_mode_of_that_size_nearest_the_rate() {
    let mode = ScreenMode {
        width: 1280,
        height: 1024,
        bpp: 32,
        hz: 60,
        full_screen: true,
        borderless: false,
    };
    let modes = [
        (1920, 1080, 32, 60_000),
        (1280, 1024, 32, 75_025),
        (1280, 1024, 16, 60_000),
        (1280, 1024, 32, 59_940),
        (1280, 1024, 32, 60_000),
    ];
    assert_eq!(exclusive_mode(&mode, &modes), Some(4));
    assert_eq!(exclusive_mode(&mode, &modes[..4]), Some(3));
    assert_eq!(exclusive_mode(&mode, &modes[..1]), None, "borderless then");
}

#[test]
fn a_fresh_profile_starts_at_720p_and_a_saved_resolution_is_kept() {
    let on = |size| Monitor {
        size,
        video_modes: vec![size],
    };
    let canvas = (800, 600);
    assert_eq!(
        start_size(true, canvas, Some(&on((2560, 1440))), &TgeScreen),
        (1280, 720)
    );
    // A saved resolution, or no monitor known: the canvas's (the pref's or the default).
    assert_eq!(
        start_size(false, canvas, Some(&on((2560, 1440))), &TgeScreen),
        canvas
    );
    assert_eq!(start_size(true, canvas, None, &TgeScreen), canvas);
}

mod frame_input {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use torque_server::server::Server;
    use ts_vm::vm::Vm;

    use super::*;
    use crate::conn::tge::TgeConn;
    use crate::host::EnginePass;
    use crate::input_source::{InputCx, InputSource, JoyAxis};
    use crate::state::ClientState;

    type Host = ClientHost<TgeConn, Server>;

    /// A source whose X axis is a new value at every poll, so a bound function is called once per poll; it counts its polls.
    struct Counting(Rc<Cell<u32>>);

    impl InputSource for Counting {
        fn poll(&mut self, cx: &mut InputCx<'_>) {
            self.0.set(self.0.get() + 1);
            cx.axis(0, JoyAxis::X, self.0.get() as f32 / 100_000.0);
        }
    }

    /// A host whose `joystick0` X axis calls `onX`, which counts its calls in `$calls`; and the source's poll count.
    fn host() -> (Host, Rc<Cell<u32>>) {
        let mut vm = Vm::new();
        let gui = gui::register(&mut vm);
        let client = Rc::new(RefCell::new(ClientState::<TgeConn>::default()));
        crate::actionmap::register(&mut vm, &client);
        vm.register_class("ActionMap", "SimObject", &[]);
        vm.eval(
            b"$calls = 0; function onX(%v) { $calls++; }
              new ActionMap(TestMap); TestMap.bind(joystick0, xaxis, onX); TestMap.push();",
        )
        .unwrap();
        let polls = Rc::new(Cell::new(0));
        client
            .borrow_mut()
            .input_sources
            .add(Box::new(Counting(Rc::clone(&polls))));
        let cache = gui::DrawCache::new(Box::new(fonts::FontCache::new(
            "fonts",
            Box::new(fonts::NoFallback),
        )));
        let host = ClientHost::new(Server::new(vm), gui, client, Rc::new(EnginePass), cache);
        (host, polls)
    }

    fn calls(host: &Host) -> u32 {
        host.server.vm().global_int("$calls").unwrap_or(0)
    }

    /// A game that runs the host's clock as the windowed games do.
    struct Fake(Host);

    impl WindowGame for Fake {
        type Conn = TgeConn;
        type Server = Server;

        fn host(&mut self) -> &mut Host {
            &mut self.0
        }

        fn host_ref(&self) -> &Host {
            &self.0
        }

        fn title(&self) -> String {
            String::new()
        }

        fn screen_rules(&self) -> &dyn ScreenRules {
            &TgeScreen
        }

        fn window_made(&mut self, _: &Arc<Window>, _: (u32, u32)) -> Result<(), String> {
            Ok(())
        }

        fn resized(&mut self, _: (u32, u32)) {}

        fn step(&mut self, ms: u32) {
            self.0.run(ms, |_| false);
        }

        fn draw(&mut self) -> Result<Option<bool>, String> {
            Ok(None)
        }

        fn vsync(&self) -> bool {
            true
        }
    }

    #[test]
    fn a_bound_axis_is_called_once_per_frame_not_once_per_tick() {
        let (host, polls) = host();
        let mut game = Fake(host);
        let mut owed = 0.0;
        // Two seconds of a fake wall clock at 144 frames a second: about 62 ticks of 32 ms, 288 frames.
        let frame_ms = 1000.0 / 144.0;
        for _ in 0..288 {
            clock_pass(&mut game, &mut owed, frame_ms);
        }
        assert_eq!(polls.get(), 288);
        assert_eq!(calls(&game.0), 288);
        assert!(game.0.server.vm().sim_time() >= 1_900, "the clock ran");
        assert!(!game.0.frame_polled);
        assert!(game.0.input_errors.is_empty());
    }

    #[test]
    fn a_frame_that_runs_several_ticks_polls_once() {
        let (host, polls) = host();
        let mut game = Fake(host);
        let mut owed = 0.0;
        // One slow frame of 160 ms: five ticks, one poll.
        clock_pass(&mut game, &mut owed, 160.0);
        assert_eq!(polls.get(), 1);
        assert_eq!(calls(&game.0), 1);
    }

    #[test]
    fn a_frame_too_short_to_run_a_millisecond_still_polls() {
        let (host, polls) = host();
        let mut game = Fake(host);
        let mut owed = 0.0;
        clock_pass(&mut game, &mut owed, 0.4);
        assert_eq!(polls.get(), 1);
        assert_eq!(game.0.server.vm().sim_time(), 0);
    }

    #[test]
    fn a_host_that_draws_no_frames_polls_once_per_tick() {
        let (mut host, polls) = host();
        host.run(3_000, |_| false);
        let ticks = polls.get();
        assert!(ticks >= 60, "a poll before every tick: {ticks}");
        assert_eq!(calls(&host), ticks);
        // The ticks of a frame that polled leave the sources alone; once the frame's input has ended they poll again.
        host.poll_input_frame();
        assert_eq!(polls.get(), ticks + 1);
        host.run(640, |_| false);
        assert_eq!(polls.get(), ticks + 1);
        host.end_input_frame();
        host.run(640, |_| false);
        assert!(polls.get() >= ticks + 1 + 15);
    }
}
