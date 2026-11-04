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
//! **Ground speed.** Walk and run carry ground frames: their sequence moves the figure a known distance a second
//! ([`ground_speed`]). The thread's time scale is the figure's speed over that, as a Player scales its action thread by its
//! velocity over the action's speed, so the feet keep pace with the floor at any speed.

use torque::shapes::instance::{ShapeInstance, Thread, ThreadId, find_sequence};
use torque::shapes::shape::Shape;
use torque_render_scene::math::Mat;

/// Seconds a change of state blends over (a Player's action transition time).
pub const TRANSITION: f32 = 0.25;
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
    /// The thread playing `hold` while the figure holds a pistol.
    holding: Option<ThreadId>,
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
            holding: None,
        };
        figure.animate(shape);
        Some(figure)
    }

    /// Whether the figure holds a pistol (its right arm in the `hold` pose).
    pub fn holding(&self) -> bool {
        self.holding.is_some()
    }

    /// Takes the `hold` pose (a second thread over the state's, the right arm raised) or leaves it; nothing when the shape
    /// has no `hold` sequence.
    pub fn set_holding(&mut self, shape: &Shape, on: bool) {
        match (on, self.holding, self.sequences.hold) {
            (true, None, Some(seq)) => {
                if let Some(t) = self.instance.create_thread(shape) {
                    self.instance.set_sequence(shape, t, seq as i32, 0.0);
                    self.holding = Some(t);
                }
            }
            (false, Some(t), _) => {
                self.instance.destroy_thread(shape, t);
                self.holding = None;
            }
            _ => return,
        }
        self.instance.set_all_dirty();
        self.animate(shape);
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

    fn animate(&mut self, shape: &Shape) {
        // The figure's own shape: nothing to run out of.
        let _ = self
            .instance
            .animate(shape, self.detail, None, &mut |_| true);
    }
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
