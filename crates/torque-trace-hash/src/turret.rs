//! The turret scenario (`turret`, engine mode, where the hierarchy is on): one client drives the buggy, a tank here, and a
//! second client controls a turret hung on its roof, a Player that aims (turns and looks up and down) while the tank drives,
//! turns and brakes under it. Each tick the hash takes in the server's tank, the turret's world, local pose and state, both
//! clients' predictions, and where the gunner's client draws its turret.
//!
//! [`run`] runs the same moves with the turret on the tank or on a base that never moves, with or without the gunner's
//! client, and counts each client's corrections (torque-plugin-hierarchy's `tests/turret.rs` compares them).

use torque_collide::math::{Matrix, mul_p};
use torque_game::scene::access::matrix_of;
use torque_plugin_hierarchy::ServerHierarchy;
use torque_ts_vm::sim::ObjId;

use crate::hash::TraceHasher;
use crate::rig::{Control, Mode, Peer, Rig};
use crate::scenarios::{Baseline, Outcome, TICKS, observe_client, observe_server, observe_state};
use torque_ghost_world::world::Pose;

/// The most ticks the scenario waits for each connection and its prediction.
const SETTLE_LIMIT: u32 = 400;

/// The latency of the scenario's line: each datagram from the server held a few steps, so a correction replays moves.
const LATENCY: u32 = 3;

/// The ticks the turret takes to settle on its roof before the corrections are counted.
const BOARDING: u32 = 30;

/// Where the turret stands on what carries it: on the middle of the tank's roof, a little over it (the body's collision box
/// tops out at 1.9).
const ON_ROOF: [f32; 3] = [0.0, 0.4, 1.95];

/// The base that never moves: a cube sunk to its middle in the floor, its top 1.9 over its origin like the tank's roof.
const STILL_BASE: &str = "$T::Base = new StaticShape() { dataBlock = TraceFloorData; position = \"-30 0 0\"; \
scale = \"3 4 3.8\"; };";

/// What the turret stands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base {
    /// The tank the first client drives.
    Tank,
    /// A cube that never moves, while the first client drives the tank the same way beside it.
    Still,
}

/// What a [`run`] came to.
#[derive(Debug, Clone, PartialEq)]
pub struct TurretRun {
    /// The gunner's corrections after the turret settled (`None` without the gunner's client).
    pub gunner: Option<u64>,
    /// The driver's corrections after the turret settled.
    pub driver: u64,
    /// Whether the turret was still on its base at the end (on the server).
    pub aboard: bool,
    /// The farthest the gunner's client drew its turret from where its drawn base and the turret's local pose put it, over
    /// [`FRAMES`] frames a tick after the turret settled (units).
    pub drawn_gap: f32,
    /// How far the tank drove (the length of its path on the server), where it ended, and the ticks the driver's client
    /// predicted it.
    pub distance: f32,
    pub end: [f32; 3],
    pub driven_ticks: u32,
    /// The run's hash and the values in it.
    pub hash: u64,
    pub values: u64,
}

/// The driver's keys at `tick`: the throttle held, the steering swung one way then the other, and the brake now and then.
fn drive(rig: &mut Rig, tick: u32) {
    let phase = tick % 160;
    let yaw = match phase {
        0..40 => 0.01,
        40..120 => -0.01,
        _ => 0.01,
    };
    let braking = u32::from(tick % 200 >= 180);
    rig.set_global("$mvForwardAction", 1.0);
    rig.set_global("$mvYaw", yaw);
    rig.set_global("$mvTriggerCount2", (2 * (tick / 200) + braking) as f32);
}

/// The gunner's keys at `tick`: a turn one way then the other, and the gun looking up and down.
fn aim(rig: &mut Rig, gunner: usize, tick: u32) {
    let yaw = if tick % 120 < 60 { 0.004 } else { -0.003 };
    let pitch = if tick % 90 < 45 { 0.002 } else { -0.002 };
    let peer = &mut rig.peers[gunner];
    peer.set_global("$mvYaw", yaw);
    peer.set_global("$mvPitch", pitch);
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

/// Puts the turret on `base` at [`ON_ROOF`] in its frame, upright, and hangs it there.
fn mount_turret(rig: &mut Rig, turret: ObjId, base: ObjId) -> Result<(), String> {
    let at: Matrix = matrix_of(rig.server.vm(), base).ok_or("the base")?;
    let p = mul_p(&at, ON_ROOF);
    eval(
        rig,
        &format!(
            "{turret}.setTransform(\"{} {} {} 0 0 1 0\");",
            p[0], p[1], p[2]
        ),
    )?;
    if eval(rig, &format!("return {turret}.setParent({base}, 1);"))? != "1" {
        let error = eval(rig, "return getParentError();")?;
        return Err(format!("the turret was refused: {error}"));
    }
    Ok(())
}

/// The run ([module docs](self)): the turret on `base`, with the gunner's client when `gunner`, over a link that holds each
/// datagram from the server `latency` steps.
///
/// # Errors
/// A client never predicted, a script failed, or the turret could not be hung.
pub fn run(base: Base, gunner: bool, latency: u32) -> Result<TurretRun, String> {
    let mut rig = Rig::in_mode(Mode::Engine, Baseline, Control::Buggy)?;
    if !rig.settle(SETTLE_LIMIT) {
        return Err(format!(
            "the driver's client never predicted in {SETTLE_LIMIT} ticks"
        ));
    }
    eval(
        &mut rig,
        "$T::Gun = new Player() { dataBlock = TraceFigureData; position = \"0 0 50\"; };",
    )?;
    let turret = object(&mut rig, "$T::Gun")?;
    let carrier = match base {
        Base::Tank => object(&mut rig, "TraceBuggy.getId()")?,
        Base::Still => {
            eval(&mut rig, STILL_BASE)?;
            object(&mut rig, "$T::Base")?
        }
    };
    mount_turret(&mut rig, turret, carrier)?;
    let gunner = if gunner {
        eval(&mut rig, "$Trace::Next = $T::Gun;")?;
        Some(rig.add_client(SETTLE_LIMIT)?)
    } else {
        None
    };
    rig.set_latency(latency);
    let hierarchy = ServerHierarchy::of(rig.server.vm()).ok_or("the server has no hierarchy")?;
    let mut h = TraceHasher::new();
    let (mut driver_from, mut gunner_from) = (0, 0);
    let mut drawn_gap = 0.0f32;
    let tank = object(&mut rig, "TraceBuggy.getId()")?;
    let (mut distance, mut driven_ticks) = (0.0f32, 0);
    let mut last = position_of(&rig, tank)?;
    for tick in 0..TICKS {
        drive(&mut rig, tick);
        if let Some(g) = gunner {
            aim(&mut rig, g, tick);
        }
        rig.step();
        if tick == BOARDING {
            driver_from = corrections(&rig);
            gunner_from = gunner.map_or(0, |g| rig.peers[g].corrections());
        }
        observe_server(&rig, &mut h, Control::Buggy);
        driven_ticks += u32::from(observe_client(&rig, &mut h));
        let at = position_of(&rig, tank)?;
        distance += (0..3)
            .map(|k| (at[k] - last[k]).powi(2))
            .sum::<f32>()
            .sqrt();
        last = at;
        let vm = rig.server.vm();
        let s = torque_game::sim::player::state(vm, turret).ok_or("the turret went")?;
        h.floats(&s.obj_to_world[..12]);
        h.floats(&s.velocity);
        h.floats(&s.head);
        h.floats(&s.rot);
        match hierarchy.local(turret) {
            Some(local) => {
                h.floats(&local.translation);
                h.floats(&local.rotation);
            }
            None => h.bool(false),
        }
        if let Some(g) = gunner {
            let peer = &rig.peers[g];
            observe_state(&peer.client.borrow(), &mut h);
            for back in FRAMES {
                let (drawn, gap) = draw_turret(peer, back).ok_or("the gunner drew no turret")?;
                h.floats(&drawn);
                if tick >= BOARDING {
                    drawn_gap = drawn_gap.max(gap);
                }
            }
        }
    }
    let vm = rig.server.vm();
    let at = matrix_of(vm, turret).ok_or("the turret went")?;
    let under = matrix_of(vm, carrier).ok_or("the base went")?;
    let roof = mul_p(&under, ON_ROOF);
    let aboard =
        hierarchy.parent(turret).is_some() && (0..3).all(|k| (at[4 * k + 3] - roof[k]).abs() < 0.5);
    Ok(TurretRun {
        drawn_gap,
        distance,
        end: last,
        driven_ticks,
        gunner: gunner.map(|g| rig.peers[g].corrections() - gunner_from),
        driver: corrections(&rig) - driver_from,
        aboard,
        hash: h.finish(),
        values: h.count(),
    })
}

/// Where between its last two ticks the gunner's client draws a frame, for each of the four frames a tick it draws.
pub const FRAMES: [f32; 4] = [0.75, 0.5, 0.25, 0.0];

/// A frame the gunner's client draws `back` from its last tick: where it draws its turret (position, then yaw), and how far
/// that is from its drawn base times the turret's local pose.
fn draw_turret(peer: &Peer, back: f32) -> Option<([f32; 4], f32)> {
    let mut st = peer.client.borrow_mut();
    let c = &mut **st.conn.conn.as_mut()?;
    c.predict.back = back;
    c.predict.update_view(&mut c.world);
    c.predict.back = 0.0;
    let frame = c.predict.predicted.as_ref()?.frame?;
    let Some(Pose::Euler { position, yaw, .. }) = c.world.control_render else {
        return None;
    };
    let nodes = |_: u32, _: u16| None;
    let (base, scale) = c.world.world_pose(frame.parent, &nodes)?;
    // The parent's scale scales the local translation (the mount rule).
    let on = mul_p(
        &base,
        std::array::from_fn(|k| frame.local.translation[k] * scale[k]),
    );
    let gap = (0..3)
        .map(|k| (position[k] - on[k]).powi(2))
        .sum::<f32>()
        .sqrt();
    Some(([position[0], position[1], position[2], yaw], gap))
}

/// Where `obj` is on the server.
fn position_of(rig: &Rig, obj: ObjId) -> Result<[f32; 3], String> {
    let m = matrix_of(rig.server.vm(), obj).ok_or("an object of the scenario went")?;
    Ok([m[3], m[7], m[11]])
}

/// The driver's corrections so far.
fn corrections(rig: &Rig) -> u64 {
    rig.client
        .borrow()
        .conn
        .conn
        .as_ref()
        .map_or(0, |c| c.predict.stats.corrections)
}

/// The scenario's run: the turret on the tank, both clients connected.
pub fn turret() -> Result<Outcome, String> {
    let run = run(Base::Tank, true, LATENCY)?;
    if !run.aboard {
        return Err("the turret left the tank".into());
    }
    Ok(Outcome {
        hash: run.hash,
        values: run.values,
        distance: run.distance,
        end: run.end,
        corrections: run.gunner.unwrap_or(0) + run.driver,
        driven_ticks: run.driven_ticks,
    })
}
