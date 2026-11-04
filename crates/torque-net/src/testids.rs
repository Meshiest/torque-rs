//! Event-class ids and a Huffman table for the unit tests: Torque's event classes and four test classes registered beside them, as
//! a game registers its own (17 classes, so a class id is 5 bits wide). The ids come out of the registry as they do in a running
//! server. The Huffman frequencies are the tests' made-up ones.

use crate::classes::Direction;
use crate::events::{EventCtx, EventIds, EventRegistry};
use crate::huffman::Table;
use crate::profile::DEFAULT_DATABLOCK_BITS;

/// The test classes. Their ids follow Torque's 13: `TestAnyEvent` 13, `TestToClientEvent` 14, `TestToServerEvent` 15,
/// `TestUnorderedEvent` 16.
pub const TEST_EVENTS: [(&str, Direction); 4] = [
    ("TestAnyEvent", Direction::Any),
    ("TestToClientEvent", Direction::ServerToClient),
    ("TestToServerEvent", Direction::ClientToServer),
    ("TestUnorderedEvent", Direction::Any),
];

/// The id of `TestAnyEvent` (direction any).
pub const ANY: u32 = 13;
/// The id of `TestToClientEvent` (server to client).
pub const TO_CLIENT: u32 = 14;
/// The id of `TestToServerEvent` (client to server).
pub const TO_SERVER: u32 = 15;

pub fn registry() -> EventRegistry {
    let mut r = EventRegistry::torque();
    for (name, direction) in TEST_EVENTS {
        r.register(name, direction);
    }
    r
}

pub fn ids() -> EventIds {
    static IDS: std::sync::OnceLock<EventIds> = std::sync::OnceLock::new();
    IDS.get_or_init(build_ids).clone()
}

fn build_ids() -> EventIds {
    registry().ids().expect("the test registry derives")
}

pub fn table() -> Table {
    shared_table().clone()
}

fn shared_table() -> &'static Table {
    static TABLE: std::sync::OnceLock<Table> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        Table::from_frequencies(&crate::huffman::tests::sample_frequencies()).expect("a table")
    })
}

/// Runs `f` with a context over the test table and ids (ghost width 18, as Torque3D's ghost limit gives; TGE 1.4's datablock width).
pub fn with_ctx<R>(f: impl FnOnce(&EventCtx<'_>) -> R) -> R {
    static IDS: std::sync::OnceLock<EventIds> = std::sync::OnceLock::new();
    let ids = IDS.get_or_init(build_ids);
    f(&EventCtx {
        table: shared_table(),
        ids,
        ghost_bits: 18,
        datablock_bits: DEFAULT_DATABLOCK_BITS,
    })
}
