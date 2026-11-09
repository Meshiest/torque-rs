//! The scenarios: a scripted run of the predicted paths, hashed tick by tick.
//!
//! Every scenario starts a [`Rig`] (a server of `MinimalPlugins` and the plugin under test, and a client connected over the
//! in-memory link), waits for the client's prediction to start, then runs [`TICKS`] ticks. Before each tick the client's move
//! globals are set from a scripted input (a seeded generator, so the same on every target); after it the hash takes in the
//! server's control object and the client's predicted one, each by the bits of its position, rotation, velocity and key fields,
//! and the client's checksum and prediction counters.
//!
//! **The predictors' scenarios** ([`predictors`]) run the same paths with another predictor on the client's prediction slot
//! ([`torque_client::predict::slot`]) and forced corrections, and fold in what is drawn each frame as well; they print after
//! `all` (which stays the fold of TGE's predictor's scenarios) and fold into `predictors`.
//!
//! **The physics scenarios** ([`physics`]) step a physics world alone (no server or client: the Rapier world, [`crate::rapier`]);
//! they print after the predictors' and fold into `physics`.
//!
//! **The plugins' scenarios** ([`plugins`]) run a server and a predicting client with an opt-in plugin that simulates on the server
//! (the props, [`crate::props`], and their joints, [`crate::joints`]); they print last and fold into `plugins`.
//!
//! **Adding a plugin's scenario:** a plugin whose systems run on a predicted path adds a [`Scenario`] to [`all`] that calls
//! [`figure`] or [`buggy`] with the plugin (or its own function built on [`Rig`]); a plugin that only adds a ghost or a channel
//! needs only [`trace_hash`], which runs both with the plugin added.

use torque_engine::{EngineBuilder, Plugin, PluginId, PluginType, Plugins};

use crate::hash::TraceHasher;
use crate::rig::{Control, Mode, Rig};

/// The ticks every scenario runs.
pub const TICKS: u32 = 600;
/// The most ticks a scenario waits for the connection and the prediction before its run.
const SETTLE_LIMIT: u32 = 400;
/// Ticks a set of keys is held.
const STRETCH: u32 = 15;

/// A named scenario: its name and protocol for the output and the run that makes its hash. A scenario that runs no protocol
/// (a physics world stepped alone) has no mode and prints the label `none`.
#[derive(Clone, Copy)]
pub struct Scenario {
    pub name: &'static str,
    pub mode: Option<Mode>,
    pub run: fn() -> Result<Outcome, String>,
}

impl Scenario {
    /// The word that labels the scenario's line: its protocol, or `none`.
    pub fn label(&self) -> &'static str {
        self.mode.map_or("none", Mode::label)
    }
}

/// Every scenario of the engine's own plugins, in the order they print. A plugin's scenario is one more entry here.
pub fn all() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "figure",
            mode: Some(Mode::Tge),
            run: || figure(Baseline),
        },
        Scenario {
            name: "buggy",
            mode: Some(Mode::Tge),
            run: || buggy(Baseline),
        },
        Scenario {
            name: "figure",
            mode: Some(Mode::Engine),
            run: || figure_in(Mode::Engine, Baseline),
        },
        Scenario {
            name: "buggy",
            mode: Some(Mode::Engine),
            run: || buggy_in(Mode::Engine, Baseline),
        },
    ]
}

/// The scenarios of the predictors other than TGE's, in the order they print after `all`.
pub fn predictors() -> Vec<Scenario> {
    vec![Scenario {
        name: "figure-smooth",
        mode: Some(Mode::Tge),
        run: figure_smooth,
    }]
}

/// The scenarios of the physics worlds a server and a predicting client both step outside TGE's objects, in the order they
/// print after `predictors`.
pub fn physics() -> Vec<Scenario> {
    vec![Scenario {
        name: "rapier-drop",
        mode: None,
        run: crate::rapier::drop,
    }]
}

/// The scenarios of the engine's opt-in plugins that a server simulates and a predicting client sees, in the order they print
/// after `physics`.
pub fn plugins() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "props-drop",
            mode: Some(Mode::Tge),
            run: crate::props::drop,
        },
        Scenario {
            name: "props-joints",
            mode: Some(Mode::Tge),
            run: crate::joints::grapple,
        },
        Scenario {
            name: "carry",
            mode: Some(Mode::Tge),
            run: crate::props_predict::carry,
        },
        Scenario {
            name: "projectile-props",
            mode: Some(Mode::Tge),
            run: crate::props_predict::shots,
        },
    ]
}

/// The scenarios of the scene's own parts that only the engine's protocol has on, in the order they print after `plugins`.
pub fn scene() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "hierarchy",
            mode: Some(Mode::Engine),
            run: crate::hierarchy::hierarchy,
        },
        Scenario {
            name: "hierarchy-props",
            mode: Some(Mode::Engine),
            run: crate::hierarchy_props::run,
        },
    ]
}

/// What a run came to: the hash, and numbers a caller can check the run was not an empty one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Outcome {
    pub hash: u64,
    /// Values hashed.
    pub values: u64,
    /// How far the server's control object went from where it began (units; the length of its path, for a scenario that holds it
    /// near its start), and where it ended.
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
pub(crate) fn observe_server(rig: &Rig, h: &mut TraceHasher, control: Control) {
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
pub(crate) fn observe_client(rig: &Rig, h: &mut TraceHasher) -> bool {
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
    mode: Mode,
    plugins: impl Plugins<M>,
    control: Control,
    seed: u64,
    keys: fn(&mut Rng) -> Keys,
) -> Result<Outcome, String> {
    run_with(mode, plugins, control, seed, keys, &mut Extras::default())
}

/// What a predictor's scenario adds to a run: the client's predictor, corrections forced on the server and the frames drawn.
#[derive(Default)]
struct Extras {
    /// The predictor the client runs (none: TGE's, with no slot).
    prediction: Option<torque_client::predict::slot::PredictionEntry>,
    /// Every `kick.0` ticks (at tick `kick.1` of each period) the server moves the control object by `kick.2`.
    kick: Option<(u32, u32, [f32; 3])>,
    /// The frames drawn after each tick, each folded in (0: none).
    frames: u32,
    /// The client's game in place of the rig's.
    client_game: Option<std::rc::Rc<dyn torque_client::game::ClientGame>>,
}

/// Moves the server's control object by `by` (a forced correction: the client predicted it elsewhere).
pub fn kick(rig: &mut Rig, control: Control, by: [f32; 3]) -> Result<(), String> {
    let name = match control {
        Control::Figure => "TraceFigure",
        Control::Buggy => "TraceBuggy",
    };
    let text = format!(
        "%t = {name}.getTransform(); {name}.setTransform(VectorAdd(getWords(%t, 0, 2), \"{} {} {}\") SPC getWords(%t, 3, 6));",
        by[0], by[1], by[2]
    );
    rig.server
        .vm_mut()
        .eval(text.as_bytes())
        .map(|_| ())
        .map_err(|e| format!("{e:?}"))
}

/// Gives the rig's client a prediction slot that runs `entry`'s predictor.
pub fn install_prediction(rig: &mut Rig, entry: &torque_client::predict::slot::PredictionEntry) {
    let (slot, _) =
        torque_client::predict::PredictionSlot::new(std::slice::from_ref(entry), Some(entry.id()));
    slot.install(&mut rig.client_vm);
}

/// What one frame drew of the control object and where its simulation stood.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawnFrame {
    /// The tick it follows (counted from the run's start) and where between the last two ticks it is drawn.
    pub tick: u32,
    pub back: f32,
    /// The drawn body (the vehicle the client drives, else its Player) and the camera.
    pub drawn: Option<[f32; 3]>,
    pub view: Option<[f32; 3]>,
    /// The simulated body at the last tick.
    pub sim: Option<[f32; 3]>,
}

/// `frames` frames drawn after the last tick, evenly between it and the one before (as a window at `frames` times the tick
/// rate draws them).
pub fn draw_frames(rig: &Rig, frames: u32) -> Vec<DrawnFrame> {
    (1..=frames)
        .filter_map(|k| draw_frame(rig, 1.0 - k as f32 / frames as f32))
        .collect()
}

/// A frame drawn `back` from the client's last tick (0 at it, 1 at the one before).
pub fn draw_frame(rig: &Rig, back: f32) -> Option<DrawnFrame> {
    let mut st = rig.client.borrow_mut();
    let c = &mut **st.conn.conn.as_mut()?;
    c.predict.back = back;
    c.predict.update_view(&mut c.world);
    // The driven vehicle when there is one, else the Player.
    let driven = c
        .predict
        .predicted
        .as_ref()
        .and_then(|p| p.vehicle.as_ref());
    let pose = match driven {
        Some(v) => c.world.vehicle_render.get(&v.index),
        None => c.world.control_render.as_ref(),
    };
    let sim = match driven {
        Some(v) => Some(v.v.s.position()),
        None => c.predict.predicted.as_ref().map(|p| p.player.s.position()),
    };
    let frame = DrawnFrame {
        tick: rig.ticks,
        back,
        drawn: pose.and_then(torque_client::predict::slot::pose_position),
        view: c.world.view.as_ref().map(|e| e.position),
        sim,
    };
    c.predict.back = 0.0;
    Some(frame)
}

fn run_with<M>(
    mode: Mode,
    plugins: impl Plugins<M>,
    control: Control,
    seed: u64,
    keys: fn(&mut Rng) -> Keys,
    extras: &mut Extras,
) -> Result<Outcome, String> {
    let mut rig = Rig::in_mode(mode, plugins, control)?;
    if let Some(game) = extras.client_game.take() {
        torque_client::game::set_client_game(&mut rig.client_vm, game);
    }
    if let Some(entry) = extras.prediction.take() {
        install_prediction(&mut rig, &entry);
    }
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
        if let Some((every, at, by)) = extras.kick
            && tick % every == at
        {
            kick(&mut rig, control, by)?;
        }
        rig.step();
        observe_server(&rig, &mut h, control);
        driven_ticks += u32::from(observe_client(&rig, &mut h));
        for frame in draw_frames(&rig, extras.frames) {
            h.floats(&[frame.back]);
            for p in [frame.drawn, frame.view] {
                match p {
                    Some(p) => h.floats(&p),
                    None => h.u32(0),
                }
            }
        }
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
    figure_in(Mode::Tge, plugins)
}

/// [`figure`] over the protocol `mode`.
pub fn figure_in<M>(mode: Mode, plugins: impl Plugins<M>) -> Result<Outcome, String> {
    run(
        mode,
        plugins,
        Control::Figure,
        0x9e37_79b9_7f4a_7c15,
        Keys::figure,
    )
}

/// `control` standing still on the floor (no keys held), the server moving it by `by` every `every` ticks (`kicks` times, the
/// first after `every` ticks), with `prediction` on the client's slot (none: TGE's): the frames drawn, `frames` a tick, from the
/// first kick to `every` ticks after the last.
pub fn forced_corrections(
    control: Control,
    prediction: Option<torque_client::predict::slot::PredictionEntry>,
    by: [f32; 3],
    every: u32,
    kicks: u32,
    frames: u32,
) -> Result<Vec<DrawnFrame>, String> {
    let mut rig = Rig::new(Baseline, control)?;
    if let Some(entry) = prediction {
        install_prediction(&mut rig, &entry);
    }
    if !rig.settle(SETTLE_LIMIT) {
        return Err("the client never predicted its control object".into());
    }
    Keys::default().apply(&mut rig, 0);
    for _ in 0..every {
        rig.step();
    }
    let mut out = Vec::new();
    for _ in 0..kicks {
        kick(&mut rig, control, by)?;
        for _ in 0..every {
            rig.step_frames(frames, |rig, back| out.extend(draw_frame(rig, back)));
        }
    }
    Ok(out)
}

/// What a client's prediction of a control object at rest came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Idle {
    /// The server's heads that resent the state, of them the ones counted as corrections, and the heads that matched.
    pub resends: u64,
    pub corrections: u64,
    pub matching: u64,
}

/// `control` standing still on the floor (no keys held) for `ticks` ticks, after `warm` ticks that let it come to rest (the buggy
/// drops onto its springs): what the client's prediction did over the `ticks`.
pub fn idle(control: Control, warm: u32, ticks: u32) -> Result<Idle, String> {
    let mut rig = Rig::new(Baseline, control)?;
    if !rig.settle(SETTLE_LIMIT) {
        return Err("the client never predicted its control object".into());
    }
    Keys::default().apply(&mut rig, 0);
    let stats = |rig: &Rig| {
        let st = rig.client.borrow();
        st.conn.conn.as_ref().map(|c| c.predict.stats)
    };
    for _ in 0..warm {
        rig.step();
    }
    let before = stats(&rig).ok_or("no connection")?;
    for _ in 0..ticks {
        rig.step();
    }
    let after = stats(&rig).ok_or("no connection")?;
    Ok(Idle {
        resends: after.resends - before.resends,
        corrections: after.corrections - before.corrections,
        matching: after.matching - before.matching,
    })
}

/// The figure on foot with the smoothed predictor (`torque-plugin-smooth-prediction`): the server moves it sideways every 75
/// ticks, so the client corrects, and four frames a tick are drawn and folded in.
pub fn figure_smooth() -> Result<Outcome, String> {
    use torque_plugin_smooth_prediction::SmoothPrediction;
    let mut extras = Extras {
        prediction: Some(torque_client::predict::slot::PredictionEntry::new(
            SmoothPrediction::default(),
        )),
        kick: Some((75, 40, [0.8, 0.3, 0.0])),
        frames: 4,
        client_game: None,
    };
    run_with(
        Mode::Tge,
        Baseline,
        Control::Figure,
        0x9e37_79b9_7f4a_7c15,
        Keys::figure,
        &mut extras,
    )
}

/// The buggy: a server `WheeledVehicle` its client drives and the client's prediction of it, run with `plugins` added.
pub fn buggy<M>(plugins: impl Plugins<M>) -> Result<Outcome, String> {
    buggy_in(Mode::Tge, plugins)
}

/// [`buggy`] over the protocol `mode`.
pub fn buggy_in<M>(mode: Mode, plugins: impl Plugins<M>) -> Result<Outcome, String> {
    run(
        mode,
        plugins,
        Control::Buggy,
        0xd1b5_4a32_d192_ed03,
        Keys::buggy,
    )
}

/// [`buggy`] with `game` as the client's game (it reads the shapes from [`crate::rig::files`], as the rig's own does).
pub fn buggy_with_client_game(
    game: std::rc::Rc<dyn torque_client::game::ClientGame>,
) -> Result<Outcome, String> {
    run_with(
        Mode::Tge,
        Baseline,
        Control::Buggy,
        0xd1b5_4a32_d192_ed03,
        Keys::buggy,
        &mut Extras {
            client_game: Some(game),
            ..Extras::default()
        },
    )
}

/// The 600-tick trace hash of the plugin `P` (made with its `Default`) on every predicted path: the two scenarios' hashes
/// folded into one. The plugin is added to the server's engine, so what it registers runs under the scripted inputs.
pub fn trace_hash<P: PluginType + Default>() -> Result<u64, String> {
    let mut h = TraceHasher::new();
    h.u64(figure(P::default())?.hash);
    h.u64(buggy(P::default())?.hash);
    Ok(h.finish())
}
