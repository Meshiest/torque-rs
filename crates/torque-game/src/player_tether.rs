//! The tethers plugins put on Players ([`::sim::player::tether`]): a grapple's pull, applied inside the Player's own step on the
//! server and in a client's prediction alike.
//!
//! A plugin adds a [`TetherSource`] to a script engine ([`add`]) on each side: the server's step asks it with the Player's object
//! id, a client's prediction with the ghost index of the Player it predicts. An engine with no source has nothing installed, and
//! the Player's step is Torque's. Each side keeps its own sources ([`of`]), so on a script engine a client shares with its
//! server the server's step never asks a client's source with an object id, nor the prediction a server's with a ghost index.

use std::rc::Rc;

pub use ::sim::player::tether::{MAX_DAMPING, MAX_LENGTH, MAX_STIFFNESS, Tether, apply};
use collide::math::F3;
pub use collide::source::Side;
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

/// The sources of each side of a script engine.
#[derive(Clone, Default)]
struct BySide {
    server: Option<Rc<TetherSources>>,
    client: Option<Rc<TetherSources>>,
}

impl BySide {
    fn side_mut(&mut self, side: Side) -> &mut Option<Rc<TetherSources>> {
        match side {
            Side::Server => &mut self.server,
            Side::Client => &mut self.client,
        }
    }
}

/// Adds `source` to `vm`'s tether sources of `side` (the server's step, or a client's prediction).
pub fn add(vm: &mut Vm, side: Side, source: Rc<dyn TetherSource>) {
    let mut sides = vm
        .extension::<BySide>()
        .map_or_else(BySide::default, |s| (*s).clone());
    let slot = sides.side_mut(side);
    let mut sources = slot.as_deref().cloned().unwrap_or_default();
    sources.0.push(source);
    *slot = Some(Rc::new(sources));
    vm.set_extension(Rc::new(sides));
}

/// The sources `vm` was given for `side`, if any.
pub fn of(vm: &Vm, side: Side) -> Option<Rc<TetherSources>> {
    let mut sides = (*vm.extension::<BySide>()?).clone();
    sides.side_mut(side).take()
}
