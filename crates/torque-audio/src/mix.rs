//! The mixer: the playing voices into interleaved `f32` frames at the output's rate (the device's callback; tests). The play state
//! they move is the same as [`Audio::advance`]'s.
//!
//! Each voice: its buffer resampled linearly by `rate * pitch / output rate`; its gain `AL_GAIN` times the listener's gain times
//! OpenAL 1.0's inverse distance attenuation (`ref / (ref + (d - ref))`, `d` clamped to the reference and the maximum distance, rolloff
//! 1, as `alDistanceModel(AL_INVERSE_DISTANCE)` gives); a positioned voice is panned by the side its direction from the listener lies
//! on (equal power); a relative one (2D) sits in the middle.

use crate::system::{Audio, PlayState};

/// One sample of a buffer (`frame`, `channel`) in -1..1.
fn sample(data: &[u8], bits: u16, channels: u16, frame: usize, channel: usize) -> f32 {
    let ch = usize::from(channels.max(1));
    let c = channel.min(ch - 1);
    if bits == 8 {
        data.get(frame * ch + c)
            .map_or(0.0, |b| (f32::from(*b) - 128.0) / 128.0)
    } else {
        let at = (frame * ch + c) * 2;
        match data.get(at..at + 2) {
            Some(b) => f32::from(i16::from_le_bytes([b[0], b[1]])) / 32_768.0,
            None => 0.0,
        }
    }
}

fn attenuation(reference: f32, max: f32, distance: f32) -> f32 {
    if reference.is_nan() || reference <= 0.0 {
        return 1.0;
    }
    let d = distance.max(reference).min(max.max(reference));
    reference / (reference + (d - reference))
}

impl Audio {
    /// Mixes `out.len() / channels` frames (adding to `out`) and moves the voices on.
    pub fn render(&mut self, out: &mut [f32], channels: usize, rate: u32) {
        if channels == 0 || rate == 0 {
            return;
        }
        let frames = out.len() / channels;
        let listener = self.listener;
        let at = [
            listener.orientation[0],
            listener.orientation[1],
            listener.orientation[2],
        ];
        let up = [
            listener.orientation[3],
            listener.orientation[4],
            listener.orientation[5],
        ];
        let right = [
            at[1] * up[2] - at[2] * up[1],
            at[2] * up[0] - at[0] * up[2],
            at[0] * up[1] - at[1] * up[0],
        ];
        for s in &mut self.sources {
            let v = &mut s.voice;
            if v.state != PlayState::Playing {
                continue;
            }
            let Some(b) = v.buffer.clone() else {
                v.state = PlayState::Stopped;
                continue;
            };
            let total = b.pcm.frames();
            if total == 0 {
                v.state = PlayState::Stopped;
                continue;
            }
            let rel = if v.relative {
                v.position
            } else {
                [
                    v.position[0] - listener.position[0],
                    v.position[1] - listener.position[1],
                    v.position[2] - listener.position[2],
                ]
            };
            let d = (rel[0] * rel[0] + rel[1] * rel[1] + rel[2] * rel[2]).sqrt();
            let gain =
                v.gain * listener.gain * attenuation(v.reference_distance, v.max_distance, d);
            let pan = if v.relative || d <= 0.0 {
                0.0
            } else {
                ((rel[0] * right[0] + rel[1] * right[1] + rel[2] * right[2]) / d).clamp(-1.0, 1.0)
            };
            let angle = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
            let (left_gain, right_gain) = (angle.cos(), angle.sin());
            let step = f64::from(b.pcm.rate) * f64::from(v.pitch.clamp(0.0, 4.0)) / f64::from(rate);
            for f in 0..frames {
                if v.cursor >= total as f64 {
                    if v.looping {
                        v.cursor %= total as f64;
                    } else {
                        v.state = PlayState::Stopped;
                        break;
                    }
                }
                let i = v.cursor as usize;
                let t = (v.cursor - i as f64) as f32;
                let j = if i + 1 < total {
                    i + 1
                } else if v.looping {
                    0
                } else {
                    i
                };
                let stereo = b.pcm.channels >= 2;
                for c in 0..channels {
                    let src = if stereo { c.min(1) } else { 0 };
                    let a = sample(&b.pcm.data, b.pcm.bits, b.pcm.channels, i, src);
                    let n = sample(&b.pcm.data, b.pcm.bits, b.pcm.channels, j, src);
                    let mut x = (a + (n - a) * t) * gain;
                    if channels >= 2 && !stereo {
                        x *= if c == 0 {
                            left_gain
                        } else if c == 1 {
                            right_gain
                        } else {
                            0.0
                        };
                    }
                    out[f * channels + c] += x;
                }
                v.cursor += step;
            }
            if v.state == PlayState::Playing && v.cursor >= total as f64 && !v.looping {
                v.state = PlayState::Stopped;
            }
        }
    }
}

#[cfg(test)]
#[path = "mix_tests.rs"]
mod tests;
