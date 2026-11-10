//! A client's camera modes, the one in use and the ease between them ([`CameraModes`]).

use std::cell::RefCell;

use torque_render_scene::source::Eye;

use super::builtin::{FirstPerson, OrbitMode, Rig, ThirdPerson};
use super::{
    CameraCx, CameraInput, CameraMode, CameraModeEntry, FIRST_PERSON, MAX_MODES, MAX_NAME, ORBIT,
    THIRD_PERSON,
};
use crate::camera_behind::moved_only;
use crate::camera_blend::{CameraBlend, SETTLE_SECONDS};

struct Slot {
    name: String,
    mode: Box<dyn CameraMode>,
}

struct Inner {
    slots: Vec<Slot>,
    /// The index of the mode in use.
    active: usize,
    /// The mode the last [`CameraModes::begin`] saw in use (none: no frame yet, or a cut).
    seen: Option<usize>,
    blend: CameraBlend,
    /// The wheel and look input pushed since the last frame.
    input: CameraInput,
}

/// A client's camera modes (module [docs](super)). Shared by the client's natives and the game's frame, so the methods take
/// `&self`; none may be called from inside a mode's own methods.
pub struct CameraModes {
    inner: RefCell<Inner>,
}

/// Whether `name` is one a mode may have: lower case ASCII letters, digits, `-` and `_`, no longer than [`MAX_NAME`].
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

impl CameraModes {
    /// The built-in modes ([`FIRST_PERSON`], [`THIRD_PERSON`], [`ORBIT`]) and then `entries`, in order; a later mode of a name
    /// replaces the earlier. `warn` is told of an entry left out (a bad name, or past [`MAX_MODES`]). The first person is in
    /// use.
    pub fn from_entries(entries: &[CameraModeEntry], mut warn: impl FnMut(String)) -> CameraModes {
        let modes = CameraModes {
            inner: RefCell::new(Inner {
                slots: Vec::new(),
                active: 0,
                seen: None,
                blend: CameraBlend::default(),
                input: CameraInput::default(),
            }),
        };
        let rig = Rig::default();
        modes.register(FIRST_PERSON, FirstPerson::new(rig));
        modes.register(THIRD_PERSON, ThirdPerson::new(rig));
        modes.register(ORBIT, OrbitMode::default());
        for entry in entries {
            if !valid_name(entry.name()) {
                warn(format!(
                    "camera mode \"{}\" is left out: a name is lower case letters, digits, - and _, at most {MAX_NAME} long.",
                    entry.name()
                ));
            } else if !modes.add(entry.name(), entry.make()) {
                warn(format!(
                    "camera mode \"{}\" is left out: a client keeps at most {MAX_MODES} modes.",
                    entry.name()
                ));
            }
        }
        modes
    }

    /// Adds `mode` under `name`, replacing the mode of that name. False: the name is not valid, or there are [`MAX_MODES`]
    /// already. A mode added in place of the one in use is the one in use.
    pub fn register(&self, name: &str, mode: impl CameraMode) -> bool {
        valid_name(name) && self.add(name, Box::new(mode))
    }

    fn add(&self, name: &str, mode: Box<dyn CameraMode>) -> bool {
        let mut inner = self.inner.borrow_mut();
        if let Some(slot) = inner.slots.iter_mut().find(|s| s.name == name) {
            slot.mode = mode;
        } else if inner.slots.len() < MAX_MODES {
            inner.slots.push(Slot {
                name: name.to_string(),
                mode,
            });
        } else {
            return false;
        }
        true
    }

    /// The names of the modes, in the order they were added.
    pub fn names(&self) -> Vec<String> {
        self.inner
            .borrow()
            .slots
            .iter()
            .map(|s| s.name.clone())
            .collect()
    }

    /// The name of the mode in use.
    pub fn active(&self) -> String {
        let inner = self.inner.borrow();
        inner.slots[inner.active].name.clone()
    }

    /// Whether the mode in use is called `name`.
    pub fn is_active(&self, name: &str) -> bool {
        let inner = self.inner.borrow();
        inner.slots[inner.active].name == name
    }

    fn find(inner: &Inner, name: &str) -> Option<usize> {
        let name = name.trim();
        inner.slots.iter().position(|s| s.name == name).or_else(|| {
            inner
                .slots
                .iter()
                .position(|s| s.name.eq_ignore_ascii_case(name))
        })
    }

    /// Uses the mode called `name` (spelled in any case): the next [`begin`](Self::begin) starts the ease to it. False: there
    /// is no such mode and nothing changed.
    pub fn set(&self, name: &str) -> bool {
        let mut inner = self.inner.borrow_mut();
        match Self::find(&inner, name) {
            Some(i) => {
                inner.active = i;
                true
            }
            None => false,
        }
    }

    /// Uses the mode called `name` with no ease: the camera is at its eye at the next frame. The modes keep what they follow.
    pub fn set_now(&self, name: &str) -> bool {
        let found = self.set(name);
        if found {
            let mut inner = self.inner.borrow_mut();
            inner.blend.reset();
            inner.seen = None;
        }
        found
    }

    /// Adds `look` (yaw, pitch; radians) and `zoom` (the wheel's notches) to
    /// what the next [`take_input`](Self::take_input) gives.
    pub fn push_input(&self, look: [f32; 2], zoom: f32) {
        let mut inner = self.inner.borrow_mut();
        for (sum, d) in inner.input.look.iter_mut().zip(look) {
            if d.is_finite() {
                *sum += d;
            }
        }
        if zoom.is_finite() {
            inner.input.zoom += zoom;
        }
    }

    /// The input pushed since the last call, which starts over.
    pub fn take_input(&self) -> CameraInput {
        std::mem::take(&mut self.inner.borrow_mut().input)
    }

    /// The start of a frame: a switch since the last frame starts the ease (the incoming mode's `enter`, the outgoing mode's
    /// `leave`), and the ease's clock moves on `cx.dt`.
    pub fn begin(&self, cx: &CameraCx<'_>) {
        let mut inner = self.inner.borrow_mut();
        let inner = &mut *inner;
        let now = inner.active;
        let last = inner.seen.replace(now);
        if let Some(last) = last
            && last != now
        {
            inner.slots[last].mode.leave();
            inner.slots[now].mode.enter(cx);
            let seconds = inner.slots[now]
                .mode
                .blend_in()
                .or_else(|| inner.slots[last].mode.blend_out())
                .unwrap_or(SETTLE_SECONDS);
            inner.blend.set_settle(seconds);
            inner.blend.switch();
        }
        inner.blend.advance(cx.dt);
    }

    /// Every mode moves on the frame (the one in use is told so).
    pub fn update(&self, cx: &CameraCx<'_>) {
        let mut inner = self.inner.borrow_mut();
        let active = inner.active;
        for (i, slot) in inner.slots.iter_mut().enumerate() {
            slot.mode.update(cx, i == active);
        }
    }

    /// A new figure or a teleport: every mode forgets what it followed and the camera is at the new place at once.
    pub fn cut(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.blend.reset();
        inner.seen = None;
        for slot in &mut inner.slots {
            slot.mode.reset();
        }
    }

    /// The eye of the mode in use, with no ease (`fallback` when it has none).
    pub fn target(&self, cx: &CameraCx<'_>, fallback: Eye) -> Eye {
        let inner = self.inner.borrow();
        inner.slots[inner.active].mode.eye(cx).unwrap_or(fallback)
    }

    /// The eye to draw this frame: [`target`](Self::target), eased from the last mode's after a switch, kept out of walls and
    /// ground as the mode in use keeps its own camera.
    pub fn drawn(&self, cx: &CameraCx<'_>, fallback: Eye) -> Eye {
        let mut inner = self.inner.borrow_mut();
        let inner = &mut *inner;
        let slot = &inner.slots[inner.active];
        let own = slot.mode.eye(cx);
        let target = own.unwrap_or(fallback);
        let eye = inner.blend.eye(target, |p| slot.mode.keep(cx, p));
        if own.is_some() && slot.mode.keeps_look(cx) {
            moved_only(eye, target)
        } else {
            eye
        }
    }

    /// Whether the camera is easing between modes.
    pub fn blending(&self) -> bool {
        !self.inner.borrow().blend.arrived()
    }

    /// How far the camera still is from the mode's eye (m); see [`CameraBlend::remaining`].
    pub fn remaining(&self) -> f32 {
        self.inner.borrow().blend.remaining()
    }

    /// Runs `f` on the mode called `name` as a `T` (none: there is no such mode, or it is another type). For a game that sets
    /// up a mode it added.
    pub fn with_mode<T: CameraMode, R>(
        &self,
        name: &str,
        f: impl FnOnce(&mut T) -> R,
    ) -> Option<R> {
        let mut inner = self.inner.borrow_mut();
        let i = Self::find(&inner, name)?;
        let mode: &mut dyn std::any::Any = &mut *inner.slots[i].mode;
        mode.downcast_mut::<T>().map(f)
    }
}
