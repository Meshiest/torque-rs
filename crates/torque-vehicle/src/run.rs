//! Running a vehicle over a sequence of moves: a start state and the moves in, each tick's packet words out. A tick with no
//! move is `None` (the process list's `processTick(NULL)`, or a disabled driver's null move). Each tick's [`VehicleWords`] can
//! be compared by the caller.

use crate::Move;
use crate::data::{FlyingVehicleData, WheeledVehicleData};
use crate::flying::{self, Flying};
use crate::host::VehicleHost;
use crate::tick::{Vehicle, process_tick};
use crate::wheeled::{self, Wheeled};
use crate::words::VehicleWords;

/// One tick's outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct TickOutcome {
    pub words: VehicleWords,
    /// The mask bits the tick set.
    pub mask: u32,
    /// The work budget ran out inside the tick (the state is as far as it got).
    pub exhausted: bool,
}

/// Runs `moves` on a copy of a WheeledVehicle.
pub fn run_wheeled(
    start: &Vehicle<Wheeled>,
    db: &WheeledVehicleData,
    moves: &[Option<Move>],
    host: &mut dyn VehicleHost,
) -> Vec<TickOutcome> {
    let mut v = start.clone();
    let mut out = Vec::with_capacity(moves.len());
    for mv in moves {
        v.s.mask_bits = 0;
        let exhausted = process_tick(&mut v, db, mv.as_ref(), host).is_err();
        out.push(TickOutcome {
            words: wheeled::packet_words(&v, db, host),
            mask: v.s.mask_bits,
            exhausted,
        });
    }
    out
}

/// Runs `moves` on a copy of a FlyingVehicle.
pub fn run_flying(
    start: &Vehicle<Flying>,
    db: &FlyingVehicleData,
    moves: &[Option<Move>],
    host: &mut dyn VehicleHost,
) -> Vec<TickOutcome> {
    let mut v = start.clone();
    let mut out = Vec::with_capacity(moves.len());
    for mv in moves {
        v.s.mask_bits = 0;
        let exhausted = process_tick(&mut v, db, mv.as_ref(), host).is_err();
        out.push(TickOutcome {
            words: flying::packet_words(&v, db, host),
            mask: v.s.mask_bits,
            exhausted,
        });
    }
    out
}
