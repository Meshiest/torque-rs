//! A client's world follows at most `MAX_WORLD_GHOSTS` ghosts, whatever ghost table a server's accept allows.

use net::ghost::GhostChange;
use torque_ghost_world::ClientWorld;
use torque_ghost_world::set::{ClientSet, SentGhost, SentTable};
use torque_ghost_world::tge::TgeClientSet;
use torque_ghost_world::world::MAX_WORLD_GHOSTS;

/// A ghost with no class or record.
struct Bare;

impl SentGhost<TgeClientSet> for Bare {
    fn class(&self) -> Option<<TgeClientSet as ClientSet>::Class> {
        None
    }

    fn serial(&self) -> usize {
        1
    }

    fn record(&self) -> Option<&<TgeClientSet as ClientSet>::Record> {
        None
    }

    fn updates(&self) -> u32 {
        0
    }
}

/// A table that has a ghost at every index.
struct Every(Bare);

impl SentTable<TgeClientSet> for Every {
    type Ghost = Bare;

    fn ghost(&self, _index: u32) -> Option<&Bare> {
        Some(&self.0)
    }
}

#[test]
fn a_24_bit_ghost_table_is_followed_up_to_the_worlds_cap() {
    let mut world = ClientWorld::<TgeClientSet>::default();
    let table = Every(Bare);
    let n = MAX_WORLD_GHOSTS as u32 + 100;
    // Indices spread over a 24-bit table.
    let changes: Vec<GhostChange> = (0..n)
        .map(|i| GhostChange::Created(i * 61 % (1 << 24)))
        .collect();
    world.apply_ghosts(1, &changes, &table, [0.0; 3], &mut ());
    assert_eq!(world.ghosts.len(), MAX_WORLD_GHOSTS);
    assert_eq!(world.over_cap, 100);
    // A ghost already followed is made again under its index.
    world.apply_ghosts(2, &[GhostChange::Created(0)], &table, [0.0; 3], &mut ());
    assert_eq!(world.over_cap, 100);
    // Room made: the next one is followed.
    let next = [GhostChange::Deleted(0), GhostChange::Created(1 << 23)];
    world.apply_ghosts(3, &next, &table, [0.0; 3], &mut ());
    assert!(world.ghosts.contains_key(&(1 << 23)));
}
