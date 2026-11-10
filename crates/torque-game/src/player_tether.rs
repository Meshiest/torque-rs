//! The tethers plugins put on Players ([`::sim::player::tether`]): a grapple's pull, applied inside the Player's own step on the
//! server and in a client's prediction alike.
//!
//! A plugin adds a [`TetherSource`] to a script engine ([`add`]) on each side: the server's step asks it with the Player's object
//! id, a client's prediction with the ghost index of the Player it predicts. An engine with no source has nothing installed, and
//! the Player's step is Torque's.

use std::rc::Rc;

pub use ::sim::player::tether::{MAX_DAMPING, MAX_LENGTH, MAX_STIFFNESS, Tether, apply};
use collide::math::F3;
use ts_vm::vm::Vm;

/// A plugin's tethers on Players.
pub trait TetherSource {
    /// The tether on the Player `key` (the server's object id, or a client's ghost index of it) this tick, if any.
    fn tether(&self, key: u32) -> Option<Tether>;
    /// The tether changed the Player `key`'s velocity by `change`, the Player being `mass` heavy (the server's step only; the
    /// source gives the thing at the other end the opposite impulse).
    fn pulled(&self, _key: u32, _change: F3, _mass: f32) {}
    /// The tether changed the Player `key`'s velocity by `change` in a client's prediction of its move `index` (a replay of the
    /// move tells it again, with what the replay made): for a source that shows the pull on the thing at the other end ahead
    /// of the server. Nothing by default; it changes nothing of the Player's step.
    fn predicted_pull(&self, _key: u32, _index: u32, _change: F3, _mass: f32) {}
}

/// The sources a script engine was given, asked in the order they were added: the first tether wins.
#[derive(Clone, Default)]
pub struct TetherSources(Vec<Rc<dyn TetherSource>>);

impl TetherSources {
    /// The first source's tether on the Player `key`, held to the tether's ranges (a source's non-finite tether is none).
    pub fn tether(&self, key: u32) -> Option<(usize, Tether)> {
        self.0
            .iter()
            .enumerate()
            .find_map(|(i, s)| Some((i, s.tether(key)?.held()?)))
    }

    /// Tells source `index` what its tether did.
    pub fn pulled(&self, index: usize, key: u32, change: F3, mass: f32) {
        if let Some(s) = self.0.get(index) {
            s.pulled(key, change, mass);
        }
    }

    /// Tells source `index` what its tether did in a client's prediction of move `move_index`.
    pub fn predicted_pull(&self, index: usize, key: u32, move_index: u32, change: F3, mass: f32) {
        if let Some(s) = self.0.get(index) {
            s.predicted_pull(key, move_index, change, mass);
        }
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Adds `source` to `vm`'s tether sources.
pub fn add(vm: &mut Vm, source: Rc<dyn TetherSource>) {
    let mut sources = of(vm).map_or_else(TetherSources::default, |s| (*s).clone());
    sources.0.push(source);
    vm.set_extension(Rc::new(sources));
}

/// The sources `vm` was given, if any.
pub fn of(vm: &Vm) -> Option<Rc<TetherSources>> {
    vm.extension::<TetherSources>()
}
