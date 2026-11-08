//! Map logic: the entity input/output system of Hammer-made maps, run on the server.
//!
//! A map's entities name each other (`targetname`) and wire outputs to inputs: when a button's `OnPressed` fires, a door gets
//! `Open` half a second later. [`MapLogic`] holds a map's entities (the format-neutral [`torque_mapcore::scene::Entity`];
//! [`connection::entity_from_pairs`] makes them from an entity text's keyvalues), resolves target names, keeps the delayed
//! events in one deterministic queue and runs the supported classes ([`entities`]): the logic entities, triggers and filters as
//! server-only records, movers as the plugin ghost class `MapMover`, `ambient_generic` as sound events.
//!
//! ```
//! use torque_map_logic::connection::entity_from_pairs;
//! use torque_map_logic::defaults::Defaults;
//! use torque_map_logic::{BrushVolumes, MapLogic};
//!
//! let kv = |list: &[(&str, &str)]| {
//!     entity_from_pairs(&list.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>())
//! };
//! let map = [
//!     kv(&[("classname", "logic_auto"), ("OnMapSpawn", "count,Add,5,0.1,-1")]),
//!     kv(&[("classname", "math_counter"), ("targetname", "count"), ("max", "10")]),
//! ];
//! let mut logic = MapLogic::new(&map, &BrushVolumes::new(), &mut Defaults::coded());
//! logic.run(10);
//! let id = logic.find("count")[0];
//! assert!(matches!(logic.kind(id), Some(torque_map_logic::entities::Kind::Counter(c)) if c.value == 5.0));
//! ```
//!
//! The bus's rules (names, wildcards, the special names, time, order, caps) are in [`bus`]; the classes' in [`logic`],
//! [`trigger`], [`mover`] and [`sound`]. Entity definitions can come from a user's installed FGD files ([`fgd`], optional, read at
//! run time, never shipped) over the coded defaults ([`defaults`]).
//!
//! [`MapLogicPlugin`] (`torque.map-logic`) adds the mover ghost class and the server's console functions. It is not in
//! `DefaultPlugins`: a game adds it.

pub mod bus;
pub mod connection;
pub mod defaults;
pub mod entities;
pub mod fgd;
pub mod logic;
pub mod mover;
pub mod plugin;
pub mod sound;
pub mod trigger;
pub mod value;
pub mod volume;

pub use bus::{
    BrushVolumes, Ctx, Delivered, Ent, EntityId, Fired, Limits, MapLogic, Start, Stats, TICK_US,
    Toucher, ToucherKind,
};
pub use mover::MoverGhost;
pub use plugin::{MapLogicPlugin, SharedLogic, install};
pub use sound::{SoundAction, SoundEvent};
pub use volume::{Pose, Volume};

use entities::Kind;
use torque_collide::sat::{Penetration, Polytope, penetration};

impl MapLogic {
    /// The movers' ghost records, in spawn order: what the `MapMover` class sends of each.
    pub fn mover_ghosts(&self) -> Vec<(EntityId, MoverGhost)> {
        self.entities()
            .filter(|(_, e)| e.alive)
            .filter_map(|(id, e)| {
                let m = e.kind.as_mover()?;
                Some((id, m.ghost(e.model.unwrap_or(0))))
            })
            .collect()
    }

    /// The first solid mover the box `min`..`max` overlaps and how far the box must move to clear it (the separating axis
    /// test; a host's player movement collides with movers through this).
    pub fn mover_hit(&self, min: [f32; 3], max: [f32; 3]) -> Option<(EntityId, Penetration)> {
        let mut boxp = Polytope::default();
        let b = Volume::of_box(min, max);
        for &p in &b.points {
            boxp.add_point(p);
        }
        for pl in &b.planes {
            boxp.add_normal([pl[0], pl[1], pl[2]]);
        }
        for e in &b.edges {
            boxp.add_edge(e[0], e[1]);
        }
        for (id, e) in self.entities() {
            let Some(m) = e.kind.as_mover().filter(|m| m.solid && e.alive) else {
                continue;
            };
            for v in m.volumes_now() {
                let (lo, hi) = v.bounds();
                if (0..3).any(|k| !(lo[k] < max[k] && min[k] < hi[k])) {
                    continue;
                }
                let mut out = Penetration::default();
                if penetration(&boxp, &v.polytope(), &mut out) {
                    return Some((id, out));
                }
            }
        }
        None
    }

    /// The looped sounds playing now (for a client that joins late).
    pub fn playing_sounds(&self) -> Vec<SoundEvent> {
        self.entities()
            .filter(|(_, e)| e.alive)
            .filter_map(|(id, e)| match &e.kind {
                Kind::Ambient(a) if a.playing => Some(a.event(self, id, SoundAction::Play)),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod fgd_tests;
#[cfg(test)]
mod fuzz_tests;
#[cfg(test)]
mod tests;
