//! A driver for the AIPlayer: a start state and, per tick, the console calls a script made before it; the AIPlayer's own
//! `processTick` (no move given, so `getAIMove` builds one) runs, and the words and the AI's fields come out. A caller can feed it
//! a scripted list of calls and compare each tick with the expected one.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/aiPlayer.cpp` (Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here is this crate's own.

use torque_collide::math::F3;

use super::{AiOp, AiState};
use crate::Move;
use crate::player::{Player, PlayerData, PlayerHost, PlayerWords};

/// What a sampler reads of an AIPlayer after a tick.
#[derive(Debug, Clone, PartialEq)]
pub struct AiTick {
    pub words: PlayerWords,
    /// The position (`getPosition`) after the tick.
    pub position: F3,
    /// The move `getAIMove` built (the Player's last move).
    pub mv: Move,
    pub ai: AiState,
    /// The work budget ran out inside the step.
    pub exhausted: bool,
}

/// Runs one entry of `steps` per tick (the calls made before it, in order) on a copy of `start`. A start state with no [`AiState`]
/// gets a default one.
pub fn run(
    start: &Player,
    db: &PlayerData,
    steps: &[Vec<AiOp>],
    host: &mut dyn PlayerHost,
) -> Vec<AiTick> {
    let mut p = start.clone();
    if p.s.ai.is_none() {
        p.s.ai = Some(Box::default());
    }
    let mut out = Vec::with_capacity(steps.len());
    for ops in steps {
        p.s.mask_bits = 0;
        if let Some(ai) = p.s.ai.as_deref_mut() {
            for op in ops {
                op.apply(ai);
            }
        }
        let exhausted = p.process_tick(db, None, host).is_err();
        out.push(AiTick {
            words: p.packet_words(db, host),
            position: p.s.position(),
            mv: p.s.last_move,
            ai: p.s.ai.as_deref().cloned().unwrap_or_default(),
            exhausted,
        });
    }
    out
}
