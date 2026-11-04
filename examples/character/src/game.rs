//! The example's client: a client host (script engine, GUI and action maps; no connection) whose canvas is a `GameTSCtrl` with a
//! help line, the figure and its state machine ([`crate::motion`]), and the orbit camera.
//!
//! **Input.** The keys and the mouse go through the host's platform layer to an action map (`CharacterMap`, the script below),
//! whose functions set script globals; each step reads them: `W A S D` move, `Shift` runs, `Space` jumps, the mouse (the
//! cursor is off, so the window holds it) or `Q`/`E` orbit, the wheel zooms, `Escape` quits.
//!
//! **A frame** ([`CharacterGame::frame`]): the figure as the world view's moving object, the canvas drawn (its `GameTSCtrl`
//! leaves the world's rectangle in the draws), and the eye: the orbit camera around the figure's `cam` node.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use torque::client::conn::tge::TgeConn;
use torque::client::host::{ClientHost, EnginePass};
use torque::client::input::{WM_KEYDOWN, WM_KEYUP, key_lparam};
use torque::client::state::ClientState;
use torque::gui::GuiDraws;
use torque::script::text::atof;
use torque::script::vm::vm::Vm;
use torque::server::server::{GameServer as _, Server};
use torque::shapes::instance::ShapeInstance;
use torque::shapes::math::mul_p;
use torque_example_common::orbit::Orbit;
use torque_example_common::view::{SceneShape, WorldView};
use torque_example_common::{KitAssets, draw_cache, kit_path};
use torque_render_core::Gpu;
use torque_render_scene::math::Mat;
use torque_render_scene::source::Eye;

use crate::motion::{Figure, Input, State};

/// The figure's id among the frame's moving objects.
pub const FIGURE_ID: u32 = 1;
pub const PISTOL_ID: u32 = 2;
/// The floor: its half side and how often its texture repeats (units).
pub const FLOOR_HALF: f32 = 400.0;
pub const FLOOR_TILE: f32 = 4.0;
/// Radians the camera turns for a mouse count, and a second with `Q` or `E` held.
pub const MOUSE_TURN: f32 = 0.005;
pub const KEY_TURN: f32 = 2.0;
/// How much one wheel notch (120 counts) moves the camera in or out.
pub const ZOOM_STEP: f32 = 0.9;

/// The virtual keys the example binds.
pub mod keys {
    pub const W: u8 = 0x57;
    pub const A: u8 = 0x41;
    pub const S: u8 = 0x53;
    pub const D: u8 = 0x44;
    pub const Q: u8 = 0x51;
    pub const E: u8 = 0x45;
    pub const SHIFT: u8 = 0x10;
    pub const SPACE: u8 = 0x20;
    pub const ESCAPE: u8 = 0x1b;
    pub const F: u8 = 0x46;
}

/// The example's game folder: `main.cs` builds the canvas, its profiles and the action map.
pub fn game_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("game")
}

/// The example's client.
pub struct CharacterGame {
    pub host: ClientHost<TgeConn, Server>,
    assets: KitAssets,
    shape: SceneShape,
    figure: Figure,
    /// The pistol the figure holds (`F`), and its pose.
    pistol: SceneShape,
    pistol_pose: ShapeInstance,
    pub orbit: Orbit,
    /// The `cam` node (the point the camera circles).
    cam: Option<usize>,
}

impl CharacterGame {
    pub fn new() -> Result<CharacterGame, String> {
        let assets = KitAssets::new().map_err(|e| format!("the asset kit: {e}"))?;
        let shape =
            SceneShape::load(&assets, &kit_path("figure.dts")).ok_or("the figure does not load")?;
        let detail = shape.detail().ok_or("the figure has no detail")?;
        let figure = Figure::new(shape.shape(), detail).ok_or("the figure lacks a sequence")?;
        let cam = shape.node("cam");
        let pistol =
            SceneShape::load(&assets, &kit_path("pistol.dts")).ok_or("the pistol does not load")?;
        let pistol_pose = pistol.instance().ok_or("the pistol does not pose")?;
        let mut vm = Vm::new();
        let gui = torque::gui::register(&mut vm);
        let state = Rc::new(RefCell::new(ClientState::<TgeConn>::default()));
        // The engine's natives on ActionMap; the class itself is the game's to declare (TGE's is a plain SimObject).
        vm.register_class("ActionMap", "SimObject", &[]);
        torque::client::actionmap::register(&mut vm, &state);
        torque::client::screen::register(&mut vm, &gui, &state);
        let main = game_dir().join("main.cs");
        let script = std::fs::read(&main).map_err(|e| format!("main.cs: {e}"))?;
        vm.eval(&script).map_err(|e| format!("main.cs: {e}"))?;
        let mut host = ClientHost::new(
            Server::new(vm),
            gui,
            state,
            Rc::new(EnginePass),
            draw_cache(),
        );
        host.set_windowed();
        Ok(CharacterGame {
            host,
            assets,
            shape,
            figure,
            pistol,
            pistol_pose,
            orbit: Orbit::default(),
            cam,
        })
    }

    /// A world view on `gpu` with the example's floor.
    pub fn view(&self, gpu: &Gpu) -> WorldView {
        let mut view = WorldView::new(gpu);
        view.set_floor(&self.assets, &kit_path("grid"), FLOOR_HALF, FLOOR_TILE);
        view
    }

    pub fn figure(&self) -> &Figure {
        &self.figure
    }

    /// A key going down or up, as the window posts it.
    pub fn key(&mut self, vk: u8, down: bool) {
        let msg = if down { WM_KEYDOWN } else { WM_KEYUP };
        self.host
            .post(msg, u32::from(vk), key_lparam(vk, !down, false));
    }

    /// A global the action map's functions set, as a number (0 when unset).
    fn number(&self, name: &str) -> f32 {
        self.host
            .server
            .vm()
            .global(name)
            .map_or(0.0, |v| atof(&v) as f32)
    }

    /// A global the mouse adds to, taken (set back to 0).
    fn take(&mut self, name: &str) -> f32 {
        let v = self.number(name);
        if v != 0.0 {
            self.host.server.vm_mut().set_global(name, "0");
        }
        v
    }

    /// `ms` of the clock: the host's pass (its input, layout and scripts), then the camera and the figure.
    pub fn step(&mut self, ms: u32) {
        self.host.run(ms, |_| false);
        let dt = ms as f32 / 1000.0;
        let input = Input {
            forward: self.number("$charForward") - self.number("$charBack"),
            right: self.number("$charRight") - self.number("$charLeft"),
            run: self.number("$charRun") != 0.0,
            jump: self.number("$charJump") != 0.0,
        };
        let keys_turn = self.number("$charTurnRight") - self.number("$charTurnLeft");
        let (yaw, pitch, zoom) = (
            self.take("$charYaw"),
            self.take("$charPitch"),
            self.take("$charZoom"),
        );
        self.orbit.turn(
            yaw * MOUSE_TURN + keys_turn * KEY_TURN * dt,
            pitch * MOUSE_TURN,
            ZOOM_STEP.powf(zoom / 120.0),
        );
        let holding = self.number("$charPistol") != 0.0;
        self.figure.set_holding(self.shape.shape(), holding);
        let (forward, right) = (self.orbit.forward(), self.orbit.right());
        if self
            .figure
            .step(self.shape.shape(), &input, forward, right, dt)
        {
            self.show_state(self.figure.state());
        }
    }

    fn show_state(&mut self, state: State) {
        let name = state.name().as_bytes().to_vec();
        if let Err(e) = self
            .host
            .server
            .vm_mut()
            .invoke(None, b"showState", &[name])
        {
            eprintln!("example-character: {e}");
        }
    }

    /// The point the camera circles: the figure's `cam` node where the figure stands.
    pub fn camera_target(&self) -> [f32; 3] {
        let local = self
            .cam
            .and_then(|n| self.figure.instance().node_transforms().get(n))
            .map_or([0.0, 0.0, 1.8], |m| mul_p(m, [0.0; 3]));
        self.figure.transform().mul_p(local)
    }

    /// Where the pistol is while the figure holds it: its `mountPoint` on the figure's `mount0`.
    pub fn held_pistol(&self) -> Option<Mat> {
        if !self.figure.holding() {
            return None;
        }
        let hand = self.shape.node("mount0")?;
        let grip = self.pistol.node("mountPoint")?;
        let hand = Mat {
            m: *self.figure.instance().node_transforms().get(hand)?,
        };
        let grip = Mat {
            m: *self.pistol_pose.node_transforms().get(grip)?,
        };
        Some(Mat::mul(
            &Mat::mul(&self.figure.transform(), &hand),
            &grip.affine_inverse(),
        ))
    }

    pub fn eye(&self) -> Eye {
        self.orbit.eye(self.camera_target())
    }

    /// The frame: the figure into `view`, the canvas drawn (with the world's rectangle) and the eye.
    pub fn frame(&mut self, view: &mut WorldView) -> (GuiDraws, Eye) {
        let figure = view.shape_object(
            &self.assets,
            &self.shape,
            self.figure.instance(),
            &self.figure.transform(),
            FIGURE_ID,
        );
        let mut objects = vec![figure];
        if let Some(held) = self.held_pistol() {
            objects.push(view.shape_object(
                &self.assets,
                &self.pistol,
                &self.pistol_pose,
                &held,
                PISTOL_ID,
            ));
        }
        view.set_objects(objects);
        // No connection gives the GameTSCtrl a camera: the example's own camera is the view's.
        self.host.gui.0.borrow_mut().world_camera = true;
        (self.host.draw_frame(), self.eye())
    }
}
