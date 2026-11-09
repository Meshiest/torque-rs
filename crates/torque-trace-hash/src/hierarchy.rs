//! The scene hierarchy's scenario (`hierarchy`, engine mode, where the hierarchy is on): a root moved every tick, three levels
//! of children on nodes and origins, a Player child that falls onto the root and rides it, and reparents with both keep
//! modes, a local pose set and a detach along the way; beside that tree, the client's own figure rides a deck hung on a
//! lift that slides, rises, falls and turns, jumping now and then, while the client predicts it over the deck. Each tick the
//! hash takes in the server's world and scale of every object of the tree, every child's local pose, the Player child's
//! velocity, and the figure as the server and the client's prediction have it.
//!
//! [`ride`] runs the ride alone over a link with latency, with the deck hung on the lift or moved by itself along the same
//! path, and counts the client's corrections (torque-plugin-hierarchy's `tests/ride.rs` compares the two).

use torque_collide::math::Matrix;
use torque_game::scene::access::{matrix_of, scale_of};
use torque_plugin_hierarchy::ServerHierarchy;
use torque_ts_vm::sim::ObjId;

use crate::hash::TraceHasher;
use crate::rig::{Control, Mode, Rig};
use crate::scenarios::{Baseline, Outcome, TICKS, observe_client, observe_server};

/// The most ticks the scenario waits for the connection and the prediction.
const SETTLE_LIMIT: u32 = 400;

/// The tree: a root cube, a cube on its origin, a half-size cube on that one's `cube` node, a cube on that one, and a Player
/// over the root.
const SCENE: &str = "\
$H::Root = new StaticShape() { dataBlock = TraceFloorData; position = \"0 12 6\"; };\n\
$H::A = new StaticShape() { dataBlock = TraceFloorData; position = \"2 12 6\"; };\n\
$H::B = new StaticShape() { dataBlock = TraceFloorData; position = \"2 14 6\"; scale = \"0.5 0.5 0.5\"; };\n\
$H::C = new StaticShape() { dataBlock = TraceFloorData; position = \"0 14 7.5\"; };\n\
$H::P = new Player() { dataBlock = TraceFigureData; position = \"0 12 7\"; };\n\
$H::Ok = $H::A.setParent($H::Root) && $H::B.setParent($H::A, 1, \"cube\") && $H::C.setParent($H::B) \
&& $H::P.setParent($H::Root);";

/// The ride: a small lift under the floor and an eight-unit deck over it, the deck's top at z = 1 (the lift's cube never
/// meets the figure).
const RIDE_SCENE: &str = "\
$H::Lift = new StaticShape() { dataBlock = TraceFloorData; position = \"-6 -8 -20\"; };\n\
$H::Deck = new StaticShape() { dataBlock = TraceFloorData; position = \"-4 -8 0.75\"; scale = \"8 8 0.5\"; };";

/// Hangs the deck on the lift's `cube` node where it is (a node at the cube's origin: the client reads it from the shape).
const HANG_DECK: &str = "return $H::Deck.setParent($H::Lift, 1, \"cube\");";

/// Puts the figure on the deck, a little over its top.
const BOARD: &str = "TraceFigure.setTransform(\"-4 -8 1.05 0 0 1 0\");";

/// The ticks the figure takes to land on the deck before the ride's corrections are counted.
const BOARDING: u32 = 30;

/// Whole steps up to `period` and back down over `2 * period` ticks: a triangle wave.
fn triangle(tick: u32, period: u32) -> u32 {
    let phase = tick % (2 * period);
    if phase < period {
        phase
    } else {
        2 * period - phase
    }
}

/// The lift's pose at `tick` as `setTransform` text: it slides along x, rises and falls and turns back and forth, by whole
/// steps of a fixed size, so the text is the same on every target.
fn lift_pose(tick: u32) -> String {
    let x = -6.0 + triangle(tick, 100) as f32 * 0.01;
    let z = -20.0 + triangle(tick, 80) as f32 * 0.004;
    let yaw = triangle(tick, 120) as f32 * 0.001;
    format!("{x} -8 {z} 0 0 1 {yaw}")
}

/// Moves `obj` and marks its record's position, as a class whose own `setTransform` sends its pose does (TGE's StaticShape
/// marks nothing, so a client would never see it move).
fn move_shape(rig: &mut Rig, obj: ObjId, pose: &str) -> Result<(), String> {
    eval(rig, &format!("{obj}.setTransform(\"{pose}\");"))?;
    mark_position(rig, obj);
    Ok(())
}

fn mark_position(rig: &Rig, obj: ObjId) {
    torque_game::scene::ghost::mark_record(
        rig.server.vm(),
        obj,
        torque_ghosts::shapes::STATIC_POSITION_MASK,
    );
}

/// The figure's keys at `tick` of the ride: a slow turn, and a jump held for two ticks of every 64.
fn ride_keys(rig: &mut Rig, tick: u32) {
    let held = u32::from(tick % 64 < 2);
    rig.set_global("$mvYaw", 0.002);
    rig.set_global("$mvTriggerCount2", (2 * (tick / 64) + held) as f32);
}

/// The client's corrections so far.
fn corrections(rig: &Rig) -> u64 {
    rig.client
        .borrow()
        .conn
        .conn
        .as_ref()
        .map_or(0, |c| c.predict.stats.corrections)
}

/// How the ride's deck moves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Deck<'a> {
    /// Hung on the lift: the server carries it, and the client places it from the lift's pose and its local pose.
    Hung,
    /// A root moved by itself each tick to the pose it had at that tick of a hung run (`path`), its record marked.
    Root(&'a [Matrix]),
    /// A root that never moves: what the figure's own moves cost in corrections on a deck.
    Still,
}

/// What a [`ride`] came to.
#[derive(Debug, Clone, PartialEq)]
pub struct Ride {
    /// The client's corrections after the figure landed.
    pub corrections: u64,
    /// The deck's server pose at each tick of the ride (the path a [`Deck::Root`] run follows).
    pub path: Vec<Matrix>,
    /// The largest distance between where the client's collision world placed the hung deck at a tick and where the server
    /// had it `latency` ticks before, the tick of the latest record the client held (units; 0 for a root deck).
    pub placed_error: f32,
    /// Whether the figure was still over the deck at the end (on the server).
    pub aboard: bool,
}

/// The ride alone ([module docs](self)): the figure boards the deck and rides it for [`TICKS`] ticks over a link that holds
/// each datagram from the server `latency` steps.
///
/// # Errors
/// The client never predicted, a script failed, or the deck could not be hung.
pub fn ride(deck: Deck<'_>, latency: u32) -> Result<Ride, String> {
    let mut rig = Rig::in_mode(Mode::Engine, Baseline, Control::Figure)?;
    if !rig.settle(SETTLE_LIMIT) {
        return Err(format!(
            "the client never predicted its control object in {SETTLE_LIMIT} ticks"
        ));
    }
    rig.set_latency(latency);
    eval(&mut rig, RIDE_SCENE)?;
    let lift = object(&mut rig, "$H::Lift")?;
    let deck_id = object(&mut rig, "$H::Deck")?;
    if deck == Deck::Hung && eval(&mut rig, HANG_DECK)? != "1" {
        let error = eval(&mut rig, "return getParentError();")?;
        return Err(format!("the deck was refused: {error}"));
    }
    eval(&mut rig, BOARD)?;
    let mut counted = 0;
    let mut path = Vec::with_capacity(TICKS as usize);
    let mut placed_error = 0.0f32;
    for tick in 0..TICKS {
        move_shape(&mut rig, lift, &lift_pose(tick))?;
        if let Deck::Root(path) = deck {
            let at = *path.get(tick as usize).ok_or("the path is too short")?;
            torque_game::scene::object::set_transform(rig.server.vm_mut(), deck_id, at);
            mark_position(&rig, deck_id);
        }
        ride_keys(&mut rig, tick);
        rig.step();
        if tick == BOARDING {
            counted = corrections(&rig);
        }
        path.push(matrix_of(rig.server.vm(), deck_id).ok_or("the deck went")?);
        if deck == Deck::Hung
            && let Some(placed) = placed_deck(&rig)
            && let Some(sent) = tick.checked_sub(latency).and_then(|t| path.get(t as usize))
        {
            let d = [3, 7, 11]
                .iter()
                .map(|&k| (placed[k] - sent[k]).powi(2))
                .sum::<f32>()
                .sqrt();
            placed_error = placed_error.max(d);
        }
    }
    let vm = rig.server.vm();
    let figure = vm.objects().find(b"TraceFigure").ok_or("the figure")?;
    let at = torque_game::sim::player::position(vm, figure).ok_or("the figure's position")?;
    let last = path.last().ok_or("no tick ran")?;
    let aboard = (at[0] - last[3]).abs() < 4.0 && (at[1] - last[7]).abs() < 4.0 && at[2] > last[11];
    Ok(Ride {
        corrections: corrections(&rig) - counted,
        path,
        placed_error,
        aboard,
    })
}

/// Where a prediction tick's collision world of the client places the one ghost with a link (the deck): a container made as
/// the client's prediction makes one, its shapes read from the trace world's files.
fn placed_deck(rig: &Rig) -> Option<Matrix> {
    use torque_client::game::ClientGame;
    use torque_client::predict::{ClientCollision, Ctx, StaticWorld, TICK_WORK, drive::ShapeCache};
    let st = rig.client.borrow();
    let world = &st.conn.conn.as_ref()?.world;
    let index = world
        .records
        .ghosts
        .keys()
        .copied()
        .find(|&i| world.records.links.parent(i).is_some())?;
    let mut static_world = StaticWorld::default();
    static_world.reader = crate::rig::TraceClient(crate::rig::files()).file_reader();
    let mut shapes = ShapeCache::default();
    let collision = ClientCollision::new(
        world,
        u32::MAX,
        TICK_WORK,
        Some(Ctx {
            near: [0.0; 3],
            static_world: &mut static_world,
            shapes: &mut shapes,
            driven: None,
            ghosts: None,
            disabled: None,
            player_at: None,
        }),
    );
    collision
        .get(torque_collide::ObjectId(index + 1))
        .map(|o| o.view.obj_to_world)
}

/// What each tick does to the tree, beside moving the root.
fn change(tick: u32) -> Option<&'static str> {
    match tick {
        200 => Some("$H::C.setParent($H::A, 0);"),
        300 => Some("$H::C.setParent($H::Root, 1);"),
        400 => Some("$H::B.setLocalTransform(\"0 2 0 0 0 1 0.5\");"),
        500 => Some("$H::P.setParent(0);"),
        _ => None,
    }
}

fn eval(rig: &mut Rig, script: &str) -> Result<String, String> {
    let out = rig
        .server
        .vm_mut()
        .eval(script.as_bytes())
        .map_err(|e| format!("{script}: {e:?}"))?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

fn object(rig: &mut Rig, global: &str) -> Result<ObjId, String> {
    let text = eval(rig, &format!("return {global};"))?;
    text.parse()
        .map_err(|_| format!("{global} is {text:?}, not an object"))
}

/// The scenario's run (module docs).
pub fn hierarchy() -> Result<Outcome, String> {
    let mut rig = Rig::in_mode(Mode::Engine, Baseline, Control::Figure)?;
    if !rig.settle(SETTLE_LIMIT) {
        return Err(format!(
            "the client never predicted its control object in {SETTLE_LIMIT} ticks"
        ));
    }
    eval(&mut rig, SCENE)?;
    if eval(&mut rig, "return $H::Ok;")? != "1" {
        let error = eval(&mut rig, "return getParentError();")?;
        return Err(format!("the tree was refused: {error}"));
    }
    eval(&mut rig, RIDE_SCENE)?;
    if eval(&mut rig, HANG_DECK)? != "1" {
        let error = eval(&mut rig, "return getParentError();")?;
        return Err(format!("the deck was refused: {error}"));
    }
    eval(&mut rig, BOARD)?;
    let lift = object(&mut rig, "$H::Lift")?;
    let names = ["$H::Root", "$H::A", "$H::B", "$H::C", "$H::P"];
    let mut ids = Vec::with_capacity(names.len());
    for name in names {
        ids.push(object(&mut rig, name)?);
    }
    let hierarchy = ServerHierarchy::of(rig.server.vm()).ok_or("the server has no hierarchy")?;
    let start = matrix_of(rig.server.vm(), ids[4]).ok_or("the Player")?;
    let mut h = TraceHasher::new();
    let mut distance = 0.0f32;
    let mut last = [start[3], start[7], start[11]];
    for tick in 0..TICKS {
        // The root slides, rises and turns: whole steps of a fixed size, so the text is the same on every target.
        let t = tick as f32;
        let root = format!(
            "{} 12 {} 0 0 1 {}",
            t * 0.04,
            6.0 + (tick % 120) as f32 * 0.005,
            t * 0.01
        );
        eval(&mut rig, &format!("$H::Root.setTransform(\"{root}\");"))?;
        if let Some(script) = change(tick) {
            eval(&mut rig, script)?;
        }
        move_shape(&mut rig, lift, &lift_pose(tick))?;
        ride_keys(&mut rig, tick);
        rig.step();
        observe_server(&rig, &mut h, Control::Figure);
        observe_client(&rig, &mut h);
        let vm = rig.server.vm();
        for &id in &ids {
            let m = matrix_of(vm, id).ok_or("an object of the tree went")?;
            h.floats(&m[..12]);
            h.floats(&scale_of(vm, id));
            match hierarchy.local(id) {
                Some(local) => {
                    h.floats(&local.translation);
                    h.floats(&local.rotation);
                }
                None => h.bool(false),
            }
        }
        let player = torque_game::sim::player::state(vm, ids[4]).ok_or("the Player's state")?;
        h.floats(&player.velocity);
        let at = [
            player.obj_to_world[3],
            player.obj_to_world[7],
            player.obj_to_world[11],
        ];
        distance += (0..3)
            .map(|k| (at[k] - last[k]).powi(2))
            .sum::<f32>()
            .sqrt();
        last = at;
    }
    hierarchy.audit().map_err(|v| format!("{v:?}"))?;
    Ok(Outcome {
        hash: h.finish(),
        values: h.count(),
        distance,
        end: last,
        corrections: corrections(&rig),
        driven_ticks: 0,
    })
}
