//! The script threads (`playThread`, `stopThread`, `pauseThread`) a ShapeBase ghost carries, as TGE's client keeps them.
//!
//! **What travels** (`ShapeBase::unpackUpdate`'s thread block): per slot the sequence (`ThreadSequenceBits`, 6), the state (2 bits:
//! 0 play, 1 stop, 2 pause), `forward` and `atEnd`. The client writes the state and the two flags into the slot; then a new
//! sequence goes through `setThreadSequence(slot, seq, reset = false)` (which takes only a sequence below `MaxSequenceIndex`, 63, and
//! puts the thread on it at position 0) and the slot's own sequence through `updateThread`. `updateThread` runs the state: stop puts
//! the thread back to position 0 and holds it, pause holds it where it is, play holds it at its end (`atEnd`: the end forward, the
//! start backward) or runs it at speed 1 (-1 backwards) from where it is.
//!
//! A game whose client applies the block another way installs its own rule ([`super::set::ClientSet::apply_threads`]).
//!
//! The renderer poses a thread from [`ScriptThread::position`] (or, for a thread that runs, [`ScriptThread::seconds`] at
//! [`ScriptThread::time_scale`] from position 0).
//!
//! Credit: the rules are TGE's, as Torque3D (MIT) carries them in `T3D/shapeBase.cpp` (Copyright (c) 2012 GarageGames, LLC;
//! Copyright (C) 2015 Faust Logic, Inc.).

/// A script thread slot as an update sent it: the sequence, the state (0 play, 1 stop, 2 pause), `forward` and `atEnd`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Thread {
    pub sequence: u32,
    pub state: u32,
    pub forward: bool,
    pub at_end: bool,
}

/// `ShapeBase::MaxScriptThreads`.
pub const THREAD_SLOTS: usize = 4;
/// `MaxSequenceIndex`: `setThreadSequence` takes only a sequence below it.
pub const MAX_SEQUENCE_INDEX: u32 = 63;

/// A script thread's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadState {
    Play,
    /// Held at position 0.
    Stop,
    /// Held where it was.
    Pause,
}

/// One script thread on the client.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScriptThread {
    pub sequence: u32,
    pub state: ThreadState,
    pub forward: bool,
    /// `atEnd`: a playing thread held at its end (forward) or its start (backward).
    pub at_end: bool,
    /// The world clock (ms) the thread has run (or been held) from since its last change.
    pub started_ms: Option<u32>,
    /// Where it was then: seconds into its sequence, unwrapped (a backward thread goes below 0).
    pub from: f32,
}

impl ScriptThread {
    /// A thread put on `sequence` at position 0, playing forward from the world clock `now`.
    pub fn new(sequence: u32, now: Option<u32>) -> ScriptThread {
        ScriptThread {
            sequence,
            state: ThreadState::Play,
            forward: true,
            at_end: false,
            started_ms: now,
            from: 0.0,
        }
    }

    /// The speed it runs at (`updateThread`: 1, -1 backwards; 0 held).
    pub fn time_scale(&self) -> f32 {
        match self.state {
            ThreadState::Play if self.at_end => 0.0,
            ThreadState::Play if self.forward => 1.0,
            ThreadState::Play => -1.0,
            ThreadState::Stop | ThreadState::Pause => 0.0,
        }
    }

    /// For a thread that runs: the seconds it has run at [`ScriptThread::time_scale`] from position 0 at the world clock `now` (0
    /// without a clock, and for a held thread).
    pub fn seconds(&self, now: Option<u32>) -> f32 {
        let scale = self.time_scale();
        if scale == 0.0 {
            return 0.0;
        }
        self.from * scale + self.elapsed(now)
    }

    /// Where the thread is at the world clock `now`: seconds into its sequence, unwrapped (a held thread stays where it was; one
    /// held [`ScriptThread::at_end`] is at its end or start, which the shape's duration places).
    pub fn position(&self, now: Option<u32>) -> f32 {
        self.from + self.elapsed(now) * self.time_scale()
    }

    fn elapsed(&self, now: Option<u32>) -> f32 {
        match (self.started_ms, now) {
            (Some(s), Some(n)) => n.saturating_sub(s) as f32 / 1000.0,
            _ => 0.0,
        }
    }
}

/// A ghost's four slots.
pub type Slots = [Option<ScriptThread>; THREAD_SLOTS];

/// The thread block of one record applied to a ghost's slots at the world clock `now`, by TGE's rule (see the module docs). A
/// state of 3 (none of TGE's) changes nothing. The world loads no shapes, so a thread held at its end that runs on again runs
/// from its start.
pub fn apply(slots: &mut Slots, sent: &[Option<Thread>; THREAD_SLOTS], now: Option<u32>) {
    for (slot, sent) in slots.iter_mut().zip(sent) {
        let Some(t) = sent else {
            continue;
        };
        let state = match t.state & 3 {
            0 => ThreadState::Play,
            1 => ThreadState::Stop,
            2 => ThreadState::Pause,
            _ => continue,
        };
        let from = match slot {
            Some(k) if k.sequence == t.sequence && !k.at_end => k.position(now),
            Some(k) if k.sequence == t.sequence => 0.0,
            _ if t.sequence >= MAX_SEQUENCE_INDEX => continue,
            _ => 0.0,
        };
        *slot = Some(ScriptThread {
            sequence: t.sequence,
            state,
            forward: t.forward,
            at_end: t.at_end,
            started_ms: now,
            from: if state == ThreadState::Stop {
                0.0
            } else {
                from
            },
        });
    }
}

#[cfg(test)]
#[path = "threads_tests.rs"]
mod tests;
