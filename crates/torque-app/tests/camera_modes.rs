//! Camera modes that plugins add: a test plugin's mode is switched to at run time (from a script and from Rust) and the eyes
//! it gives are asserted frame by frame, eased from the mode before; the built-in modes are there by default; a script's bad
//! name changes nothing.

use std::cell::Cell;
use std::rc::Rc;

use torque_app::camera_modes::{
    CameraBuilderExt, CameraCx, CameraMode, CameraModes, FIRST_PERSON, OpenWorld, THIRD_PERSON,
};
use torque_app::ghosts::OwnView;
use torque_app::plugins::DefaultPlugins;
use torque_app::{ClientBuilder, ClientNet, GameClient, GameServer};
use torque_engine::{EngineBuilder, Plugin, PluginGroup as _, PluginId};
use torque_render_scene::source::Eye;

/// A camera `HEIGHT` above the figure, looking straight down, that turns with the frame's look and sinks as time passes.
struct Crane {
    turned: f32,
    seconds: f32,
    left: Rc<Cell<u32>>,
}

const HEIGHT: f32 = 12.0;

impl CameraMode for Crane {
    fn eye(&self, cx: &CameraCx<'_>) -> Option<Eye> {
        let f = cx.control?;
        Some(Eye {
            position: [
                f.position[0],
                f.position[1],
                f.position[2] + HEIGHT - self.seconds,
            ],
            yaw: self.turned,
            pitch: std::f32::consts::FRAC_PI_2,
            fov: None,
        })
    }

    fn update(&mut self, cx: &CameraCx<'_>, active: bool) {
        if active {
            self.turned += cx.input.look[0];
            self.seconds += cx.dt;
        }
    }

    fn leave(&mut self) {
        self.left.set(self.left.get() + 1);
    }

    fn blend_in(&self) -> Option<f32> {
        Some(0.5)
    }
}

#[derive(PluginId)]
#[plugin(id = "test.crane")]
struct CranePlugin(Rc<Cell<u32>>);

impl Plugin for CranePlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        let left = Rc::clone(&self.0);
        engine.add_camera_mode("crane", move || Crane {
            turned: 0.0,
            seconds: 0.0,
            left: Rc::clone(&left),
        });
    }
}

fn client(left: &Rc<Cell<u32>>) -> GameClient<GameServer> {
    let engine = EngineBuilder::new()
        .add_plugins((DefaultPlugins, CranePlugin(Rc::clone(left))))
        .build()
        .expect("the engine builds");
    ClientBuilder::from_engine(&engine, None, ClientNet::None)
        .expect("the client")
        .build::<GameServer>()
}

fn figure() -> OwnView {
    OwnView {
        ghost: 1,
        position: [10.0, 20.0, 5.0],
        yaw: 0.0,
        pitch: 0.0,
        eye: [10.0, 20.0, 6.6],
        drawn_eye: [10.0, 20.0, 6.6],
        fall: 0.0,
        fall_moved: [0.0; 3],
        cut: false,
    }
}

/// One frame of `dt` seconds as a game runs it, over the figure `f`: the eye drawn.
fn frame(modes: &CameraModes, f: &OwnView, dt: f32) -> Eye {
    let cx = CameraCx::new(&OpenWorld, dt)
        .with_control(Some(f))
        .with_input(modes.take_input());
    modes.begin(&cx);
    modes.update(&cx);
    modes.drawn(&cx, fallback())
}

fn fallback() -> Eye {
    Eye {
        position: [0.0, 0.0, 100.0],
        yaw: 0.0,
        pitch: 0.0,
        fov: None,
    }
}

#[test]
fn a_plugins_mode_is_listed_beside_the_built_in_ones_and_a_client_starts_in_the_first_person() {
    let mut game = client(&Rc::default());
    let modes = game.camera_modes().expect("the client has camera modes");
    assert_eq!(
        modes.names(),
        ["first-person", "third-person", "orbit", "crane"]
    );
    assert_eq!(modes.active(), FIRST_PERSON);
    assert_eq!(
        game.call("getCameraModes", &[]).unwrap(),
        "first-person third-person orbit crane"
    );
    assert_eq!(game.call("getCameraMode", &[]).unwrap(), "first-person");
}

#[test]
fn a_script_switches_to_the_plugins_mode_at_run_time_and_the_eye_eases_there() {
    let left = Rc::new(Cell::new(0));
    let mut game = client(&left);
    let modes = game.camera_modes().unwrap();
    let f = figure();

    // The first person: the figure's eyes, exactly.
    modes.set_now(FIRST_PERSON);
    let first = frame(&modes, &f, 0.016);
    assert_eq!(first.position, f.eye);

    // The script picks the plugin's mode; the next frame starts the ease from the first person's eye.
    assert_eq!(game.call("setCameraMode", &["crane"]).unwrap(), "1");
    assert_eq!(game.call("getCameraMode", &[]).unwrap(), "crane");
    let start = frame(&modes, &f, 0.016);
    assert!(modes.blending());
    assert!(
        start.position[2] > f.eye[2] && start.position[2] < f.position[2] + HEIGHT,
        "between the two: {start:?}"
    );
    assert_eq!((start.position[0], start.position[1]), (10.0, 20.0));

    // The ease takes the mode's own time (95% of the way in 0.5 s, all of it a little later), then the eye is the mode's, exactly, and keeps sinking with the frame time.
    let mut crane_seconds = 0.016f32;
    let mut seconds = 0.016;
    while modes.blending() {
        frame(&modes, &f, 0.016);
        crane_seconds += 0.016;
        seconds += 0.016;
        assert!(seconds < 2.0, "the ease ends");
    }
    assert!((0.5..1.6).contains(&seconds), "{seconds}");
    let arrived = frame(&modes, &f, 0.5);
    crane_seconds += 0.5;
    assert_eq!(
        arrived.position,
        [10.0, 20.0, f.position[2] + HEIGHT - crane_seconds]
    );
    assert_eq!(arrived.pitch, std::f32::consts::FRAC_PI_2);

    // Back by script: the mode is told it was left, and the first person's eye is the figure's again once the ease ends.
    assert_eq!(game.call("setCameraMode", &["First-Person"]).unwrap(), "1");
    frame(&modes, &f, 0.016);
    assert_eq!(left.get(), 1);
    for _ in 0..200 {
        frame(&modes, &f, 0.016);
    }
    assert!(!modes.blending());
    assert_eq!(frame(&modes, &f, 0.016).position, f.eye);
}

#[test]
fn the_look_input_the_game_pushes_turns_the_plugins_mode() {
    let mut game = client(&Rc::default());
    let modes = game.camera_modes().unwrap();
    let f = figure();
    modes.set_now("crane");
    modes.push_input([0.25, 0.0], 0.0);
    modes.push_input([0.25, 0.0], 0.0);
    let eye = frame(&modes, &f, 0.016);
    assert_eq!(eye.yaw, 0.5);
    // The input was taken: the next frame turns no further.
    assert_eq!(frame(&modes, &f, 0.016).yaw, 0.5);
    let _ = &mut game;
}

#[test]
fn a_name_no_mode_has_changes_nothing_and_says_so_in_the_console() {
    let mut game = client(&Rc::default());
    assert_eq!(game.call("setCameraMode", &["sideways"]).unwrap(), "0");
    assert_eq!(game.call("getCameraMode", &[]).unwrap(), "first-person");
    let log: String = game
        .vm_mut()
        .drain_log()
        .iter()
        .map(|l| String::from_utf8_lossy(&l.text).into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(log.contains("no camera mode called \"sideways\""), "{log}");
}

#[test]
fn a_rust_call_switches_too_and_each_client_has_its_own_modes() {
    let left = Rc::new(Cell::new(0));
    let (a, b) = (client(&left), client(&left));
    let (ma, mb) = (a.camera_modes().unwrap(), b.camera_modes().unwrap());
    assert!(ma.set(THIRD_PERSON));
    assert_eq!(ma.active(), THIRD_PERSON);
    assert_eq!(mb.active(), FIRST_PERSON);
    assert!(!Rc::ptr_eq(&ma, &mb));
}

#[test]
fn an_engine_without_the_plugin_has_no_camera_modes_and_no_script_functions() {
    use torque_app::plugins::CameraModesPlugin;
    let engine = EngineBuilder::new()
        .add_plugins(DefaultPlugins.disable::<CameraModesPlugin>())
        .build()
        .expect("the engine builds");
    let game = ClientBuilder::from_engine(&engine, None, ClientNet::None)
        .expect("the client")
        .build::<GameServer>();
    assert!(game.camera_modes().is_none());
    assert!(!game.vm().is_native_function(b"getCameraMode"));
}
