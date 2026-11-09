//! A realm's limits: engine settings, never script globals, so no script reads or changes them.

use std::time::Duration;

use torque_client_script_format::abi::Config;

use crate::runner::InstanceLimits;

/// What a Luau step of the realm's guest costs under Wasmtime, measured (`realm-gate`): about 102 ns.
pub const STEP_NANOS: u64 = 102;
/// The steps of a frame's budget: about 2 ms of Luau, the native frame budget's most. Each callback of a frame gets
/// at most these.
pub const FRAME_STEPS: u32 = 20_000;
/// The steps of a file run (a mod's entry and what it `require`s): about 11 ms of Luau, under the 25 ms that is a
/// strike. A server with a bigger setup spreads it over frames (`realm.after`, `realm.onFrame`).
pub const RUN_STEPS: u32 = 100_000;

/// The linear memory a guest has beyond Luau's cap: its 8 MiB shadow stack and its own data.
pub const GUEST_OVERHEAD: u32 = 16 << 20;

/// How much of a frame a realm's work may take.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FrameBudget {
    /// The guest runs on the engine's thread: its entries and the applying of their ops share at most `max`, and at
    /// most `share` of the frame's target time.
    Shared {
        /// The most time a frame, whatever the target.
        max: Duration,
        /// The part of the target frame time, 0 to 1.
        share: f64,
    },
    /// The guest runs in a Worker, off the engine's thread: only applying its ops counts, at most `cap` a frame.
    ApplyOnly {
        /// The most applying time a frame.
        cap: Duration,
    },
}

impl FrameBudget {
    /// The time a frame of `target` gives the realm.
    pub fn of(self, target: Duration) -> Duration {
        match self {
            FrameBudget::Shared { max, share } => max.min(target.mul_f64(share.clamp(0.0, 1.0))),
            FrameBudget::ApplyOnly { cap } => cap,
        }
    }
}

/// When limit stops turn a realm off for its connection: `count` of them within `window`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StrikeRule {
    /// How many stops.
    pub count: usize,
    /// Within how long, in the realm's microseconds.
    pub window_us: u64,
}

/// What a realm runs with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Luau's allocator cap, in bytes; the linear memory cap is this and [`GUEST_OVERHEAD`].
    pub lua_memory: u32,
    /// The steps a file run may take ([`RUN_STEPS`]); a frame's callbacks get at most [`Settings::frame_steps`].
    pub callback_steps: u32,
    /// The steps all callbacks of a guest frame may take before the rest waits.
    pub frame_steps: u32,
    /// How long one entry may run before the instance is stopped.
    pub deadline: Duration,
    /// How long an entry may run before it counts as a strike (a builtin the steps barely see holding the frame).
    pub long_entry: Duration,
    /// How much of a frame the realm's work may take.
    pub budget: FrameBudget,
    /// Over budget in this many of the last [`SLOW_WINDOW`] frames, the realm runs every other frame.
    pub slow_after: u32,
    /// When limit stops turn the realm off.
    pub strikes: StrikeRule,
    /// `math.random`'s seed.
    pub seed: u32,
}

/// The frames the slow rule looks back over.
pub const SLOW_WINDOW: u32 = 60;

impl Settings {
    /// The native defaults: 32 MiB of Lua; [`RUN_STEPS`] a file run and [`FRAME_STEPS`] a frame (a frame's callback
    /// gets at most a frame's budget, so a realm at its caps every frame holds the frame to about 2 ms, and its stops
    /// are strikes); a 250 ms
    /// deadline (the epoch); an entry past 25 ms (12.5 budgets) a strike; `min(2 ms, 12%)` of a frame; slow after 30
    /// of 60 frames; off after 3 strikes in 10 s.
    pub fn native() -> Settings {
        Settings {
            lua_memory: Config::default().memory,
            callback_steps: RUN_STEPS,
            frame_steps: FRAME_STEPS,
            deadline: Duration::from_millis(250),
            long_entry: Duration::from_millis(25),
            budget: FrameBudget::Shared {
                max: Duration::from_millis(2),
                share: 0.12,
            },
            slow_after: 30,
            strikes: StrikeRule {
                count: 3,
                window_us: 10_000_000,
            },
            seed: 0,
        }
    }

    /// The web defaults: as [`Settings::native`] (the same steps, so a script counts the same on both), with a 1 s
    /// deadline (the Worker's watchdog) and 1 ms of applying a frame.
    pub fn web() -> Settings {
        Settings {
            deadline: Duration::from_secs(1),
            budget: FrameBudget::ApplyOnly {
                cap: Duration::from_millis(1),
            },
            ..Settings::native()
        }
    }

    /// The caps of the realm's instance.
    pub fn instance(&self) -> InstanceLimits {
        InstanceLimits {
            memory: self.lua_memory.saturating_add(GUEST_OVERHEAD) as usize,
            deadline: self.deadline,
            long_entry: self.long_entry,
        }
    }

    /// The config record the realm is made with.
    pub(crate) fn config(&self, grants: torque_client_script_format::Grants) -> Config {
        Config {
            grants,
            memory: self.lua_memory.max(Config::MIN_MEMORY),
            callback_steps: self.callback_steps.max(1),
            frame_steps: self.frame_steps.max(1),
            seed: self.seed,
        }
    }
}
