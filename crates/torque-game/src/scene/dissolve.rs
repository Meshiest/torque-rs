//! A ShapeBase dissolving away or materializing: the engine's own state beside TGE's fade ([`Dissolve`], in the object's ext),
//! set by the script methods [`register`] adds and ghosted with the cloak block (the engine protocol's dissolve block,
//! `torque_ghosts::shapebase::DissolveField`). The server keeps when it started and how it looks; each client runs the clock,
//! the clip and the particles itself.
//!
//! - `%obj.dissolve(%ms, %out[, %edgeColor, %edgeWidth, %emitter, %tint])` starts one: `%out` 1 dissolves away, 0
//!   materializes; `%edgeColor` "r g b glow" (0..1 each, the glow the color's multiple in light, 0..31.875; by default
//!   "1 0.45 0.1 6"); `%edgeWidth` the glowing band's share of the surface (0..1, 0.04); `%emitter` a `ParticleEmitterData`
//!   the front emits (none: no particles); `%tint` 1: the particles take the surface's color.
//! - `%obj.setDissolveLook(%cellSize, %sweep, %density)`: the noise's cell (units, 0.25), a sweep from the top down (1) or
//!   the bottom up (-1) mixed with the noise (0: noise alone), the particles a square unit makes (64). Kept for later dissolves.
//! - `%obj.clearDissolve()`: whole again. `%obj.getDissolve()`: the share eaten now (0 with none).
//!
//! Every value is held to what the wire carries as it is set ([`Dissolve::held`]). Each call marks the cloak block's bit
//! ([`super::shapebase::FADE_MASK`], as `startFade` does).

use ts_vm::sim::ObjId;
use ts_vm::vm::{Native, Vm};

use super::ext;
use crate::methods::register_method;
use crate::scene::ghost as ghosts;

/// The longest dissolve (ms): the wire's 20 bits, as the client holds it too.
pub const MAX_MS: u32 =
    torque_ghosts::shapebase::DissolveField::max_of(torque_ghosts::shapebase::dissolve_bits::MS);
/// The class a dissolve's emitter must be (any other object names none).
pub const EMITTER_CLASS: &str = "ParticleEmitterData";
/// The largest glow (the wire's 8 bits of eighths).
pub const MAX_GLOW: f32 = 255.0 / 8.0;
/// The largest noise cell (units; the wire's 10 bits of 64ths) and particle density (a square unit; its 16 bits).
pub const MAX_CELL: f32 = 1023.0 / 64.0;
pub const MAX_DENSITY: f32 = torque_ghosts::shapebase::DissolveField::max_of(
    torque_ghosts::shapebase::dissolve_bits::DENSITY,
) as f32;

/// An object's dissolve (module docs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dissolve {
    /// One runs (false: cleared, or only a look set).
    pub running: bool,
    /// Counts the dissolves started (a client starts its clock again on a new one).
    pub serial: u32,
    pub out: bool,
    pub duration_ms: u32,
    /// The Sim time it started (ms).
    pub start_ms: u32,
    pub edge_color: [f32; 3],
    pub glow: f32,
    pub edge: f32,
    pub emitter: Option<ObjId>,
    pub tint: bool,
    pub cell: f32,
    pub sweep: f32,
    pub density: f32,
}

impl Default for Dissolve {
    fn default() -> Dissolve {
        Dissolve {
            running: false,
            serial: 0,
            out: true,
            duration_ms: 2000,
            start_ms: 0,
            edge_color: [1.0, 0.45, 0.1],
            glow: 6.0,
            edge: 0.04,
            emitter: None,
            tint: false,
            cell: 0.25,
            sweep: 0.0,
            density: 64.0,
        }
    }
}

impl Dissolve {
    /// The dissolve with every value in its wire range (a value that is not finite is the default's).
    #[must_use]
    pub fn held(self) -> Dissolve {
        let d = Dissolve::default();
        let finite = |v: f32, or: f32| if v.is_finite() { v } else { or };
        Dissolve {
            duration_ms: self.duration_ms.min(MAX_MS),
            edge_color: [0, 1, 2].map(|k| finite(self.edge_color[k], 0.0).clamp(0.0, 1.0)),
            glow: finite(self.glow, d.glow).clamp(0.0, MAX_GLOW),
            edge: finite(self.edge, d.edge).clamp(0.0, 1.0),
            cell: finite(self.cell, d.cell).clamp(0.0, MAX_CELL),
            sweep: finite(self.sweep, 0.0).clamp(-1.0, 1.0),
            density: finite(self.density, d.density).clamp(0.0, MAX_DENSITY),
            ..self
        }
    }

    /// The share eaten at Sim time `now_ms`.
    #[must_use]
    pub fn eaten_at(&self, now_ms: u32) -> f32 {
        if !self.running {
            return 0.0;
        }
        let p = if self.duration_ms == 0 {
            1.0
        } else {
            (now_ms.saturating_sub(self.start_ms) as f32 / self.duration_ms as f32).clamp(0.0, 1.0)
        };
        if self.out { p } else { 1.0 - p }
    }
}

/// Whether `id` is a `ParticleEmitterData` (a dissolve's emitter names nothing else: another object's id on the wire would
/// alias a datablock's).
pub fn is_emitter(vm: &Vm, id: ObjId) -> bool {
    super::camera::is_class(vm, id, EMITTER_CLASS)
}

/// The emitter `d` names while it is still a `ParticleEmitterData` (it may have been deleted since).
pub fn emitter_of(vm: &Vm, d: &Dissolve) -> Option<ObjId> {
    d.emitter.filter(|&e| is_emitter(vm, e))
}

/// Object `id`'s dissolve, if one was ever set.
pub fn dissolve_of(vm: &Vm, id: ObjId) -> Option<Dissolve> {
    ext::get(vm.objects(), id)?.dissolve.as_deref().copied()
}

/// `id`'s dissolve changed by `f` (made with the defaults first), held to its ranges, and the cloak block marked.
fn change(vm: &mut Vm, id: ObjId, f: impl FnOnce(&mut Dissolve)) {
    let Some(e) = ext::get_mut(vm.objects_mut(), id) else {
        return;
    };
    let d = e.dissolve.get_or_insert_with(Box::default);
    f(d);
    **d = d.held();
    ghosts::mark(vm, id, super::shapebase::FADE_MASK);
}

/// What `dissolve()` was called with: the time, the way, and the optional arguments (`None` where the call gave none: the
/// last dissolve's, else the defaults).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Start {
    pub duration_ms: u32,
    pub out: bool,
    /// "r g b glow".
    pub color: Option<[f32; 4]>,
    pub edge: Option<f32>,
    pub emitter: Option<ObjId>,
    pub tint: bool,
}

/// Starts a dissolve on `id` now (module docs).
pub fn start(vm: &mut Vm, id: ObjId, s: Start) {
    let now = vm.sim_time();
    change(vm, id, |d| {
        d.running = true;
        d.serial = d.serial.wrapping_add(1);
        d.out = s.out;
        d.duration_ms = s.duration_ms;
        d.start_ms = now;
        if let Some(c) = s.color {
            d.edge_color = [c[0], c[1], c[2]];
            d.glow = c[3];
        }
        if let Some(e) = s.edge {
            d.edge = e;
        }
        d.emitter = s.emitter;
        d.tint = s.tint;
    });
}

/// Ends `id`'s dissolve: the object is whole again.
pub fn clear(vm: &mut Vm, id: ObjId) {
    if dissolve_of(vm, id).is_some_and(|d| d.running) {
        change(vm, id, |d| d.running = false);
    }
}

/// The script methods (module docs), on ShapeBase.
pub fn register(vm: &mut Vm) {
    register_method(vm, "ShapeBase", "dissolve", |vm, c| {
        let Some(id) = c.this_object else {
            return Native::Void;
        };
        let given = |i: usize| c.argc() > i && !c.arg(i).is_empty();
        let d = Dissolve::default();
        let start_with = Start {
            duration_ms: c.int(2).max(0).cast_unsigned(),
            out: !given(3) || c.boolean(3),
            color: given(4).then(|| {
                let mut v = [d.edge_color[0], d.edge_color[1], d.edge_color[2], d.glow];
                ts_vm::text::scan_floats(c.arg(4), &mut v);
                v
            }),
            edge: given(5).then(|| c.float(5)),
            emitter: given(6)
                .then(|| vm.objects().find(c.arg(6)))
                .flatten()
                .filter(|&e| is_emitter(vm, e)),
            tint: c.boolean(7),
        };
        start(vm, id, start_with);
        Native::Void
    });
    register_method(vm, "ShapeBase", "setDissolveLook", |vm, c| {
        if let Some(id) = c.this_object {
            let (cell, sweep, density) = (c.float(2), c.float(3), c.float(4));
            change(vm, id, |d| {
                d.cell = cell;
                d.sweep = sweep;
                d.density = density;
            });
        }
        Native::Void
    });
    register_method(vm, "ShapeBase", "clearDissolve", |vm, c| {
        if let Some(id) = c.this_object {
            clear(vm, id);
        }
        Native::Void
    });
    register_method(vm, "ShapeBase", "getDissolve", |vm, c| {
        let now = vm.sim_time();
        let eaten = c
            .this_object
            .and_then(|id| dissolve_of(vm, id))
            .map_or(0.0, |d| d.eaten_at(now));
        Native::Float(f64::from(eaten))
    });
}

#[cfg(test)]
#[path = "dissolve_tests.rs"]
mod tests;
