//! The example's files: the asset kit (made in memory), the examples' shared scripts and the game folder (`game/`: the
//! scripts, and the maps in `game/missions/`), read from disk once. The server and every client read them from memory.

use torque_example_common::GameDir;

/// The arena's game folder.
#[derive(Debug, Clone, Copy)]
pub struct Arena;

impl GameDir for Arena {
    const DIR: &'static str = concat!(env!("CARGO_MANIFEST_DIR"), "/game");
}

/// The kit, the shared scripts and the arena's game folder.
pub type GameFiles = torque_example_common::GameFiles<Arena>;
