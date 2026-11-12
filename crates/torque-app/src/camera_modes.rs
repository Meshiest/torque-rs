//! Camera modes: named ways to find the eye from the control object, the frame's time and the input, that plugins add and the
//! client picks between by name.
//!
//! | Item | What it is |
//! |---|---|
//! | [`CameraMode`] | the trait a mode implements: `eye` (and `update`, `keep`, `blend_in`/`blend_out`, `enter`/`leave`, `reset`) |
//! | [`CameraModes`] | a client's modes, the one in use and the eased move from one to the next ([`CameraBlend`](crate::camera_blend::CameraBlend)) |
//! | [`CameraCx`] | what a mode reads: the control object ([`OwnView`]), the frame's time, the frame's input and the world's surroundings |
//! | [`CameraWorld`] | what the game answers for the camera to keep clear of walls and ground (the cubes, the interiors, the terrain) |
//! | [`CameraModesPlugin`] | `torque.camera-modes` (in [`DefaultPlugins`](crate::plugins::DefaultPlugins)): each client's [`CameraModes`] and `setCameraMode`, `getCameraMode` and `getCameraModes` |
//! | [`CameraBuilderExt::add_camera_mode`] | how a plugin adds a mode while the engine is built |
//!
//! **Built in** ([`FIRST_PERSON`], [`THIRD_PERSON`], [`ORBIT`]): the first person looks from the figure's eyes; the third
//! person rides a boom ([`CameraBoom`](crate::camera_boom::CameraBoom)) back from the figure along its look, kept in front of what is in the way
//! ([`ThirdPerson`], shaped by a [`Rig`]), looking where the figure looks ([`camera_behind`](crate::camera_behind)); the orbit
//! circles the figure ([`Orbit`](crate::orbit::Orbit)) turned by the frame's look and zoomed by its wheel. A client starts in the first person,
//! as TGE's does. A game that wants its own numbers adds a mode under the same name (the later one replaces the earlier).
//!
//! **A frame** is the same for a game's own client and a plugin's mode. The game makes a [`CameraCx`] and calls
//! [`begin`](CameraModes::begin) (a switch since the last frame starts the ease; the ease's clock moves on),
//! [`update`](CameraModes::update) (every mode moves on its state: a boom follows even while another mode is in use, so that
//! a switch to it finds it in place), and [`drawn`](CameraModes::drawn): the eye of the mode in use, eased from the last
//! mode's after a switch. [`target`](CameraModes::target) is the mode's own eye with no ease. A new figure or a teleport is a
//! [`cut`](CameraModes::cut): every mode forgets what it followed and the camera is at the new place at once.
//!
//! **A switch** keeps the camera's motion and eases the eye ([`CameraBlend`](crate::camera_blend::CameraBlend): a critically damped spring on the eye's offset
//! from the new mode's eye, so a switch while walking or driving never lags behind). Its time is the incoming mode's
//! [`blend_in`](CameraMode::blend_in), else the outgoing mode's [`blend_out`](CameraMode::blend_out), else
//! [`SETTLE_SECONDS`](crate::camera_blend::SETTLE_SECONDS); 0 cuts. A mode that [`keeps_look`](CameraMode::keeps_look) has its own look drawn whatever the ease did:
//! the ease moves the camera and never turns the view.
//!
//! **Scripts**: `setCameraMode(name)` picks a mode (1 if there is one by that name, else 0 and a warning in the console);
//! `getCameraMode()` is the name of the mode in use; `getCameraModes()` lists the names, space separated. Names are lower
//! case letters, digits, `-` and `_`; a script's spelling may differ in case.
//!
//! ```
//! use torque_app::camera_modes::{CameraBuilderExt, CameraCx, CameraMode, CameraModes};
//! use torque_engine::{EngineBuilder, Plugin, PluginId};
//! use torque_render_scene::source::Eye;
//!
//! /// A camera that hangs 40 m above the origin, looking down.
//! struct Overhead;
//!
//! impl CameraMode for Overhead {
//!     fn eye(&self, _: &CameraCx<'_>) -> Option<Eye> {
//!         Some(Eye { position: [0.0, 0.0, 40.0], yaw: 0.0, pitch: std::f32::consts::FRAC_PI_2, fov: None })
//!     }
//! }
//!
//! #[derive(PluginId)]
//! #[plugin(id = "example.overhead")]
//! struct OverheadPlugin;
//!
//! impl Plugin for OverheadPlugin {
//!     fn build(&self, engine: &mut EngineBuilder) {
//!         engine.add_camera_mode("overhead", || Overhead);
//!     }
//! }
//!
//! let engine = EngineBuilder::new().add_plugins(OverheadPlugin).build()?;
//! let modes = CameraModes::of_engine(&engine);
//! assert!(modes.set("overhead"));
//! assert_eq!(modes.active(), "overhead");
//! # Ok::<(), torque_engine::BuildError>(())
//! ```
//!
//! Credit: the third person's boom and the first person's eye follow TGE's `ShapeBase::getCameraTransform` and
//! `ShapeBase::getEyeTransform` as Torque3D (MIT) keeps them (each Copyright (c) 2012 GarageGames, LLC). The code is this
//! crate's own.

mod builtin;
mod registry;

use std::any::Any;
use std::rc::Rc;

use torque_engine::{ClientStage, EngineBuilder, Plugin};
use torque_render_scene::source::Eye;
use torque_ts_vm::vm::{Native, Vm};

pub use builtin::{FirstPerson, OrbitMode, Rig, ThirdPerson};
pub use registry::{CameraModes, valid_name};

use crate::ghosts::OwnView;

/// The name of the built-in first person: the eye at the figure's eyes. A client starts in it.
pub const FIRST_PERSON: &str = "first-person";
/// The name of the built-in third person: on a boom behind the figure.
pub const THIRD_PERSON: &str = "third-person";
/// The name of the built-in orbit: circling the figure, turned and zoomed by the frame's input.
pub const ORBIT: &str = "orbit";

/// The most modes a client keeps (a mode added past it is dropped with a warning), so a runaway plugin cannot grow the list
/// without end.
pub const MAX_MODES: usize = 64;
/// The longest a mode's name is (bytes).
pub const MAX_NAME: usize = 48;

/// What the frame's input did to the camera, summed since the last frame.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CameraInput {
    /// The look turned: yaw (positive to the right) and pitch (positive down), radians.
    pub look: [f32; 2],
    /// The wheel's notches (positive away from the user: in).
    pub zoom: f32,
}

/// What a mode reads in a frame.
#[derive(Clone, Copy)]
pub struct CameraCx<'a> {
    /// The client's own figure as the frame placed it (none before it, or with none).
    pub control: Option<&'a OwnView>,
    /// The frame's time (seconds).
    pub dt: f32,
    /// The frame's input.
    pub input: CameraInput,
    /// What the world offers a camera that keeps clear of it.
    pub world: &'a dyn CameraWorld,
}

impl<'a> CameraCx<'a> {
    /// A frame's context over `world`, with no figure and no input.
    pub fn new(world: &'a dyn CameraWorld, dt: f32) -> CameraCx<'a> {
        CameraCx {
            control: None,
            dt,
            input: CameraInput::default(),
            world,
        }
    }

    /// With the figure `control`.
    pub fn with_control(mut self, control: Option<&'a OwnView>) -> CameraCx<'a> {
        self.control = control;
        self
    }

    /// With the frame's `input`.
    pub fn with_input(mut self, input: CameraInput) -> CameraCx<'a> {
        self.input = input;
        self
    }

    /// The world as the game's own type `T`, for a mode that reads more of it than [`CameraWorld`] tells (none: the game's
    /// world is another type).
    pub fn world_as<T: CameraWorld>(&self) -> Option<&'a T> {
        let any: &'a dyn Any = self.world;
        any.downcast_ref::<T>()
    }
}

/// What the game answers for a camera to keep clear of: a line of sight through its cubes, interiors, terrain and water.
/// Every question has an answer for a world with nothing in it (nothing in the way), so a game gives only what it has.
pub trait CameraWorld: Any {
    /// The share (0 to 1) of the way from `from` to `to` at which the line first meets something the camera keeps out of, if it
    /// does. Used by the boom.
    fn clear_share(&self, _from: [f32; 3], _to: [f32; 3]) -> Option<f32> {
        None
    }

    /// How far along the unit `dir` from `from` the line first meets something the camera keeps out of, within `reach`.
    fn clear_hit(&self, _from: [f32; 3], _dir: [f32; 3], _reach: f32) -> Option<f32> {
        None
    }

    /// The height of the ground (or water) at `x`, `y`, if there is any.
    fn ground(&self, _x: f32, _y: f32) -> Option<f32> {
        None
    }
}

/// A world with nothing in it.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenWorld;

impl CameraWorld for OpenWorld {}

/// A way to find the eye. Only [`eye`](CameraMode::eye) is required.
pub trait CameraMode: Any {
    /// The eye this mode gives now, from its state and the frame (none: no view, as with no figure to follow).
    fn eye(&self, cx: &CameraCx<'_>) -> Option<Eye>;

    /// The mode moves on a frame (its boom follows, its orbit turns). Called each frame on every mode, `active` for the one
    /// in use; see [`CameraModes::update`].
    fn update(&mut self, _cx: &CameraCx<'_>, _active: bool) {}

    /// The mode becomes the one in use (the frame the game's `begin` sees the switch).
    fn enter(&mut self, _cx: &CameraCx<'_>) {}

    /// The mode stops being the one in use.
    fn leave(&mut self) {}

    /// The seconds an ease into this mode takes (none: the default, [`SETTLE_SECONDS`](crate::camera_blend::SETTLE_SECONDS); 0 cuts).
    fn blend_in(&self) -> Option<f32> {
        None
    }

    /// The seconds an ease out of this mode takes when the mode it goes to names none.
    fn blend_out(&self) -> Option<f32> {
        None
    }

    /// `point`, a place the ease puts the camera, moved out of walls and ground as this mode's own camera is kept.
    fn keep(&self, _cx: &CameraCx<'_>, point: [f32; 3]) -> [f32; 3] {
        point
    }

    /// Whether the view this mode draws is its own look whatever the ease did: the ease moves the camera only.
    fn keeps_look(&self, _cx: &CameraCx<'_>) -> bool {
        false
    }

    /// A new figure or a teleport: forget what the mode followed.
    fn reset(&mut self) {}
}

/// A mode a plugin added, made for each client.
pub struct CameraModeEntry {
    name: String,
    make: Box<dyn Fn() -> Box<dyn CameraMode>>,
}

impl CameraModeEntry {
    /// The name the mode is picked by.
    pub fn name(&self) -> &str {
        &self.name
    }

    fn make(&self) -> Box<dyn CameraMode> {
        (self.make)()
    }
}

/// Adds camera modes to an engine being built.
pub trait CameraBuilderExt {
    /// Adds the mode `make` makes under `name` (lower case letters, digits, `-` and `_`); each client the engine makes gets its
    /// own. A name taken twice (or a built-in's) is the later one's. A bad name is a warning in the client's console and the
    /// mode is left out.
    fn add_camera_mode<M: CameraMode>(
        &mut self,
        name: &str,
        make: impl Fn() -> M + 'static,
    ) -> &mut Self;
}

impl CameraBuilderExt for EngineBuilder {
    fn add_camera_mode<M: CameraMode>(
        &mut self,
        name: &str,
        make: impl Fn() -> M + 'static,
    ) -> &mut Self {
        self.push(CameraModeEntry {
            name: name.to_string(),
            make: Box::new(move || Box::new(make())),
        })
    }
}

/// Each client's [`CameraModes`] (the built-in modes and the ones plugins added) and the console functions `setCameraMode`,
/// `getCameraMode` and `getCameraModes`. In [`DefaultPlugins`](crate::plugins::DefaultPlugins).
///
/// ```
/// use torque_app::camera_modes::{CameraModesPlugin, FIRST_PERSON};
/// use torque_engine::EngineBuilder;
///
/// let engine = EngineBuilder::new().add_plugins(CameraModesPlugin).build()?;
/// assert!(engine.is_plugin_added::<CameraModesPlugin>());
/// assert_eq!(torque_app::camera_modes::CameraModes::of_engine(&engine).active(), FIRST_PERSON);
/// # Ok::<(), torque_engine::BuildError>(())
/// ```
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.camera-modes")]
pub struct CameraModesPlugin;

impl Plugin for CameraModesPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.client(ClientStage::Natives, |vm, engine| {
            let modes = Rc::new(CameraModes::from_entries(
                engine.list::<CameraModeEntry>(),
                |warning| vm.warn(warning),
            ));
            register_natives(vm, &modes);
            vm.set_extension(modes);
            Ok(())
        });
    }
}

impl CameraModes {
    /// A client's modes, as the script engine `vm` was made with them (none: its engine has no [`CameraModesPlugin`]).
    pub fn of(vm: &Vm) -> Option<Rc<CameraModes>> {
        vm.extension::<CameraModes>()
    }

    /// The modes a client of `engine` starts with (the built-ins and the plugins'), outside a client: for a test, or a game
    /// that runs modes without a script engine. Warnings are dropped.
    pub fn of_engine(engine: &torque_engine::Engine) -> CameraModes {
        CameraModes::from_entries(engine.list::<CameraModeEntry>(), |_| {})
    }
}

fn register_natives(vm: &mut Vm, modes: &Rc<CameraModes>) {
    let m = Rc::clone(modes);
    vm.register(
        "setCameraMode",
        2,
        2,
        "(name) - Use the camera mode called name (see getCameraModes); 1 if there is one, else 0.",
        move |vm, call| {
            let name = String::from_utf8_lossy(call.cstr(1)).into_owned();
            let found = m.set(name.trim());
            if !found {
                vm.warn(format!("setCameraMode: no camera mode called \"{name}\"."));
            }
            Native::Bool(found)
        },
    );
    let m = Rc::clone(modes);
    vm.register(
        "getCameraMode",
        1,
        1,
        "() - The name of the camera mode in use.",
        move |_, _| Native::Str(m.active().into_bytes()),
    );
    let m = Rc::clone(modes);
    vm.register(
        "getCameraModes",
        1,
        1,
        "() - The names of the camera modes, separated by spaces.",
        move |_, _| Native::Str(m.names().join(" ").into_bytes()),
    );
}

#[cfg(test)]
#[path = "camera_modes_tests.rs"]
mod tests;
