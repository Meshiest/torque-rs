//! The client's half: ropes the server ghosts become ropes of the client's own world, which it keeps simulating and drawing (the
//! points never cross the network); predicted ropes made at once and adopted by the server's ghost; the one-shot ropes a remote
//! command makes.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use glam::Vec3;
use torque_engine::net_classes::{ClientGhostLookup, GhostEvent};
use torque_engine::predict_ids::{self, PredictIds};
use torque_engine::{Res, ScriptClass, VmMut, function};
use torque_ts_text::atof;
use torque_ts_vm::vm::Vm;

use super::records::{AnchorWire, DriverWire, RopeGhost};
use super::server::MAX_FX_SECONDS;
use crate::data::{RopeData, RopeLook, RopeParams};
use crate::driver::RopeDrivers;
use crate::natives::{RopeShared, texture_loaded};
use crate::sim::{Curve, MAX_COORD, RopeEnd, RopeError, RopeId, RopeWorld, Tangent, pose_at};

/// The most predicted ropes a client waits on at once (and so the most predict ids in use).
pub const MAX_PREDICTED: usize = predict_ids::MAX_WAITING;

/// How long a predicted rope waits for the server's ghost (seconds) before it fades out.
pub const PREDICT_TIMEOUT: f32 = predict_ids::PREDICT_TIMEOUT;

/// The seconds a predicted rope that was never confirmed fades out over.
pub const PREDICT_FADE: f32 = predict_ids::PREDICT_FADE;

/// The most one-shot ropes a client shows at once.
pub const MAX_FX: usize = 32;

/// The seconds at the end of a one-shot rope it fades out over.
pub const FX_FADE: f32 = 0.25;

/// A rope the server ghosts, as this client has it.
struct Held {
    rope: RopeId,
    /// What the client has applied of the ghost's record: an update changes only the groups that differ.
    applied: RopeGhost,
}

/// The networked ropes of one client: the ghosts it follows, the predictions it waits on and the one-shot ropes it shows.
#[derive(Default)]
pub struct NetRopes {
    by_ghost: BTreeMap<u32, Held>,
    /// The ropes the client made itself and told the server about.
    predicted: PredictIds<RopeId>,
    fx: Vec<RopeId>,
}

/// The engine resource that holds a client's [`NetRopes`].
#[derive(Clone, Default)]
pub struct RopeNet(pub Rc<RefCell<NetRopes>>);

impl RopeNet {
    /// A new, empty book.
    pub fn new() -> RopeNet {
        RopeNet::default()
    }

    /// The book, for the length of the guard.
    pub fn lock(&self) -> std::cell::RefMut<'_, NetRopes> {
        self.0.borrow_mut()
    }
}

impl NetRopes {
    /// How many ropes the server ghosts that this client holds.
    pub fn ghosted(&self) -> usize {
        self.by_ghost.len()
    }

    /// The client's rope for the server's ghost `index`.
    pub fn rope_of(&self, index: u32) -> Option<RopeId> {
        self.by_ghost.get(&index).map(|h| h.rope)
    }

    /// How many predicted ropes wait for the server's ghost.
    pub fn waiting(&self) -> usize {
        self.predicted.len()
    }

    /// How many one-shot ropes the client shows.
    pub fn fx_count(&self) -> usize {
        self.fx.len()
    }

    /// Tags the client's own rope `rope` as a prediction and gives it a predict id (1 to 255, one not in use): the server's ghost that
    /// carries the id is the rope. None when [`MAX_PREDICTED`] are already waiting, the rope is not there or is tagged already.
    pub fn predict(&mut self, world: &RopeWorld, rope: RopeId) -> Option<u8> {
        if world.rope(rope).is_none() || self.predicted.any(|&r| r == rope) {
            return None;
        }
        self.predicted.issue(rope)
    }
}

/// The point an anchor stands for when a rope is made (a ghost the client has not got is the origin).
fn anchor_point(anchor: &AnchorWire, lookup: Option<&ClientGhostLookup>) -> [f32; 3] {
    match anchor {
        AnchorWire::Point(at) => *at,
        AnchorWire::Ghost { ghost, offset, .. } => lookup
            .and_then(|l| placed(l, *ghost))
            .map_or([0.0; 3], |pose| {
                crate::sim::held_at(&pose, Vec3::from(*offset)).to_array()
            }),
        _ => [0.0; 3],
    }
}

/// Where the client has ghost `ghost`, as a transform: its resolved pose, else its position with no turn.
fn placed(lookup: &ClientGhostLookup, ghost: u32) -> Option<[f32; 16]> {
    lookup
        .0
        .pose(ghost)
        .or_else(|| lookup.0.position(ghost).map(pose_at))
}

fn pin(world: &mut RopeWorld, rope: RopeId, end: RopeEnd, anchor: &AnchorWire) {
    let _ = match anchor {
        AnchorWire::Free | AnchorWire::Waiting => world.release(rope, end),
        AnchorWire::Point(at) => world.pin_point(rope, end, *at),
        AnchorWire::Ghost {
            ghost,
            node,
            offset,
        } => world.pin_ghost_at(rope, end, *ghost, node, *offset),
    };
}

/// Puts what the record says on the rope, each group that differs from what was applied.
pub(crate) fn apply(world: &mut RopeWorld, rope: RopeId, old: &RopeGhost, new: &RopeGhost) {
    for (end, was, now) in [
        (RopeEnd::Start, &old.start, &new.start),
        (RopeEnd::End, &old.end, &new.end),
    ] {
        if let Some(anchor) = now
            && was.as_ref() != Some(anchor)
        {
            pin(world, rope, end, anchor);
        }
    }
    if let Some(length) = new.length
        && old.length != Some(length)
        && length.target > 0.0
    {
        let _ = world.set_length(rope, length.target, length.seconds);
    }
    if let Some(look) = &new.look
        && old.look.as_ref() != Some(look)
    {
        let _ = world.set_look(rope, RopeLook::default(), [true; 4]);
        let _ = world.set_look(rope, look.0.clone(), [false; 4]);
    }
    if let Some(curve) = new.curve
        && old.curve != Some(curve)
    {
        let _ = world.set_curve(rope, curve.0);
    }
    if let Some(life) = new.life
        && old.life != Some(life)
    {
        let _ = world.set_lifetime(rope, life.left, life.fade);
    }
}

/// Binds the rope to the driver the record names, or lets it go of one (each time the group differs from what was applied).
fn bind_driver(
    drivers: &RopeDrivers,
    world: &mut RopeWorld,
    rope: RopeId,
    old: &RopeGhost,
    new: &RopeGhost,
) {
    if let Some(driver) = &new.driver
        && old.driver.as_ref() != Some(driver)
    {
        match driver {
            DriverWire::On { name, ghost } => drivers.bind(rope, name, *ghost),
            DriverWire::Off | DriverWire::Waiting => {
                drivers.unbind(world, rope);
            }
        }
    }
}

/// Loads the picture of the texture a ghost's look names, so the rope can draw with it (the loader's caps and trust rules apply; a
/// file that does not load warns once and the rope is drawn plain).
fn load_look(vm: &mut Vm, ropes: &RopeShared, record: &RopeGhost, nearest: bool) {
    if let Some(path) = record
        .look
        .as_ref()
        .and_then(|look| look.0.texture.as_deref())
    {
        texture_loaded(vm, ropes, path, nearest);
    }
}

fn params_of(vm: &mut Vm, ropes: &RopeShared, db: u32) -> Option<RopeParams> {
    let data = RopeData::read(vm, db).ok()?;
    let mut params = RopeParams::new(&data);
    params.texture = texture_loaded(vm, ropes, &data.texture.0, params.texture_nearest);
    Some(params)
}

/// A ghost of the server's appeared: it becomes the client's rope, or is the rope the client predicted.
fn added(
    vm: &mut Vm,
    ropes: &RopeShared,
    net: &RopeNet,
    drivers: &RopeDrivers,
    index: u32,
    record: &RopeGhost,
) {
    let Some(db) = record.data_block.0 else {
        return;
    };
    let Some(params) = params_of(vm, ropes, db) else {
        vm.warn(format!(
            "a networked rope names the datablock {db}, which is not a RopeData the client has"
        ));
        return;
    };
    load_look(vm, ropes, record, params.texture_nearest);
    let lookup = ClientGhostLookup::of(vm);
    let mut world = ropes.lock();
    let mut book = net.lock();
    // A ghost index that a ghost of a connection before this one held: its rope goes with it.
    if let Some(stale) = book.by_ghost.remove(&index) {
        world.remove(stale.rope);
        drivers.forget(stale.rope);
    }
    let predicted = record
        .predict
        .map(|p| p.0)
        .and_then(|id| book.predicted.adopt(id))
        .filter(|&rope| world.rope(rope).is_some());
    let (rope, old) = match predicted {
        // The client's own rope is the server's: nothing more is made, and the ghost's groups go on it.
        Some(rope) => (rope, RopeGhost::default()),
        None => {
            let point = |anchor: &Option<AnchorWire>| {
                anchor
                    .as_ref()
                    .map_or([0.0; 3], |a| anchor_point(a, lookup.as_deref()))
            };
            let made = world.create(params, point(&record.start), point(&record.end));
            match made {
                Ok(rope) => (rope, RopeGhost::default()),
                Err(RopeError::TooManyRopes | RopeError::TooManySegments) => {
                    vm.warn(
                        "networked rope not shown: the client holds as many ropes as it can"
                            .to_string(),
                    );
                    return;
                }
                Err(_) => return,
            }
        }
    };
    apply(&mut world, rope, &old, record);
    bind_driver(drivers, &mut world, rope, &old, record);
    book.by_ghost.insert(
        index,
        Held {
            rope,
            applied: record.clone(),
        },
    );
}

/// An update reached a ghost: the groups that changed go on the rope.
fn changed(
    vm: &mut Vm,
    ropes: &RopeShared,
    net: &RopeNet,
    drivers: &RopeDrivers,
    index: u32,
    record: &RopeGhost,
) {
    let nearest = net.lock().by_ghost.get(&index).and_then(|held| {
        ropes
            .lock()
            .rope(held.rope)
            .map(|r| r.params().texture_nearest)
    });
    if let Some(nearest) = nearest {
        load_look(vm, ropes, record, nearest);
    }
    let mut world = ropes.lock();
    let mut book = net.lock();
    let Some(held) = book.by_ghost.get_mut(&index) else {
        return;
    };
    if world.rope(held.rope).is_some() {
        apply(&mut world, held.rope, &held.applied, record);
        bind_driver(drivers, &mut world, held.rope, &held.applied, record);
    }
    held.applied = record.clone();
}

/// The server deleted a ghost: its rope goes.
fn removed(ropes: &RopeShared, net: &RopeNet, drivers: &RopeDrivers, index: u32) {
    let mut book = net.lock();
    if let Some(held) = book.by_ghost.remove(&index) {
        ropes.lock().remove(held.rope);
        drivers.forget(held.rope);
    }
}

// ANCHOR: heard
/// What the plugin's ghost handler does with an event.
pub(crate) fn heard(
    vm: &mut Vm,
    ropes: &RopeShared,
    net: &RopeNet,
    drivers: &RopeDrivers,
    event: GhostEvent<'_, RopeGhost>,
) {
    match event {
        GhostEvent::Added { index, record } => added(vm, ropes, net, drivers, index, record),
        GhostEvent::Changed { index, record } => changed(vm, ropes, net, drivers, index, record),
        GhostEvent::Removed { index } => removed(ropes, net, drivers, index),
    }
}
// ANCHOR_END: heard

/// The client's tick: ends hung from ghosts follow them when no frame is drawn, a prediction nobody confirmed fades, and the ropes
/// that are gone are forgotten.
pub(crate) fn tick(
    world: &mut RopeWorld,
    net: &mut NetRopes,
    lookup: Option<&ClientGhostLookup>,
    seconds: f32,
) {
    if let Some(lookup) = lookup {
        world.follow_ghost_poses(|ghost, _node| placed(lookup, ghost));
    }
    for rope in net.predicted.expire(seconds, |&r| world.rope(r).is_some()) {
        let _ = world.set_lifetime(rope, PREDICT_FADE, PREDICT_FADE);
    }
    net.fx.retain(|&id| world.rope(id).is_some());
}

/// tags a rope this client made as a prediction of one the server is about to make, and returns the id the server's ghost must carry
///
/// A client-side action (firing a beam) makes its rope at once with `ropeCreate` and kin, calls this, and tells the server the id
/// (a command argument). The server's `ropeNetCreate` is given the id and the client's connection; the ghost that comes back
/// carries the id for that client alone, and the client then keeps the rope it made instead of making another. A rope the server
/// never confirms within two seconds fades out and goes. At most 16 wait at once: past that, or for a rope that is not there, the
/// result is 0.
#[function(name = "ropePredict")]
fn rope_predict(ropes: Res<RopeShared>, net: Res<RopeNet>, rope: i32) -> i32 {
    let world = ropes.lock();
    net.lock()
        .predict(&world, RopeId::try_from(rope).unwrap_or(0))
        .map_or(0, i32::from)
}

/// how many ropes the server ghosts that this client shows
#[function(name = "ropeNetCount")]
fn rope_net_count(net: Res<RopeNet>) -> i32 {
    i32::try_from(net.lock().ghosted()).unwrap_or(i32::MAX)
}

/// parses a point a server sent: three finite numbers; anything else is refused
fn fx_point(text: &str) -> Option<[f32; 3]> {
    let mut words = text.split_ascii_whitespace();
    let mut next = || {
        words
            .next()
            .map(|w| atof(w.as_bytes()) as f32)
            .filter(|v| v.is_finite() && v.abs() <= MAX_COORD)
    };
    let at = [next()?, next()?, next()?];
    words.next().is_none().then_some(at)
}

/// the server's `ropeNetFx`: shows a rope of the datablock between two points for a few seconds (the remote command `RopeFx`)
///
/// Everything is checked: a point that is not three finite numbers inside the world, a datablock that is not a `RopeData` the
/// client has, seconds that are not a positive number (held to 30) are refused with no error; a bend that is not a number is
/// straight, and at most 32 such ropes live at once. Returns true when it showed one.
// A console function takes the arguments the server sends and what it uses of the engine, each as a parameter.
#[allow(clippy::too_many_arguments)]
#[function(name = "clientCmdRopeFx")]
fn client_cmd_rope_fx(
    mut vm: VmMut,
    ropes: Res<RopeShared>,
    net: Res<RopeNet>,
    start: &str,
    end: &str,
    datablock: &str,
    seconds: f32,
    bend: f32,
) -> bool {
    let (Some(start), Some(end)) = (fx_point(start), fx_point(end)) else {
        return false;
    };
    if !(seconds.is_finite() && seconds > 0.0) {
        return false;
    }
    let seconds = seconds.min(MAX_FX_SECONDS);
    let Some(db) = vm.objects().find(datablock.as_bytes()) else {
        return false;
    };
    let Some(params) = params_of(&mut vm, &ropes, db) else {
        return false;
    };
    let mut book = net.lock();
    let mut world = ropes.lock();
    book.fx.retain(|&id| world.rope(id).is_some());
    if book.fx.len() >= MAX_FX {
        return false;
    }
    let Ok(rope) = world.create(params, start, end) else {
        return false;
    };
    let _ = world.set_lifetime(rope, seconds, FX_FADE.min(seconds));
    if bend.is_finite() && bend != 0.0 {
        let curve = Curve {
            start: Tangent::Auto,
            end: Tangent::Auto,
            bend: bend.clamp(-4.0, 4.0),
        };
        let _ = world.set_curve(rope, Some(curve));
    }
    book.fx.push(rope);
    true
}

/// The client's natives, as the plugin registers them.
pub(crate) fn declared() -> Vec<torque_engine::script::NativeDecl> {
    torque_engine::natives![rope_predict, rope_net_count, client_cmd_rope_fx]
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
