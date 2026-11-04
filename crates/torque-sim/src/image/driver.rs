//! A driver for the weapon images: a datablock table and a list of timed events in, the slot state, the mask bits and the
//! callbacks of every tick out. A caller can feed it scripted events (`mountImage`, `setImageTrigger`, `setImageAmmo`, ...)
//! at their sim times and compare each tick's [`TickRecord`] with the expected states and callbacks.
//!
//! [`ScriptedHost`] is a self-contained [`ImageHost`]: no script VM, so a callback only logs itself and runs the actions its `hooks`
//! name (what a plain `onMount`/`onFire` does to the image: `setImageAmmo`, `setImageTrigger`, ...).
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/shapeImage.cpp` (Copyright (c) 2012
//! GarageGames, LLC); the code here is this crate's own.

use std::collections::HashMap;
use std::sync::Arc;

use super::host::ImageHost;
use super::{DbId, ImageData, ImageSet, SLOTS, machine};

/// What a script does to the image (the natives of this module's console methods).
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Mount {
        db: DbId,
        slot: usize,
        loaded: bool,
        skin: u32,
    },
    Unmount {
        slot: usize,
    },
    Trigger {
        slot: usize,
        down: bool,
    },
    Ammo {
        slot: usize,
        on: bool,
    },
    Loaded {
        slot: usize,
        on: bool,
    },
}

/// Which callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallbackKind {
    /// `stateScript`.
    Script,
    OnMount,
    OnUnmount,
    /// The recoil hook: `name` is empty, `slot` and the recoil value in `arg`.
    Recoil,
}

/// A callback as the host would run it, with the sim time (ms) it ran at.
#[derive(Debug, Clone, PartialEq)]
pub struct LoggedCallback {
    pub time_ms: u32,
    pub kind: CallbackKind,
    pub db: DbId,
    pub name: String,
    pub slot: usize,
    pub arg: i32,
}

/// One slot's state after a tick.
#[derive(Debug, Clone, PartialEq)]
pub struct SlotRecord {
    pub db: Option<DbId>,
    /// `getImageState` (the current state's name).
    pub state: Option<String>,
    pub delay: f32,
    pub loaded: bool,
    pub ammo: bool,
    pub target: bool,
    pub trigger: bool,
    pub wet: bool,
    pub fire_count: u32,
}

/// One tick's outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct TickRecord {
    pub tick: u32,
    pub time_ms: u32,
    pub slots: [SlotRecord; SLOTS],
    /// The mask bits raised during the tick.
    pub mask: u32,
    /// The callbacks that ran during the tick, in order.
    pub callbacks: Vec<LoggedCallback>,
}

/// An event applied at the start of a tick (a script's call from outside the image step).
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Do(Action),
    /// The Player's slot-0 wet flag flips (`ShapeBase::processTick`, before the image loop).
    Wet(bool),
    /// The owner's energy level.
    Energy(f32),
}

/// A host with no VM.
#[derive(Debug, Clone, Default)]
pub struct ScriptedHost {
    pub images: ImageSet,
    pub data: HashMap<DbId, Arc<ImageData>>,
    pub energy: f32,
    pub mask: u32,
    pub log: Vec<LoggedCallback>,
    pub time_ms: u32,
    /// `(datablock, callback name)` to what its script does; `"onMount"`/`"onUnmount"` name those callbacks.
    pub hooks: HashMap<(DbId, String), Vec<Action>>,
    pub light_calls: Vec<bool>,
    /// How many times a fire-state pass animated the owner's shape ([`ImageHost::fire_animate`]).
    pub fire_animations: u32,
}

impl ScriptedHost {
    pub fn new() -> ScriptedHost {
        ScriptedHost::default()
    }

    pub fn add(&mut self, data: ImageData) {
        self.data.insert(data.id, Arc::new(data));
    }

    pub fn hook(&mut self, db: DbId, name: &str, actions: Vec<Action>) {
        self.hooks.insert((db, name.to_string()), actions);
    }

    /// Runs one action as a script native would.
    pub fn perform(&mut self, a: &Action) {
        match *a {
            Action::Mount {
                db,
                slot,
                loaded,
                skin,
            } => {
                machine::mount_image(self, db, slot, loaded, skin);
            }
            Action::Unmount { slot } => {
                machine::unmount_image(self, slot);
            }
            Action::Trigger { slot, down } => {
                machine::set_image_trigger(self, slot, down);
            }
            Action::Ammo { slot, on } => machine::set_image_ammo(self, slot, on),
            Action::Loaded { slot, on } => machine::set_image_loaded(self, slot, on),
        }
    }

    fn callback(&mut self, kind: CallbackKind, db: DbId, name: &str, slot: usize, arg: i32) {
        self.log.push(LoggedCallback {
            time_ms: self.time_ms,
            kind,
            db,
            name: name.to_string(),
            slot,
            arg,
        });
        if let Some(actions) = self.hooks.get(&(db, name.to_string())).cloned() {
            for a in &actions {
                self.perform(a);
            }
        }
    }

    fn snapshot(&self) -> [SlotRecord; SLOTS] {
        std::array::from_fn(|i| {
            let s = &self.images.slots[i];
            SlotRecord {
                db: s.db,
                state: s
                    .db
                    .and_then(|d| self.data.get(&d))
                    .and_then(|d| d.states.get(s.state))
                    .map(|st| st.name.clone()),
                delay: s.delay,
                loaded: s.loaded,
                ammo: s.ammo,
                target: s.target,
                trigger: s.trigger_down,
                wet: s.wet,
                fire_count: s.fire_count,
            }
        })
    }
}

impl ImageHost for ScriptedHost {
    fn images(&mut self) -> &mut ImageSet {
        &mut self.images
    }
    fn image_data(&mut self, db: DbId) -> Option<Arc<ImageData>> {
        self.data.get(&db).cloned()
    }
    fn raise_mask(&mut self, bits: u32) {
        self.mask |= bits;
    }
    fn energy(&mut self) -> f32 {
        self.energy
    }
    fn set_energy(&mut self, energy: f32) {
        self.energy = energy;
    }
    fn state_script(&mut self, db: DbId, script: &str, slot: usize) {
        self.callback(CallbackKind::Script, db, script, slot, 0);
    }
    fn on_mount(&mut self, db: DbId, slot: usize) {
        self.callback(CallbackKind::OnMount, db, "onMount", slot, 0);
    }
    fn on_unmount(&mut self, db: DbId, slot: usize) {
        self.callback(CallbackKind::OnUnmount, db, "onUnmount", slot, 0);
    }
    fn on_recoil(&mut self, slot: usize, recoil: i32) {
        self.callback(CallbackKind::Recoil, 0, "", slot, recoil);
    }
    fn light_registry(&mut self, add: bool) {
        self.light_calls.push(add);
    }
    fn fire_animate(&mut self) {
        self.fire_animations += 1;
    }
}

/// Runs `ticks` ticks of 32 ms from `start_ms`: the `events` of tick `t` (`(t, event)`, in list order) are applied first, then the
/// four slots are stepped with [`IMAGE_DT`](super::IMAGE_DT); each tick's state, mask bits and callbacks are returned.
pub fn run(
    host: &mut ScriptedHost,
    events: &[(u32, Event)],
    ticks: u32,
    start_ms: u32,
) -> Vec<TickRecord> {
    let mut out = Vec::with_capacity(ticks as usize);
    for t in 0..ticks {
        host.time_ms = start_ms.wrapping_add(t.wrapping_mul(32));
        host.mask = 0;
        host.log.clear();
        for (when, ev) in events {
            if *when != t {
                continue;
            }
            match ev {
                Event::Do(a) => host.perform(a),
                Event::Wet(w) => host.images.load_wet0(*w),
                Event::Energy(e) => host.energy = *e,
            }
        }
        machine::tick_images(host);
        out.push(TickRecord {
            tick: t,
            time_ms: host.time_ms,
            slots: host.snapshot(),
            mask: host.mask,
            callbacks: host.log.clone(),
        });
    }
    out
}
