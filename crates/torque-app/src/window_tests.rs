//! The window's runner over a game's choices, with no window made: what [`AppWindow`] answers the window driver each pass is
//! the game's ([`WindowChoices`], [`GameApp::upkeep`], [`GameApp::pass_work`]), the defaults are the window's own, and a frame
//! counts the frame average only when the game chose it.

use torque_client::platform::window::WindowGame;
use torque_engine::EngineBuilder;
use torque_gui::GuiDraws;
use torque_render_scene::source::Eye;

use super::*;
use crate::client::{ClientBuilder, ClientNet, GameClient};
use crate::orbit::Orbit;
use crate::plugins::DefaultPlugins;
use crate::{App, GameServer, Host};

/// The cap's global a test game names.
const CAP_PREF: &str = "$pref::Test::frameCap";

/// A client of a plain engine, no window or device.
fn client() -> GameClient<GameServer> {
    let engine = EngineBuilder::new()
        .add_plugins(DefaultPlugins)
        .build()
        .expect("the engine builds");
    ClientBuilder::from_engine(&engine, None, ClientNet::None)
        .expect("the client")
        .build()
}

/// A game whose window choices are `choices` (none of its own: the defaults), counting its upkeeps and noting its passes' work.
struct Chooser {
    client: GameClient<GameServer>,
    choices: Option<WindowChoices<'static>>,
    upkeeps: u32,
    work: Vec<Duration>,
}

impl Chooser {
    fn new(choices: Option<WindowChoices<'static>>) -> Chooser {
        Chooser {
            client: client(),
            choices,
            upkeeps: 0,
            work: Vec::new(),
        }
    }
}

impl App for Chooser {
    fn host(&mut self) -> &mut Host {
        &mut self.client.host
    }

    fn host_ref(&self) -> &Host {
        &self.client.host
    }

    fn title(&self) -> String {
        "chooser".to_string()
    }

    fn log_name(&self) -> &str {
        "chooser"
    }

    fn view(&self, gpu: &Gpu) -> WorldView {
        WorldView::new(gpu)
    }

    fn advance(&mut self, ms: u32) {
        self.client.step(ms);
    }

    fn frame(&mut self, _: &mut WorldView) -> (GuiDraws, Eye) {
        (
            self.client.host.draw_frame(),
            Orbit::default().eye([0.0; 3]),
        )
    }

    fn window_choices(&self) -> WindowChoices<'_> {
        self.choices.unwrap_or_default()
    }

    fn upkeep(&mut self) {
        self.upkeeps += 1;
    }

    fn pass_work(&mut self, work: Duration) {
        self.work.push(work);
    }
}

/// The global `name` of the game's script engine.
fn global(game: &Chooser, name: &str) -> Option<Vec<u8>> {
    game.client.host.server.vm.global(name)
}

#[test]
fn a_game_that_makes_no_choices_gets_the_windows_own() {
    assert_eq!(
        WindowChoices::default(),
        WindowChoices {
            fresh_resolution: false,
            frame_cap_pref: None,
            may_grab: true,
            vsync: true,
            fps_update: false,
        }
    );
    let w = AppWindow::new(Chooser::new(None));
    assert!(!w.fresh_resolution());
    assert_eq!(w.frame_cap_pref(), None);
    assert!(w.may_grab());
    assert!(w.vsync());
}

#[test]
fn the_window_answers_the_driver_with_the_games_choices_read_each_pass() {
    let choices = WindowChoices {
        fresh_resolution: true,
        frame_cap_pref: Some(CAP_PREF),
        may_grab: false,
        vsync: false,
        fps_update: true,
    };
    let mut w = AppWindow::new(Chooser::new(Some(choices)));
    assert!(w.fresh_resolution());
    assert_eq!(w.frame_cap_pref(), Some(CAP_PREF));
    assert!(!w.may_grab());
    assert!(!w.vsync());
    // A script turning vertical sync back on: the next pass's answer follows.
    w.game.choices = Some(WindowChoices {
        vsync: true,
        ..choices
    });
    assert!(w.vsync());
}

#[test]
fn each_pass_runs_the_games_upkeep_and_hands_it_the_passs_work() {
    let mut w = AppWindow::new(Chooser::new(None));
    w.upkeep();
    w.upkeep();
    w.pass_work(Duration::from_millis(3));
    w.pass_work(Duration::from_millis(7));
    assert_eq!(w.game.upkeeps, 2);
    assert_eq!(
        w.game.work,
        [Duration::from_millis(3), Duration::from_millis(7)]
    );
}

#[test]
fn a_frame_counts_the_frame_average_only_when_the_game_chose_it() {
    let mut plain = Chooser::new(None);
    let mut chosen = Chooser::new(Some(WindowChoices {
        fps_update: true,
        ..WindowChoices::default()
    }));
    // Two seconds of frames, 100 ms apart: the average is due several times over.
    for i in 0..=20 {
        let real_ms = f64::from(i) * 100.0;
        crate::count_frame(&mut plain, real_ms);
        crate::count_frame(&mut chosen, real_ms);
    }
    for game in [&plain, &chosen] {
        assert!(global(game, "$Client::FrameRate").is_some());
    }
    assert_eq!(global(&plain, "$fps::real"), None);
    let real = global(&chosen, "$fps::real").expect("the average is set");
    let real: f32 = String::from_utf8_lossy(&real)
        .trim()
        .parse()
        .expect("a number");
    // The average starts at nothing and weighs each new frame by 0.07: above the 10 a second it settles toward.
    assert!(real >= 10.0, "{real} frames a second");
}
