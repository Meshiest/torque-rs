//! The scene hierarchy's scenario (`hierarchy`, engine mode, where the hierarchy is on): a root moved every tick, three levels
//! of children on nodes and origins, a Player child that falls onto the root and rides it, and reparents with both keep
//! modes, a local pose set and a detach along the way. Each tick the hash takes in the server's world and scale of every
//! object of the tree, every child's local pose, and the Player's velocity.

use torque_game::scene::access::{matrix_of, scale_of};
use torque_plugin_hierarchy::ServerHierarchy;
use torque_ts_vm::sim::ObjId;

use crate::hash::TraceHasher;
use crate::rig::{Control, Mode, Rig};
use crate::scenarios::{Baseline, Outcome, TICKS};

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
        rig.step();
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
        corrections: 0,
        driven_ticks: 0,
    })
}
