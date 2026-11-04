//! A driver for the Player: a start state and a list of moves in, the words of every tick out. A caller can run a scripted move
//! list through it and compare each tick's [`PlayerWords`] (and the host's checksum over them) with the expected ones.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/player.cpp` (Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here is this crate's own.

use super::{Player, PlayerData, PlayerHost, PlayerWords};
use crate::Move;

/// One tick's outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayTick {
    pub words: PlayerWords,
    /// The work budget ran out inside the step (the state is as far as it got).
    pub exhausted: bool,
}

/// Runs `moves` (`None`: a tick with no move, as the process list runs an object that is nobody's control object) from a copy of
/// `start`, and yields the words after each tick. The host is the world the moves ran in.
pub fn run(
    start: &Player,
    db: &PlayerData,
    moves: &[Option<Move>],
    host: &mut dyn PlayerHost,
) -> Vec<ReplayTick> {
    let mut p = start.clone();
    let mut out = Vec::with_capacity(moves.len());
    for mv in moves {
        p.s.mask_bits = 0;
        let exhausted = p.process_tick(db, mv.as_ref(), host).is_err();
        out.push(ReplayTick {
            words: p.packet_words(db, host),
            exhausted,
        });
    }
    out
}
