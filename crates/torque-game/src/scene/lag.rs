//! Lag compensation: a shot is cast against the Players as its shooter saw them. Opt-in, off by default
//! (`$Pref::Server::LagCompensation`); with it off nothing here runs but the pref's read once a step.
//!
//! - **History.** At the end of each 32 ms step every Player on the process list gets a sample ([`record_step`]): the step,
//!   its transform, scale, object box, whether it is enabled, and for a Player with hitboxes its LOS pose
//!   ([`super::hitboxes::Pose`]). Samples are kept for the cap and two steps more; a Player that left the list is dropped.
//! - **The rewind.** The client draws a remote Player run on from its last update along its velocity from the update's arrival,
//!   not delayed: at the moment it fires it shows the state of one trip down ago, and its trigger reaches the server one trip
//!   up later. So the shooter saw the targets as they were one round trip before the server handles the shot: the rewind is the
//!   shooter's round trip ([`RewindSource`]; by default its controlling client's `getPing`), capped at
//!   `$Pref::Server::MaxRewindMs` ([`DEFAULT_MAX_MS`] when unset, never past [`LIMIT_MS`]). A shooter with no client is not
//!   rewound.
//! - **The rewound cast** ([`rewound_ray`]): the container's cast with every Player that has samples left out, then each of those
//!   (in the shot's mask, collision on, not hidden, enabled now and at the rewound time) cast at its sample, gated by its box: the
//!   position between the two steps around the rewound time, the rest from the nearer step. The nearer of the two answers is
//!   the hit (the container's on a tie). Only Players are rewound.
//! - **Which shots.** A Projectile whose source has a client takes its rewind at `onAdd` and casts each tick of its flight
//!   rewound by it; a host's own shot calls [`rewound_ray`].
//!
//! The history and the rewind are the engine's own; nothing in TGE does this.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

use torque_scene::bins::object_point;
use torque_scene::transform::{self as grid, Matrix};
use torque_scene::{NO_HIT, ray_point};
use ts_vm::fields::atob;
use ts_vm::sim::ObjId;
use ts_vm::text::atoi;
use ts_vm::vm::{Vm, VmError};

use super::access::{matrix_of, scale_of};
use super::container::{Container, ContainerHit, container_ray};
use super::ext;
use super::hitboxes::{self, Pose};
use super::rays::{object_box, ray_rules};
use super::shapebase::is_player;
use crate::shapes::Shapes;

/// The pref that turns lag compensation on (`dAtob`).
pub const PREF_ON: &str = "$Pref::Server::LagCompensation";
/// The pref that caps the rewind (ms).
pub const PREF_MAX: &str = "$Pref::Server::MaxRewindMs";
/// The cap when [`PREF_MAX`] is unset.
pub const DEFAULT_MAX_MS: u32 = 250;
/// The cap whatever [`PREF_MAX`] says: what the history holds at most.
pub const LIMIT_MS: u32 = 1000;
/// A server step (ms).
pub const STEP_MS: u32 = 32;

/// Where a shooter's round trip comes from.
pub trait RewindSource {
    /// The shooter's round trip (ms), `None` for a shooter no client controls.
    fn round_trip_ms(&self, vm: &mut Vm, shooter: ObjId) -> Option<u32>;
}

/// The shooter's controlling client's `getPing`.
#[derive(Debug, Default, Clone, Copy)]
pub struct ClientPing;

impl RewindSource for ClientPing {
    fn round_trip_ms(&self, vm: &mut Vm, shooter: ObjId) -> Option<u32> {
        let client = super::ghost::controlling_client(vm, shooter)?;
        let ping = vm.call_method(client, b"getPing", &[]).ok()?;
        u32::try_from(atoi(&ping)).ok()
    }
}

struct Source(Rc<dyn RewindSource>);

/// The host's source of round trips from now on.
pub fn set_rewind_source(vm: &mut Vm, source: Rc<dyn RewindSource>) {
    vm.set_extension(Rc::new(Source(source)));
}

/// One step of a Player's history.
#[derive(Debug, Clone)]
pub struct Sample {
    pub step: u64,
    pub matrix: Matrix,
    pub scale: [f32; 3],
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub enabled: bool,
    pub pose: Option<Pose>,
}

#[derive(Debug, Default)]
struct History {
    step: Cell<u64>,
    players: RefCell<BTreeMap<ObjId, VecDeque<Sample>>>,
    /// A rewound cast's container pass is running: the Players with samples answer nothing in it.
    rewinding: Cell<bool>,
}

/// Whether lag compensation is on.
pub fn on(vm: &Vm) -> bool {
    vm.global(PREF_ON).is_some_and(|v| atob(&v))
}

/// The rewind's cap (ms).
pub fn max_rewind_ms(vm: &Vm) -> u32 {
    match vm.global(PREF_MAX) {
        Some(v) if !v.is_empty() => u32::try_from(atoi(&v).max(0)).unwrap_or(0).min(LIMIT_MS),
        _ => DEFAULT_MAX_MS,
    }
}

/// Whether the step should call [`record_step`]: on, or a history left to clear.
pub fn wants_steps(vm: &Vm) -> bool {
    on(vm)
        || vm
            .extension::<History>()
            .is_some_and(|h| !h.players.borrow().is_empty())
}

/// The end of a step: a sample of each Player among `objects` (the process list); the history trimmed. With lag compensation
/// off the history is let go.
pub fn record_step(vm: &mut Vm, objects: &[ObjId]) {
    if !on(vm) {
        if let Some(h) = vm.extension::<History>() {
            h.players.borrow_mut().clear();
        }
        return;
    }
    let h = match vm.extension::<History>() {
        Some(h) => h,
        None => {
            let h = Rc::new(History::default());
            vm.set_extension(Rc::clone(&h));
            h
        }
    };
    let step = h.step.get().saturating_add(1);
    h.step.set(step);
    let keep = (max_rewind_ms(vm) / STEP_MS) as usize + 3;
    let mut seen = Vec::new();
    for &id in objects {
        if !is_player(vm, id) {
            continue;
        }
        let Some(sample) = sample_of(vm, id, step) else {
            continue;
        };
        seen.push(id);
        let mut players = h.players.borrow_mut();
        let q = players.entry(id).or_default();
        q.push_back(sample);
        while q.len() > keep {
            q.pop_front();
        }
    }
    seen.sort_unstable();
    h.players
        .borrow_mut()
        .retain(|id, _| seen.binary_search(id).is_ok());
}

fn sample_of(vm: &mut Vm, id: ObjId, step: u64) -> Option<Sample> {
    let pose = if hitboxes::wanted(vm, id) {
        hitboxes::pose_now(vm, id).ok().flatten()
    } else {
        None
    };
    let matrix = matrix_of(vm, id)?;
    let (min, max) = object_box(vm, id);
    Some(Sample {
        step,
        matrix,
        scale: scale_of(vm, id),
        min,
        max,
        enabled: crate::sim::vitals::get(vm, id).damage_state == 0,
        pose,
    })
}

/// How many samples the Player has.
pub fn history_len(vm: &Vm, id: ObjId) -> usize {
    vm.extension::<History>()
        .map_or(0, |h| h.players.borrow().get(&id).map_or(0, VecDeque::len))
}

/// The rewind (ms) of a shot by `shooter`: its round trip capped; 0 when lag compensation is off or no client controls it.
pub fn rewind_for(vm: &mut Vm, shooter: ObjId) -> u32 {
    if !on(vm) {
        return 0;
    }
    let source = vm.extension::<Source>().map_or_else(
        || Rc::new(ClientPing) as Rc<dyn RewindSource>,
        |s| Rc::clone(&s.0),
    );
    let trip = source.round_trip_ms(vm, shooter).unwrap_or(0);
    trip.min(max_rewind_ms(vm))
}

/// Whether a rewound cast's container pass is running and the Player has samples: it answers in the second pass.
pub fn rewound_here(vm: &Vm, id: ObjId) -> bool {
    vm.extension::<History>()
        .is_some_and(|h| h.rewinding.get() && h.players.borrow().contains_key(&id))
}

/// The sample at step `at` (fractional): the position between the two steps around it, the rest from the nearer; the oldest
/// or newest when `at` is outside.
pub fn sample_at(q: &VecDeque<Sample>, at: f64) -> Option<Sample> {
    let first = q.front()?;
    let last = q.back()?;
    if at <= first.step as f64 {
        return Some(first.clone());
    }
    if at >= last.step as f64 {
        return Some(last.clone());
    }
    let i = q.iter().rposition(|s| s.step as f64 <= at)?;
    let (s0, s1) = (q.get(i)?, q.get(i + 1)?);
    let f = ((at - s0.step as f64) / (s1.step - s0.step) as f64) as f32;
    let mut out = if f < 0.5 { s0.clone() } else { s1.clone() };
    for k in [3, 7, 11] {
        out.matrix[k] = s0.matrix[k] + (s1.matrix[k] - s0.matrix[k]) * f;
    }
    Some(out)
}

/// The cast of the world segment at a Player's sample: the fraction and the normal in the world. The box gates it, then the
/// sample's hitboxes answer when it has them.
fn cast_sample(
    vm: &Vm,
    id: ObjId,
    sample: &Sample,
    start: [f32; 3],
    end: [f32; 3],
) -> Result<Option<(f32, [f32; 3])>, VmError> {
    let w2o = grid::affine_inverse(&sample.matrix);
    let a = object_point(&w2o, sample.scale, start);
    let b = object_point(&w2o, sample.scale, end);
    let boxed = ray_rules(vm).player_cast(sample.min, sample.max, a, b);
    if boxed.is_none() {
        return Ok(None);
    }
    let local = match &sample.pose {
        Some(pose) => {
            let mut charge = |u: u64| vm.charge_work(u);
            match hitboxes::cast(&pose.kept, &pose.nodes, &pose.los, a, b, &mut charge)? {
                Some(Some(part)) => {
                    hitboxes::note_pose(vm, id, ray_point(start, end, part.t), pose, part.object);
                    Some((part.t, part.normal))
                }
                Some(None) => None,
                None => boxed,
            }
        }
        None => boxed,
    };
    Ok(local.map(|(t, n)| (t, grid::rotate(&sample.matrix, n))))
}

/// Whether a Player may answer a rewound cast with `mask` now: there, of a type in the mask, collision on, not hidden, enabled.
fn eligible(vm: &Vm, id: ObjId, mask: u32) -> bool {
    let Some(e) = ext::get(vm.objects(), id) else {
        return false;
    };
    e.type_mask & mask != 0
        && e.collision_off == 0
        && !e.hidden
        && crate::sim::vitals::get(vm, id).damage_state == 0
}

/// `Container::castRay` with the Players rewound by `rewind_ms` (module docs); the plain cast when that is 0 or there is no
/// history.
pub fn rewound_ray(
    vm: &mut Vm,
    container: &Container,
    shapes: Option<&Shapes>,
    start: [f32; 3],
    end: [f32; 3],
    mask: u32,
    rewind_ms: u32,
) -> Result<Option<ContainerHit>, VmError> {
    let history = vm
        .extension::<History>()
        .filter(|h| rewind_ms > 0 && !h.players.borrow().is_empty());
    let Some(h) = history else {
        return container_ray(vm, container, shapes, start, end, mask);
    };
    h.rewinding.set(true);
    let first = container_ray(vm, container, shapes, start, end, mask);
    h.rewinding.set(false);
    let mut found = first?;
    if !start.iter().chain(&end).all(|v| v.is_finite()) {
        return Ok(found);
    }
    let at = h.step.get() as f64 - f64::from(rewind_ms) / f64::from(STEP_MS);
    let ids: Vec<ObjId> = h.players.borrow().keys().copied().collect();
    let mut best = found.map_or(NO_HIT, |f| f.t);
    for id in ids {
        if !eligible(vm, id, mask) {
            continue;
        }
        let Some(sample) = h.players.borrow().get(&id).and_then(|q| sample_at(q, at)) else {
            continue;
        };
        if !sample.enabled {
            continue;
        }
        if let Some((t, normal)) = cast_sample(vm, id, &sample, start, end)?
            && t < best
        {
            best = t;
            found = Some(ContainerHit {
                object: id,
                t,
                point: ray_point(start, end, t),
                normal,
            });
        }
    }
    Ok(found)
}
