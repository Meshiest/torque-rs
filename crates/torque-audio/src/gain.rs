//! The volume curve: how a linear volume (a channel's, a description's, `AL_GAIN_LINEAR`) becomes OpenAL's `AL_GAIN`, and back.
//!
//! **By default there is no curve**: the volume is the gain, clamped to 0..1 (Torque3D's `SFXALVoice::setVolume` hands the
//! volume to `AL_GAIN` unchanged; a NaN is 0 here).
//!
//! **A gain table** ([`GainTable::from_gains`]): TGE 1.4 sends every volume through a table of [`GAIN_ENTRIES`] gains
//! (`Audio::linearToDB` and `DBToLinear`). A host may give such a table; the lookups are TGE's:
//!
//! - Linear to gain: `v <= 0` is 0, `v >= 1` is 1, else `table[v * 128]`, the index cut toward zero.
//! - Gain to linear: `g <= 0` is 0, `g >= 1` is 1, else the index a binary search over the table stops at (mid 64 between 0 and 128;
//!   a table entry equal to `g` stops it; below `g` moves the low end up, else the high end down) times 1/128.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `sfx/openal/sfxALVoice.cpp` (Copyright (c)
//! 2012 GarageGames, LLC); the code here is this crate's own.

/// The entries of a gain table (TGE's `logmax`).
pub const GAIN_ENTRIES: usize = 128;

/// The volume curve: none (the volume is the gain) or a table of [`GAIN_ENTRIES`] gains.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GainTable {
    gains: Option<[f32; GAIN_ENTRIES]>,
}

impl GainTable {
    /// No curve: the volume is the gain (Torque3D's).
    pub fn linear() -> GainTable {
        GainTable { gains: None }
    }

    /// A table of gains, looked up as TGE looks up its own.
    pub fn from_gains(gains: [f32; GAIN_ENTRIES]) -> GainTable {
        GainTable { gains: Some(gains) }
    }

    /// Whether this curve is a table (else the volume is the gain).
    pub fn is_table(&self) -> bool {
        self.gains.is_some()
    }

    /// Linear volume to `AL_GAIN`.
    pub fn to_gain(&self, v: f32) -> f32 {
        if v <= 0.0 {
            return 0.0;
        }
        if v >= 1.0 {
            return 1.0;
        }
        let Some(gains) = &self.gains else {
            return if v.is_nan() { 0.0 } else { v };
        };
        // The index cut toward zero; a NaN is no index.
        let i = (v * GAIN_ENTRIES as f32).trunc();
        if (0.0..GAIN_ENTRIES as f32).contains(&i) {
            gains[i as usize]
        } else {
            0.0
        }
    }

    /// `AL_GAIN` to linear volume.
    pub fn to_linear(&self, g: f32) -> f32 {
        if g <= 0.0 {
            return 0.0;
        }
        if g >= 1.0 {
            return 1.0;
        }
        let Some(gains) = &self.gains else {
            return if g.is_nan() { 0.0 } else { g };
        };
        let (mut lo, mut hi) = (0usize, GAIN_ENTRIES);
        let mut mid = hi / 2;
        loop {
            let t = gains[mid];
            if t == g {
                break;
            }
            if t < g {
                lo = mid;
            } else {
                hi = mid;
            }
            let next = lo + (hi - lo) / 2;
            if next == mid {
                break;
            }
            mid = next;
        }
        (mid as f64 * (1.0 / GAIN_ENTRIES as f64)) as f32
    }
}

#[cfg(test)]
#[path = "gain_tests.rs"]
mod tests;
