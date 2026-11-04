//! The figure's motion and its animation state machine.
//!
//! **States.** [`State::Idle`], [`State::Walk`] and [`State::Run`] on the ground, by the figure's ground speed; [`State::Jump`]
//! from a jump until it lands. The input sets a wished direction (relative to the camera) and whether to run; the ground speed
//! eases toward the wished speed, so a figure starting to move walks before it runs, and one stopping walks before it stands.
//!
//! **Threads.** One thread plays the state's sequence. A change of state is a transition (TGE's `transitionToSequence`, as a
//! Player's action thread changes action): the pose blends from where it was to the new sequence over [`TRANSITION`] seconds
//! while the new one plays. Walk and run keep their phase when one becomes the other (both start with the same leg forward);
//! idle and jump start at their beginning.
//!
//! **Holding.** The `hold` arm pose plays on a second thread. It comes in and goes out over [`HOLD_BLEND`] seconds: each step
//! poses the figure once without it (the second thread playing the state's sequence where the first is) and once with it, and
//! the instance's pose blend moves the first pose toward the second by the hold's weight. Only the arm `hold` moves differs
//! between the two, so the rest of the figure goes on as it was.
//!
//! **Ground speed.** Walk and run carry ground frames: their sequence moves the figure a known distance a second
//! ([`ground_speed`]). The thread's time scale is the figure's speed over that, as a Player scales its action thread by its
//! velocity over the action's speed, so the feet keep pace with the floor at any speed.
//!
//! Credit: the transition time and the time-scale rule follow the Player's action thread as Torque3D carries it under the
//! MIT licence (`Engine/source/T3D/player.cpp`, Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.).
//! The code here is this crate's own.

use torque::shapes::instance::{PoseBlend, ShapeInstance, Thread, ThreadId, find_sequence};
use torque::shapes::shape::Shape;
use torque_render_scene::math::Mat;

/// Seconds a change of state blends over (a Player's action transition time).
pub const TRANSITION: f32 = 0.25;
/// Seconds the `hold` pose takes to come in or go out.
pub const HOLD_BLEND: f32 = 0.2;
/// Ground acceleration and the slower steering in the air (units a second, a second).
pub const ACCELERATION: f32 = 12.0;
pub const AIR_ACCELERATION: f32 = 3.0;
/// Below this ground speed the figure stands.
pub const IDLE_BELOW: f32 = 0.2;
/// The upward speed of a jump and the fall's acceleration: half a second in the air, the length of the jump sequence.
pub const JUMP_SPEED: f32 = 5.0;
pub const GRAVITY: f32 = 20.0;
/// How fast the figure turns to face its way (radians a second).
pub const TURN_RATE: f32 = 10.0;

/// The figure's animation state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Idle,
    Walk,
    Run,
    Jump,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::Walk => "walk",
            State::Run => "run",
            State::Jump => "jump",
        }
    }
}

/// What the player asks for this step.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Input {
    /// Forward (+1) or back (-1), and right (+1) or left (-1), relative to the camera.
    pub forward: f32,
    pub right: f32,
    pub run: bool,
    /// Whether the jump key is down (a jump starts when it goes down on the ground).
    pub jump: bool,
}

/// How far a sequence's ground frames carry the figure in a second: the last frame's distance over the sequence's duration
/// (0 for a sequence without ground frames).
pub fn ground_speed(shape: &Shape, seq: usize) -> f32 {
    let Some(s) = shape.sequences.get(seq) else {
        return 0.0;
    };
    if s.num_ground_frames <= 0 || s.duration <= 0.0 {
        return 0.0;
    }
    let last = (s.first_ground_frame + s.num_ground_frames - 1) as usize;
    shape.ground_translations.get(last).map_or(0.0, |p| {
        (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt() / s.duration
    })
}

/// The figure's sequences and its gaits' ground speeds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sequences {
    pub idle: usize,
    pub walk: usize,
    pub run: usize,
    pub jump: usize,
    /// The arm pose of holding a pistol (played over the others on a second thread), when the shape has one.
    pub hold: Option<usize>,
    pub walk_speed: f32,
    pub run_speed: f32,
}

impl Sequences {
    /// The sequences named `idle`, `walk`, `run` and `jump`; None when one is missing or a gait has no ground speed.
    pub fn of(shape: &Shape) -> Option<Sequences> {
        let seq = |name: &str| find_sequence(shape, name.as_bytes());
        let (walk, run) = (seq("walk")?, seq("run")?);
        let s = Sequences {
            idle: seq("idle")?,
            walk,
            run,
            jump: seq("jump")?,
            hold: seq("hold"),
            walk_speed: ground_speed(shape, walk),
            run_speed: ground_speed(shape, run),
        };
        (s.walk_speed > 0.0 && s.run_speed > s.walk_speed).then_some(s)
    }

    fn of_state(&self, state: State) -> usize {
        match state {
            State::Idle => self.idle,
            State::Walk => self.walk,
            State::Run => self.run,
            State::Jump => self.jump,
        }
    }
}

/// The `hold` pose's thread and how far it is in (0 out, 1 in).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Hold {
    thread: ThreadId,
    weight: f32,
}

/// The figure: where it is, its velocity and heading, its state and its animated pose.
pub struct Figure {
    /// Its feet (z 0 on the floor).
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    /// Radians about z; 0 faces +y.
    pub heading: f32,
    state: State,
    instance: ShapeInstance,
    thread: ThreadId,
    sequences: Sequences,
    detail: usize,
    jump_held: bool,
    /// The `hold` pose, from the step it is asked for until it has gone out again.
    hold: Option<Hold>,
    /// Whether the figure is to hold a pistol.
    hold_wanted: bool,
}

impl Figure {
    /// The figure standing at the origin, idle, posed with its detail level `detail`.
    pub fn new(shape: &Shape, detail: usize) -> Option<Figure> {
        let sequences = Sequences::of(shape)?;
        let mut instance = ShapeInstance::new(shape, &mut |_| true).ok()?;
        let thread = instance.create_thread(shape)?;
        instance.set_sequence(shape, thread, sequences.idle as i32, 0.0);
        let mut figure = Figure {
            position: [0.0; 3],
            velocity: [0.0; 3],
            heading: 0.0,
            state: State::Idle,
            instance,
            thread,
            sequences,
            detail,
            jump_held: false,
            hold: None,
            hold_wanted: false,
        };
        figure.animate(shape);
        Some(figure)
    }

    /// Whether the figure has a pistol in its hand: from when it is asked to hold one until its arm is down again.
    pub fn holding(&self) -> bool {
        self.hold.is_some()
    }

    /// How far the `hold` pose is in: 0 out, 1 in (eased).
    pub fn hold_weight(&self) -> f32 {
        self.hold.map_or(0.0, |h| ease(h.weight))
    }

    /// Asks for the `hold` pose (a second thread over the state's, the right arm raised) or for its end; the steps after blend
    /// it in or out. Nothing when the shape has no `hold` sequence.
    pub fn set_holding(&mut self, shape: &Shape, on: bool) {
        let Some(seq) = self.sequences.hold else {
            return;
        };
        self.hold_wanted = on;
        if on
            && self.hold.is_none()
            && let Some(thread) = self.instance.create_thread(shape)
        {
            self.instance.set_sequence(shape, thread, seq as i32, 0.0);
            self.hold = Some(Hold {
                thread,
                weight: 0.0,
            });
        }
    }

    /// Moves the hold's weight `dt` seconds toward where it is asked to be; a hold gone out leaves its thread.
    fn blend_hold(&mut self, shape: &Shape, dt: f32) {
        let Some(hold) = self.hold.as_mut() else {
            return;
        };
        let d = dt / HOLD_BLEND;
        hold.weight = if self.hold_wanted {
            (hold.weight + d).min(1.0)
        } else {
            (hold.weight - d).max(0.0)
        };
        if !self.hold_wanted && hold.weight <= 0.0 {
            self.instance.destroy_thread(shape, hold.thread);
            self.hold = None;
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn sequences(&self) -> &Sequences {
        &self.sequences
    }

    /// The pose, as the last step animated it.
    pub fn instance(&self) -> &ShapeInstance {
        &self.instance
    }

    /// The thread playing the state's sequence.
    pub fn thread(&self) -> &Thread {
        self.instance
            .thread(self.thread)
            .expect("the figure's thread is never destroyed")
    }

    /// Its speed along the ground.
    pub fn ground_speed(&self) -> f32 {
        self.velocity[0].hypot(self.velocity[1])
    }

    /// Where the figure stands and faces.
    pub fn transform(&self) -> Mat {
        let mut m = Mat::euler_z(self.heading);
        m.set_column3(self.position);
        m
    }

    /// Moves the figure `dt` seconds by `input`, `forward` and `right` being the camera's ways along the ground (unit), and
    /// plays its state's sequence. Returns whether the state changed.
    pub fn step(
        &mut self,
        shape: &Shape,
        input: &Input,
        forward: [f32; 2],
        right: [f32; 2],
        dt: f32,
    ) -> bool {
        let dt = dt.clamp(0.0, 0.1);
        let on_ground = self.position[2] <= 0.0 && self.velocity[2] <= 0.0;
        // The wished velocity along the ground.
        let mut wish = [
            input.forward * forward[0] + input.right * right[0],
            input.forward * forward[1] + input.right * right[1],
        ];
        let length = wish[0].hypot(wish[1]);
        if length > 1.0 {
            wish = wish.map(|v| v / length);
        }
        let top = if input.run {
            self.sequences.run_speed
        } else {
            self.sequences.walk_speed
        };
        let wished = wish.map(|v| v * top);
        let accel = if on_ground {
            ACCELERATION
        } else {
            AIR_ACCELERATION
        };
        let dv = [wished[0] - self.velocity[0], wished[1] - self.velocity[1]];
        let dl = dv[0].hypot(dv[1]);
        let most = accel * dt;
        let k = if dl > most { most / dl } else { 1.0 };
        self.velocity[0] += dv[0] * k;
        self.velocity[1] += dv[1] * k;
        // A jump when the key goes down on the ground.
        if on_ground && input.jump && !self.jump_held {
            self.velocity[2] = JUMP_SPEED;
        }
        self.jump_held = input.jump;
        let airborne = self.velocity[2] > 0.0 || self.position[2] > 0.0;
        if airborne {
            self.velocity[2] -= GRAVITY * dt;
        }
        for (p, v) in self.position.iter_mut().zip(self.velocity) {
            *p += v * dt;
        }
        if self.position[2] <= 0.0 {
            self.position[2] = 0.0;
            self.velocity[2] = 0.0;
        }
        // Face the way it moves.
        let speed = self.ground_speed();
        if speed > IDLE_BELOW {
            let want = self.velocity[0].atan2(self.velocity[1]);
            let turn = (want - self.heading + std::f32::consts::PI)
                .rem_euclid(std::f32::consts::TAU)
                - std::f32::consts::PI;
            let most = TURN_RATE * dt;
            self.heading =
                (self.heading + turn.clamp(-most, most)).rem_euclid(std::f32::consts::TAU);
        }
        let next = if self.position[2] > 0.0 || self.velocity[2] > 0.0 {
            State::Jump
        } else if speed < IDLE_BELOW {
            State::Idle
        } else if speed <= 0.5 * (self.sequences.walk_speed + self.sequences.run_speed) {
            State::Walk
        } else {
            State::Run
        };
        let changed = next != self.state;
        if changed {
            self.play(shape, next);
        }
        let scale = match self.state {
            State::Walk => speed / self.sequences.walk_speed,
            State::Run => speed / self.sequences.run_speed,
            State::Idle | State::Jump => 1.0,
        };
        if let Some(t) = self.instance.thread_mut(self.thread) {
            t.time_scale = scale;
        }
        self.instance.advance_time(shape, self.thread, dt);
        self.blend_hold(shape, dt);
        self.animate(shape);
        changed
    }

    /// Blends into `state`'s sequence.
    fn play(&mut self, shape: &Shape, state: State) {
        let gait = |s: State| matches!(s, State::Walk | State::Run);
        let pos = if gait(self.state) && gait(state) {
            self.thread().pos
        } else {
            0.0
        };
        let seq = self.sequences.of_state(state);
        self.instance
            .transition_to_sequence(shape, self.thread, seq, pos, TRANSITION);
        self.state = state;
    }

    /// Poses the figure: with the hold part of the way in, the pose without it blended toward the pose with it.
    fn animate(&mut self, shape: &Shape) {
        let (Some(hold), Some(hold_seq)) = (self.hold, self.sequences.hold) else {
            self.pose(shape, None);
            return;
        };
        if hold.weight >= 1.0 {
            self.pose(shape, None);
            return;
        }
        // Without the hold: its thread plays the state's sequence where the state's thread is, which poses nothing else.
        let (seq, pos) = {
            let t = self.thread();
            (t.sequence.map_or(-1, |s| s as i32), t.pos)
        };
        self.pose_with(shape, hold.thread, seq, pos, None);
        let (rotations, translations) = self.instance.local_pose();
        let blend = PoseBlend {
            rotations,
            translations,
            weight: ease(hold.weight),
        };
        self.pose_with(shape, hold.thread, hold_seq as i32, 0.0, Some(blend));
    }

    /// Poses the figure with the hold's thread playing `seq` at `pos`, through `blend`.
    fn pose_with(
        &mut self,
        shape: &Shape,
        thread: ThreadId,
        seq: i32,
        pos: f32,
        blend: Option<PoseBlend>,
    ) {
        self.instance.set_sequence(shape, thread, seq, pos);
        self.pose(shape, blend);
    }

    fn pose(&mut self, shape: &Shape, blend: Option<PoseBlend>) {
        if blend.is_some() || self.instance.pose_blend().is_some() {
            self.instance.set_pose_blend(blend);
        }
        // The figure's own shape: nothing to run out of.
        let _ = self
            .instance
            .animate(shape, self.detail, None, &mut |_| true);
    }
}

/// Smoothstep: a weight that starts and ends slowly.
fn ease(w: f32) -> f32 {
    let w = w.clamp(0.0, 1.0);
    w * w * (3.0 - 2.0 * w)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn figure_shape() -> Shape {
        Shape::for_server(torque_example_assets::shapes::figure())
    }

    #[test]
    fn the_gaits_ground_speeds_are_the_kits() {
        let shape = figure_shape();
        let s = Sequences::of(&shape).unwrap();
        assert!((s.walk_speed - torque_example_assets::shapes::WALK_SPEED).abs() < 1e-4);
        assert!((s.run_speed - torque_example_assets::shapes::RUN_SPEED).abs() < 1e-4);
        assert_eq!(ground_speed(&shape, s.idle), 0.0);
    }

    /// Where the right hand (`mount0`) is in the figure's own space.
    fn hand(shape: &Shape, f: &Figure) -> [f32; 3] {
        let n = shape.find_name(b"mount0").unwrap();
        let node = shape.nodes.iter().position(|x| x.name == n).unwrap();
        torque::shapes::math::mul_p(&f.instance().node_transforms()[node], [0.0; 3])
    }

    #[test]
    fn the_hold_pose_blends_in_and_out_without_a_jump() {
        let shape = figure_shape();
        let mut f = Figure::new(&shape, 0).unwrap();
        let walk = Input {
            forward: 1.0,
            ..Input::default()
        };
        let step = |f: &mut Figure| f.step(&shape, &walk, [0.0, 1.0], [1.0, 0.0], 1.0 / 60.0);
        for _ in 0..60 {
            step(&mut f);
        }
        let down = hand(&shape, &f);
        // In: the hand rises over HOLD_BLEND, never more than a share of the way in one frame.
        f.set_holding(&shape, true);
        let mut last = hand(&shape, &f);
        let mut most = 0.0f32;
        for _ in 0..30 {
            step(&mut f);
            let now = hand(&shape, &f);
            most = most.max((now[2] - last[2]).abs());
            last = now;
        }
        let up = hand(&shape, &f);
        assert_eq!(f.hold_weight(), 1.0);
        let rise = up[2] - down[2];
        assert!(rise > 0.3, "the hand is raised: {down:?} {up:?}");
        assert!(most < 0.25 * rise, "a frame moves it {most} of {rise}");
        // Out: the same the other way, and the thread is gone once the arm is down.
        f.set_holding(&shape, false);
        let mut most = 0.0f32;
        let mut last = hand(&shape, &f);
        for _ in 0..30 {
            step(&mut f);
            let now = hand(&shape, &f);
            most = most.max((now[2] - last[2]).abs());
            last = now;
        }
        assert!(!f.holding() && f.instance().thread_count() == 1);
        assert!(most < 0.25 * rise, "a frame moves it {most} of {rise}");
    }

    #[test]
    fn the_thread_plays_at_the_speed_over_the_ground_speed() {
        let shape = figure_shape();
        let mut f = Figure::new(&shape, 0).unwrap();
        let walk = Input {
            forward: 1.0,
            ..Input::default()
        };
        for _ in 0..60 {
            f.step(&shape, &walk, [0.0, 1.0], [1.0, 0.0], 1.0 / 60.0);
        }
        assert_eq!(f.state(), State::Walk);
        let expected = f.ground_speed() / f.sequences().walk_speed;
        assert!((f.thread().time_scale - expected).abs() < 1e-5);
        assert!((f.ground_speed() - f.sequences().walk_speed).abs() < 1e-3);
        assert!(
            f.position[1] > 1.0 && f.position[0].abs() < 1e-4,
            "{:?}",
            f.position
        );
    }
}
