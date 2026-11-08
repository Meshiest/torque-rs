//! The scenarios: a scripted run of the predicted paths, hashed tick by tick.
//!
//! Every scenario starts a [`Rig`] (a server of `MinimalPlugins` and the plugin under test, and a client connected over the
//! in-memory link), waits for the client's prediction to start, then runs [`TICKS`] ticks. Before each tick the client's move
//! globals are set from a scripted input (a seeded generator, so the same on every target); after it the hash takes in the
//! server's control object and the client's predicted one, each by the bits of its position, rotation, velocity and key fields,
//! and the client's checksum and prediction counters.
//!
//! **Adding a plugin's scenario:** a plugin whose systems run on a predicted path adds a [`Scenario`] to [`all`] that calls
//! [`figure`] or [`buggy`] with the plugin (or its own function built on [`Rig`]); a plugin that only adds a ghost or a channel
//! needs only [`trace_hash`], which runs both with the plugin added.

use torque_engine::{EngineBuilder, Plugin, PluginId, PluginType, Plugins};

use crate::hash::TraceHasher;
use crate::rig::{Control, Rig};

/// The ticks every scenario runs.
pub const TICKS: u32 = 600;
/// The most ticks a scenario waits for the connection and the prediction before its run.
const SETTLE_LIMIT: u32 = 400;
/// Ticks a set of keys is held.
const STRETCH: u32 = 15;

/// A named scenario: its name for the output and the run that makes its hash.
#[derive(Clone, Copy)]
pub struct Scenario {
    pub name: &'static str,
    pub run: fn() -> Result<Outcome, String>,
}

/// Every scenario of the engine's own plugins, in the order they print. A plugin's scenario is one more entry here.
pub fn all() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "figure",
            run: || figure(Baseline),
        },
        Scenario {
            name: "buggy",
            run: || buggy(Baseline),
        },
    ]
}

/// What a run came to: the hash, and numbers a caller can check the run was not an empty one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Outcome {
    pub hash: u64,
    /// Values hashed.
    pub values: u64,
    /// How far the server's control object went from where it began (units), and where it ended.
    pub distance: f32,
    pub end: [f32; 3],
    /// The client's server heads that corrected its prediction.
    pub corrections: u64,
    /// The ticks the client predicted a vehicle it drives.
    pub driven_ticks: u32,
}

/// The plugin of the engine's own scenarios: it adds nothing, so they run on `MinimalPlugins` alone.
#[derive(Clone, Copy, Debug, Default, PluginId)]
pub struct Baseline;

impl Plugin for Baseline {
    fn build(&self, _engine: &mut EngineBuilder) {}
}

/// A small xorshift generator: a scenario's inputs must not depend on anything but a seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// One of `n` choices.
    fn below(&mut self, n: u64) -> u64 {
        (self.next() >> 11) % n
    }
}

/// The keys held for a stretch of ticks.
#[derive(Clone, Copy, Default)]
struct Keys {
    forward: bool,
    back: bool,
    left: bool,
    right: bool,
    /// The mouse's turn each tick (radians).
    yaw: f32,
    pitch: f32,
    jump: bool,
    crouch: bool,
}

impl Keys {
    /// The next stretch of keys: a figure mostly runs forward, turns and jumps now and then.
    fn figure(rng: &mut Rng) -> Keys {
        Keys {
            forward: rng.below(5) != 0,
            back: rng.below(12) == 0,
            left: rng.below(6) == 0,
            right: rng.below(6) == 0,
            yaw: (rng.below(9) as f32 - 4.0) * 0.004,
            pitch: (rng.below(5) as f32 - 2.0) * 0.002,
            jump: rng.below(5) == 0,
            crouch: rng.below(8) == 0,
        }
    }

    /// The next stretch for the buggy: throttle, steering (the move's yaw) and the brake (trigger 2) now and then.
    fn buggy(rng: &mut Rng) -> Keys {
        Keys {
            forward: rng.below(8) != 0,
            back: rng.below(10) == 0,
            left: rng.below(3) == 0,
            right: rng.below(3) == 0,
            yaw: (rng.below(9) as f32 - 4.0) * 0.01,
            jump: rng.below(9) == 0,
            ..Keys::default()
        }
    }

    fn apply(&self, rig: &mut Rig, tick: u32) {
        let held = |on: bool| if on { 1.0 } else { 0.0 };
        rig.set_global("$mvForwardAction", held(self.forward));
        rig.set_global("$mvBackwardAction", held(self.back));
        rig.set_global("$mvLeftAction", held(self.left));
        rig.set_global("$mvRightAction", held(self.right));
        rig.set_global("$mvYaw", self.yaw);
        rig.set_global("$mvPitch", self.pitch);
        // A trigger is held while its count is odd; each stretch moves the count on, so a held key is one press.
        let count = |on: bool| (2 * (tick / STRETCH) + u32::from(on)) as f32;
        rig.set_global("$mvTriggerCount2", count(self.jump));
        rig.set_global("$mvTriggerCount3", count(self.crouch));
    }
}

/// The first twelve words of a transform (the rotation rows and the translation column of a row-major 4x4).
fn transform(m: &[f32]) -> [f32; 12] {
    std::array::from_fn(|i| m.get(i).copied().unwrap_or(0.0))
}

/// What the server's control object holds, folded in by its bits.
fn observe_server(rig: &Rig, h: &mut TraceHasher, control: Control) {
    use torque_game::sim::vehicle::{Body, ext_of};
    let vm = rig.server.vm();
    match control {
        Control::Figure => {
            let state = vm
                .objects()
                .find(b"TraceFigure")
                .and_then(|id| torque_game::sim::player::state(vm, id));
            let Some(s) = state else {
                h.u32(0);
                return;
            };
            h.u32(1);
            h.floats(&transform(&s.obj_to_world));
            h.floats(&s.velocity);
            h.floats(&s.head);
            h.floats(&s.rot);
            h.floats(&[s.energy, s.damage]);
            h.bool(s.falling);
            h.u32(s.jump_delay as u32);
            h.u32(s.contact_timer);
        }
        Control::Buggy => {
            let Some(id) = vm.objects().find(b"TraceBuggy") else {
                h.u32(0);
                return;
            };
            let Some(s) = torque_game::sim::vehicle::state(vm, id) else {
                h.u32(0);
                return;
            };
            h.u32(1);
            h.floats(&transform(&s.obj_to_world));
            h.floats(&s.rigid.lin_vel);
            h.floats(&s.rigid.ang_vel);
            h.floats(&s.rigid.lin_pos);
            h.floats(&s.rigid.ang_pos);
            for w in s.rigid.to_words() {
                h.u32(w);
            }
            h.floats(&s.steering);
            h.floats(&[s.throttle, s.damage]);
            h.bool(s.rigid.at_rest);
            h.u32(s.rest_count as u32);
            h.u32(s.contacts_count);
            if let Some(Body::Wheeled(v)) = ext_of(vm, id).and_then(|e| e.body.as_ref()) {
                for w in &v.k.wheels {
                    h.floats(&[w.extension, w.avel, w.apos, w.dx, w.dy]);
                    h.bool(w.contact);
                }
            }
        }
    }
}

/// What the client's prediction holds, folded in by its bits; whether it was running a vehicle.
fn observe_client(rig: &Rig, h: &mut TraceHasher) -> bool {
    let st = rig.client.borrow();
    let Some(conn) = st.conn.conn.as_ref() else {
        h.u32(0);
        return false;
    };
    let p = &conn.predict;
    h.u32(p.checksum());
    h.u64(p.stats.steps);
    h.u64(p.stats.resends);
    h.u64(p.stats.matching);
    h.u64(p.stats.corrections);
    h.u32(p.last_client_move);
    let Some(predicted) = p.predicted.as_ref() else {
        h.u32(0);
        return false;
    };
    h.u32(1);
    let s = &predicted.player.s;
    h.floats(&transform(&s.obj_to_world));
    h.floats(&s.velocity);
    h.floats(&s.head);
    h.floats(&s.rot);
    h.bool(s.falling);
    if let Some(driven) = &predicted.vehicle {
        let v = &driven.v.s;
        h.u32(2);
        h.floats(&transform(&v.obj_to_world));
        h.floats(&v.rigid.lin_vel);
        h.floats(&v.rigid.ang_vel);
        h.floats(&v.rigid.lin_pos);
        h.floats(&v.rigid.ang_pos);
        for w in v.rigid.to_words() {
            h.u32(w);
        }
        h.floats(&v.steering);
        for w in &driven.v.k.wheels {
            h.floats(&[w.extension, w.avel, w.apos]);
        }
    }
    predicted.vehicle.is_some()
}

/// The server's control object's position, for the distance a run went.
fn position(rig: &Rig, control: Control) -> Option<[f32; 3]> {
    let vm = rig.server.vm();
    match control {
        Control::Figure => {
            torque_game::sim::player::position(vm, vm.objects().find(b"TraceFigure")?)
        }
        Control::Buggy => torque_game::sim::vehicle::state(vm, vm.objects().find(b"TraceBuggy")?)
            .map(|s| [s.obj_to_world[3], s.obj_to_world[7], s.obj_to_world[11]]),
    }
}

fn run<M>(
    plugins: impl Plugins<M>,
    control: Control,
    seed: u64,
    keys: fn(&mut Rng) -> Keys,
) -> Result<Outcome, String> {
    let mut rig = Rig::new(plugins, control)?;
    if !rig.settle(SETTLE_LIMIT) {
        return Err(format!(
            "the client never predicted its control object in {SETTLE_LIMIT} ticks: {:?}",
            rig.server.drain_log()
        ));
    }
    let start = position(&rig, control).ok_or("the server has no control object")?;
    let mut rng = Rng(seed);
    let mut h = TraceHasher::new();
    let mut current = Keys::default();
    let mut driven_ticks = 0;
    for tick in 0..TICKS {
        if tick % STRETCH == 0 {
            current = keys(&mut rng);
        }
        current.apply(&mut rig, tick);
        rig.step();
        observe_server(&rig, &mut h, control);
        driven_ticks += u32::from(observe_client(&rig, &mut h));
    }
    let end = position(&rig, control).ok_or("the server lost its control object")?;
    let distance = (0..3)
        .map(|i| (end[i] - start[i]).powi(2))
        .sum::<f32>()
        .sqrt();
    let corrections = rig
        .client
        .borrow()
        .conn
        .conn
        .as_ref()
        .map_or(0, |c| c.predict.stats.corrections);
    Ok(Outcome {
        hash: h.finish(),
        values: h.count(),
        distance,
        end,
        corrections,
        driven_ticks,
    })
}

/// The figure on foot: a server `Player` and the client's prediction of it, run with `plugins` added to the engine.
pub fn figure<M>(plugins: impl Plugins<M>) -> Result<Outcome, String> {
    run(
        plugins,
        Control::Figure,
        0x9e37_79b9_7f4a_7c15,
        Keys::figure,
    )
}

/// The buggy: a server `WheeledVehicle` its client drives and the client's prediction of it, run with `plugins` added.
pub fn buggy<M>(plugins: impl Plugins<M>) -> Result<Outcome, String> {
    run(plugins, Control::Buggy, 0xd1b5_4a32_d192_ed03, Keys::buggy)
}

/// The 600-tick trace hash of the plugin `P` (made with its `Default`) on every predicted path: the two scenarios' hashes
/// folded into one. The plugin is added to the server's engine, so what it registers runs under the scripted inputs.
pub fn trace_hash<P: PluginType + Default>() -> Result<u64, String> {
    let mut h = TraceHasher::new();
    h.u64(figure(P::default())?.hash);
    h.u64(buggy(P::default())?.hash);
    Ok(h.finish())
}
