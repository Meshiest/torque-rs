//! The client's zoom: TGE's `game.cc` functions and `GameConnection`'s camera fov:
//!
//! - **The console functions.** `setFov(fov)` sets the target: `atof` clamped to 1..179 (a NaN is 1). `setDefaultFov(fov)` keeps
//!   the default (clamped the same way) and makes it the target when the camera is not zooming. `setZoomSpeed(ms)`: `atoi` clamped
//!   to 0..2000 (0 a snap; a game may raise the floor, [`ZoomRules`]), the milliseconds of a 90 degree change. Scripts read the
//!   camera's fov as `$cameraFov` (a float variable, 90 at start); a zoom key's script sets the target and steps it.
//! - **Each render of the game view** (in `GameTSCtrl::processCameraQuery`) the camera's fov moves towards the target by
//!   `90 * elapsed / zoomSpeed` degrees on the virtual clock (snapping when the speed is 0 or no time passed), is handed to the
//!   connection (`setControlCameraFov`: clamped to 1..179; the update flag armed when its whole part changed), and `$cameraFov` is
//!   set. The frame is drawn with that fov.
//! - **The connection.** The client's packet carries the fov (8 bits, `clamp((int)fov, 1, 179)`) when the flag is armed. A fov the
//!   server sends is applied and snaps the camera and the target to it (the flag cleared when it was taken whole). A new camera
//!   object (non-null) resets both to 90 (`setCameraObject`, `getDefaultCameraFov`).
//!
//! Credit: follows TGE's zoom functions and `GameConnection`'s fov as Torque3D carries them (MIT: `T3D/gameFunctions.cpp`,
//! Copyright (c) 2012 GarageGames, LLC; `T3D/gameBase/gameConnection.cpp`, Copyright (c) 2012 GarageGames, LLC; Copyright (C)
//! 2015 Faust Logic, Inc.).

use std::fmt::Debug;
use std::rc::Rc;

use ts_vm::vm::{Native, Vm};

/// The fov's limits, the zoom speed's, a camera's default.
pub const MIN_FOV: f32 = 1.0;
pub const MAX_FOV: f32 = 179.0;
pub const MIN_ZOOM_SPEED: i32 = 0;
pub const MAX_ZOOM_SPEED: i32 = 2000;
pub const DEFAULT_FOV: f32 = 90.0;

/// A game's zoom rules (a `Vm` extension: [`set_zoom_rules`]). Every method has TGE's answer by default.
pub trait ZoomRules: Debug {
    /// `setZoomSpeed`'s lowest speed (TGE's: [`MIN_ZOOM_SPEED`]).
    fn min_speed(&self) -> i32 {
        MIN_ZOOM_SPEED
    }
}

/// TGE's rules.
#[derive(Debug, Default)]
pub struct TgeZoom;

impl ZoomRules for TgeZoom {}

struct Installed(Rc<dyn ZoomRules>);

/// The game's zoom rules for `vm` from now on.
pub fn set_zoom_rules(vm: &mut Vm, rules: Rc<dyn ZoomRules>) {
    vm.set_extension(Rc::new(Installed(rules)));
}

/// The zoom rules `vm`'s game set, else TGE's.
pub fn zoom_rules(vm: &Vm) -> Rc<dyn ZoomRules> {
    vm.extension::<Installed>().map_or_else(
        || Rc::new(TgeZoom) as Rc<dyn ZoomRules>,
        |i| Rc::clone(&i.0),
    )
}

/// `setZoomSpeed`'s speed of `atoi`'s `i`: at most [`MAX_ZOOM_SPEED`], at least `min`.
pub fn zoom_speed(i: i64, min: i32) -> i32 {
    i.min(i64::from(MAX_ZOOM_SPEED)).max(i64::from(min)) as i32
}

/// The clamp: over 179 is 179, over 1 stays, anything else (a NaN too) is 1.
pub fn clamp_fov(v: f32) -> f32 {
    if v > MAX_FOV {
        MAX_FOV
    } else if v > MIN_FOV {
        v
    } else {
        MIN_FOV
    }
}

/// The zoom's globals (`sCameraFov`, `sTargetFov`, `sDefaultFov`, `sZoomSpeed`, the last update).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zoom {
    pub camera: f32,
    pub target: f32,
    pub default: f32,
    pub speed: i32,
    pub last: f32,
}

impl Default for Zoom {
    fn default() -> Zoom {
        Zoom {
            camera: DEFAULT_FOV,
            target: DEFAULT_FOV,
            default: DEFAULT_FOV,
            speed: 500,
            last: 0.0,
        }
    }
}

impl Zoom {
    /// The camera's fov moved towards the target for the virtual time `now` (ms), as each render does.
    pub fn step(&mut self, now: f32) {
        if self.target != self.camera {
            let delta = now - self.last;
            if self.speed == 0 || delta <= 0.0 || delta.is_nan() {
                self.camera = self.target;
            } else {
                let step = (f64::from(delta) / f64::from(self.speed as f32) * 90.0) as f32;
                if self.camera > self.target {
                    self.camera -= step;
                    if self.camera < self.target {
                        self.camera = self.target;
                    }
                } else {
                    self.camera += step;
                    if self.camera > self.target {
                        self.camera = self.target;
                    }
                }
            }
        }
        self.last = now;
    }
}

/// The connection's side: the control object's fov (`mCameraFov`, 90 at construction), the update flag, and what the last check
/// saw of the server's fov and camera object.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConnFov {
    pub camera: f32,
    pub update: bool,
    pub seen_server: u32,
    pub seen_camera_object: Option<u32>,
}

impl Default for ConnFov {
    fn default() -> ConnFov {
        ConnFov {
            camera: DEFAULT_FOV,
            update: false,
            seen_server: 0,
            seen_camera_object: None,
        }
    }
}

impl ConnFov {
    /// `setControlCameraFov`: clamped, the flag armed when the whole part changed.
    pub fn set(&mut self, fov: f32) {
        let f = clamp_fov(fov);
        if f as i32 != self.camera as i32 {
            self.update = true;
        }
        self.camera = f;
    }

    /// The fov the client's next packet carries (the flag taken): `clamp((int)fov, 1, 179)`.
    pub fn take(&mut self) -> Option<u8> {
        std::mem::take(&mut self.update).then(|| (self.camera as i32).clamp(1, 179) as u8)
    }
}

/// `$cameraFov` as the console prints a float variable.
pub fn camera_fov_text(fov: f32) -> String {
    ts_text::format_g(f64::from(fov))
}

/// `setFov`, `setDefaultFov`, `setZoomSpeed` and `$cameraFov` (90).
pub fn register<G: crate::conn::ConnGame>(
    vm: &mut Vm,
    client: &std::rc::Rc<std::cell::RefCell<crate::state::ClientState<G>>>,
) {
    vm.set_global("$cameraFov", &camera_fov_text(DEFAULT_FOV));
    let state = std::rc::Rc::clone(client);
    vm.register(
        "setFov",
        2,
        2,
        "(fov) - Set the FOV of the camera.",
        move |_, c| {
            let v = c.float(1);
            state.borrow_mut().zoom.target = clamp_fov(v);
            Native::Void
        },
    );
    let state = std::rc::Rc::clone(client);
    vm.register(
        "setDefaultFov",
        2,
        2,
        "(defaultFov) - Set the default FOV for a camera.",
        move |_, c| {
            let v = clamp_fov(c.float(1));
            let mut st = state.borrow_mut();
            st.zoom.default = v;
            if st.zoom.camera == st.zoom.target {
                st.zoom.target = v;
            }
            Native::Void
        },
    );
    let state = std::rc::Rc::clone(client);
    vm.register(
        "setZoomSpeed",
        2,
        2,
        "(speed) - Set the zoom speed of the camera, in ms per 90deg FOV change.",
        move |vm, c| {
            let i = i64::from(c.int(1));
            state.borrow_mut().zoom.speed = zoom_speed(i, zoom_rules(vm).min_speed());
            Native::Void
        },
    );
}

#[cfg(test)]
#[path = "zoom_tests.rs"]
mod tests;
