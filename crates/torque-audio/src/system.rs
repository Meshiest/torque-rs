//! The `alx` layer in the design of TGE's audio code: the source table, handles, the channel ("type") volumes, the listener, and the
//! looping images, with OpenAL's own state (each source's play state, gain, pitch, position) kept beside it. Nothing here plays
//! sound: the voices' play state moves with the clock the host gives ([`Audio::advance`], the null sink) or with the frames the
//! device takes ([`crate::mix`]).
//!
//! **Handles**: a counter, `(n + 1) & 0x4fffffff`, never 0. Bits on a handle: looping `0x80000000`, streaming `0x40000000`, inactive
//! `0x20000000` (made, not played yet; or a looping image without a source), loading `0x10000000`. A handle is returned with the
//! inactive and loading bits cleared (`& 0xcfffffff`); a source is found by `(handle ^ h) & 0x4fffffff == 0`.
//!
//! **`alxCreateSource`** ([`Audio::create_source`]): nothing without a device or a name; the volume is the description's, times
//! (3D with a position) the linear estimate between the reference and the maximum distance from the listener, times the channel's;
//! a type above 31 is no source; a sound that is not looping (or streamed) is no source when its channel is at 0 or its volume is at most
//! [`MIN_VOLUME`]; a volume above [`MIN_VOLUME`] takes a free source or [`Audio::cull`]s a quieter one. A looping sound with no source
//! becomes an inactive looping image (a handle with the looping bit, no source) when its file loads. A sound whose file gives no buffer
//! is no source. The source is set up with its gain through the volume curve, looping, pitch 1, cones, relative at (0, 0, 1) without a
//! position or for a 2D description, else at it with the cone vector; the reference and maximum distances; its handle inactive.
//!
//! **`alxPlay(handle)`**: an inactive source starts; an active one is left as it is; an inactive looping image moves to the culled
//! list and the looping update may start it; a culled one likewise; any other handle is 0. **`alxStop`**: the source stops and is
//! freed; a looping image is dropped. **`alxStopAll`**. **`alxIsPlaying`**: a source whose OpenAL state is playing.
//!
//! **`alxUpdate`** (the main loop calls it at most every [`UPDATE_PERIOD_MS`]): a positioned source beyond its maximum distance is
//! silenced; every source that is not playing (and not loading) is closed (an inactive one, made and not played, too), a looping
//! one's image going to the culled list; the scores are recomputed; the culled looping images restart, loudest first, once
//! [`RESCORE_MS`] after they were culled and when their score is above [`Audio::uncull_gain`] (TGE's `MIN_UNCULL_GAIN`,
//! [`MIN_UNCULL_GAIN`] unless the host sets another).
//!
//! Streaming descriptions are handled as non-streamed sounds with the streaming bit on their handle.

use std::sync::Arc;

use crate::al;
use crate::buffer::Buffer;
use crate::gain::GainTable;

pub const MAX_SOURCES: usize = 16;
pub const NUM_TYPES: usize = 32;
/// TGE's `MIN_GAIN`: a sound that is not looping starts only above this volume.
pub const MIN_VOLUME: f32 = 0.05;
/// TGE's `MIN_UNCULL_GAIN`: a culled looping sound restarts only when its score is above this.
pub const MIN_UNCULL_GAIN: f32 = 0.1;
pub const UPDATE_PERIOD_MS: u32 = 125;
pub const RESCORE_MS: u32 = 500;
/// The most looping images kept (a hard cap).
pub const MAX_LOOP_IMAGES: usize = 4_096;

pub const LOOPING_BIT: u32 = 0x8000_0000;
pub const STREAMING_BIT: u32 = 0x4000_0000;
pub const INACTIVE_BIT: u32 = 0x2000_0000;
pub const LOADING_BIT: u32 = 0x1000_0000;
pub const MATCH_MASK: u32 = 0x4fff_ffff;
pub const RETURN_MASK: u32 = 0xcfff_ffff;

/// `Audio::Description`: the audio description fields the source table copies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Description {
    pub volume: f32,
    pub is_looping: bool,
    pub is_streaming: bool,
    pub is_3d: bool,
    pub reference_distance: f32,
    pub max_distance: f32,
    pub cone_inside_angle: i32,
    pub cone_outside_angle: i32,
    pub cone_outside_volume: f32,
    pub cone_vector: [f32; 3],
    pub environment_level: f32,
    pub loop_count: i32,
    pub min_loop_gap: i32,
    pub max_loop_gap: i32,
    pub kind: i32,
}

impl Default for Description {
    fn default() -> Description {
        Description {
            volume: 1.0,
            is_looping: false,
            is_streaming: false,
            is_3d: false,
            reference_distance: 1.0,
            max_distance: 100.0,
            cone_inside_angle: 360,
            cone_outside_angle: 360,
            cone_outside_volume: 1.0,
            cone_vector: [0.0, 0.0, 1.0],
            environment_level: 0.0,
            loop_count: -1,
            min_loop_gap: 0,
            max_loop_gap: 0,
            kind: 0,
        }
    }
}

/// OpenAL's play state of a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlayState {
    #[default]
    Initial,
    Playing,
    Stopped,
}

/// OpenAL's state of one source.
#[derive(Debug, Clone, Default)]
pub struct Voice {
    pub buffer: Option<Arc<Buffer>>,
    pub state: PlayState,
    /// Frames of the buffer played (fractional).
    pub cursor: f64,
    pub looping: bool,
    pub pitch: f32,
    /// `AL_GAIN` (after the volume curve).
    pub gain: f32,
    pub relative: bool,
    pub position: [f32; 3],
    pub direction: [f32; 3],
    pub velocity: [f32; 3],
    pub reference_distance: f32,
    pub max_distance: f32,
    pub cone_inner: i32,
    pub cone_outer: i32,
    pub cone_outer_gain: f32,
}

/// One row of the source table.
#[derive(Debug, Clone, Default)]
pub struct Source {
    /// 0: free.
    pub handle: u32,
    pub kind: i32,
    /// The linear volume (the description's, or `AL_GAIN_LINEAR` set later).
    pub volume: f32,
    pub score: f32,
    pub environment: u32,
    pub voice: Voice,
}

/// A looping sound's image: what restarts it when it has no source.
#[derive(Debug, Clone)]
pub struct LoopImage {
    pub handle: u32,
    pub buffer: Option<Arc<Buffer>>,
    pub desc: Description,
    pub environment: u32,
    pub position: Option<[f32; 3]>,
    pub score: f32,
    pub culled_at: u32,
    pub pitch: f32,
    /// In the inactive list (made with no source, not played) or the culled list (played, waiting for a source).
    pub place: LoopPlace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopPlace {
    /// It has a source.
    Active,
    Inactive,
    Culled,
}

/// The listener.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Listener {
    pub gain: f32,
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    /// At, then up.
    pub orientation: [f32; 6],
}

impl Default for Listener {
    fn default() -> Listener {
        Listener {
            gain: 1.0,
            position: [0.0; 3],
            velocity: [0.0; 3],
            orientation: [0.0, 0.0, -1.0, 0.0, 1.0, 0.0],
        }
    }
}

/// The whole layer.
#[derive(Debug, Clone)]
pub struct Audio {
    pub initialized: bool,
    pub sources: Vec<Source>,
    pub type_volume: [f32; NUM_TYPES],
    pub listener: Listener,
    pub loops: Vec<LoopImage>,
    counter: u32,
    pub curve: GainTable,
    /// The score a culled looping image must pass to restart ([`MIN_UNCULL_GAIN`] by default).
    pub uncull_gain: f32,
    /// The host clock in milliseconds (a wrapping tick count, used by the looping lists).
    pub now_ms: u32,
    last_update: u32,
}

impl Default for Audio {
    fn default() -> Audio {
        Audio::new(GainTable::linear())
    }
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// TGE's linear estimate between the reference and the maximum distance.
fn linear_falloff(distance: f32, reference: f32, max: f32) -> f32 {
    if distance < max {
        if reference <= distance {
            1.0 - (distance - reference) / (max - reference)
        } else {
            1.0
        }
    } else {
        0.0
    }
}

impl Audio {
    pub fn new(curve: GainTable) -> Audio {
        Audio {
            initialized: false,
            sources: Vec::new(),
            type_volume: [0.0; NUM_TYPES],
            listener: Listener::default(),
            loops: Vec::new(),
            counter: 0,
            curve,
            uncull_gain: MIN_UNCULL_GAIN,
            now_ms: 0,
            last_update: 0,
        }
    }

    /// `OpenALDLLInit`: the old device shut down, 16 sources, every channel at 1. The handle counter is kept.
    pub fn init(&mut self) -> bool {
        self.shutdown();
        self.sources = vec![Source::default(); MAX_SOURCES];
        self.type_volume = [1.0; NUM_TYPES];
        self.listener = Listener::default();
        self.initialized = true;
        true
    }

    /// `OpenALShutdownDriver`: everything stops and the sources go.
    pub fn shutdown(&mut self) {
        self.sources.clear();
        self.loops.clear();
        self.initialized = false;
    }

    fn new_handle(&mut self) -> u32 {
        self.counter = self.counter.wrapping_add(1) & MATCH_MASK;
        if self.counter == 0 {
            self.counter = 1;
        }
        self.counter
    }

    /// The source row of `handle`.
    pub fn find(&self, handle: u32) -> Option<usize> {
        self.sources
            .iter()
            .position(|s| s.handle != 0 && (s.handle ^ handle) & MATCH_MASK == 0)
    }

    fn find_loop(&self, handle: u32) -> Option<usize> {
        self.loops
            .iter()
            .position(|l| l.handle & MATCH_MASK == handle & MATCH_MASK)
    }

    fn channel(&self, kind: i32) -> f32 {
        usize::try_from(kind)
            .ok()
            .and_then(|k| self.type_volume.get(k).copied())
            .unwrap_or(0.0)
    }

    /// `type volume * volume` through the volume curve, clamped to 0..1.
    fn curve_gain(&self, kind: i32, volume: f32) -> f32 {
        let v = self.channel(kind) * volume;
        if v > 1.0 {
            1.0
        } else if v <= 0.0 {
            0.0
        } else {
            self.curve.to_gain(v)
        }
    }

    /// `alxCreateSource(desc, file, transform)` (see the module). `buffer` is the file as `AudioBuffer::find` loaded it (None: no
    /// buffer), `position` the transform's.
    pub fn create_source(
        &mut self,
        desc: &Description,
        buffer: Option<Arc<Buffer>>,
        position: Option<[f32; 3]>,
        environment: u32,
    ) -> u32 {
        if !self.initialized {
            return 0;
        }
        let mut volume = desc.volume;
        if let (Some(p), true) = (position, desc.is_3d) {
            let d = dist(self.listener.position, p);
            volume *= linear_falloff(d, desc.reference_distance, desc.max_distance);
        }
        if desc.kind > 31 || desc.kind < 0 {
            return 0;
        }
        let quiet_ok = desc.is_looping || desc.is_streaming;
        if !quiet_ok && self.channel(desc.kind) == 0.0 {
            return 0;
        }
        volume *= self.channel(desc.kind);
        if !quiet_ok && volume <= MIN_VOLUME {
            return 0;
        }
        let mut index = None;
        if volume > MIN_VOLUME {
            index = self.sources.iter().position(|s| s.handle == 0);
            if index.is_none() {
                self.update_scores(true);
                index = self.cull(volume);
            }
        }
        let Some(index) = index else {
            if desc.is_looping
                && !desc.is_streaming
                && buffer.is_some()
                && self.loops.len() < MAX_LOOP_IMAGES
            {
                let handle = self.new_handle() | LOOPING_BIT | INACTIVE_BIT;
                self.loops.push(LoopImage {
                    handle,
                    buffer,
                    desc: *desc,
                    environment,
                    position,
                    score: volume,
                    culled_at: 0,
                    pitch: 1.0,
                    place: LoopPlace::Inactive,
                });
                return handle & RETURN_MASK;
            }
            return 0;
        };
        let Some(buffer) = buffer else {
            return 0;
        };
        let mut handle = self.new_handle() | INACTIVE_BIT;
        let gain = self.curve_gain(desc.kind, desc.volume);
        // The transform reaches the source's set-up only for a 3D description: a 2D sound (a GUI sound played from a script) is
        // relative at (0, 0, 1) whatever transform `alxPlay` made.
        let placed = position.filter(|_| desc.is_3d);
        let voice = Voice {
            buffer: Some(buffer.clone()),
            state: PlayState::Initial,
            cursor: 0.0,
            looping: desc.is_looping,
            pitch: 1.0,
            gain,
            relative: placed.is_none(),
            position: placed.unwrap_or([0.0, 0.0, 1.0]),
            direction: if placed.is_some() {
                desc.cone_vector
            } else {
                [0.0; 3]
            },
            velocity: [0.0; 3],
            reference_distance: desc.reference_distance,
            max_distance: desc.max_distance,
            cone_inner: desc.cone_inside_angle,
            cone_outer: desc.cone_outside_angle,
            cone_outer_gain: desc.cone_outside_volume,
        };
        if desc.is_looping && !desc.is_streaming {
            handle |= LOOPING_BIT;
            if self.loops.len() < MAX_LOOP_IMAGES {
                self.loops.push(LoopImage {
                    handle,
                    buffer: Some(buffer),
                    desc: *desc,
                    environment,
                    position,
                    score: volume,
                    culled_at: 0,
                    pitch: 1.0,
                    place: LoopPlace::Active,
                });
            }
        }
        if desc.is_streaming {
            handle |= STREAMING_BIT;
        }
        self.sources[index] = Source {
            handle,
            kind: desc.kind,
            volume: desc.volume,
            score: volume,
            environment,
            voice,
        };
        handle & RETURN_MASK
    }

    /// `alxGetStreamPosition` and `alxGetStreamDuration` of a streamed source: seconds played and seconds in all. None: no such
    /// streaming source (TGE answers -1).
    pub fn stream_times(&self, handle: u32) -> Option<(f32, f32)> {
        let s = &self.sources[self.find(handle)?];
        if s.handle & STREAMING_BIT == 0 {
            return None;
        }
        let b = s.voice.buffer.as_ref()?;
        let rate = f64::from(b.pcm.rate.max(1));
        Some(((s.voice.cursor / rate) as f32, b.seconds() as f32))
    }

    /// `alxPlay(handle)`.
    pub fn play(&mut self, handle: u32) -> u32 {
        if let Some(i) = self.find(handle) {
            let s = &mut self.sources[i];
            if s.handle & INACTIVE_BIT != 0 {
                s.handle &= RETURN_MASK;
                s.voice.state = PlayState::Playing;
                s.voice.cursor = 0.0;
                let h = s.handle;
                if let Some(l) = self.find_loop(h) {
                    self.loops[l].handle &= RETURN_MASK;
                }
            }
            return handle;
        }
        if (handle as i32) < 0
            && let Some(l) = self.find_loop(handle)
            && self.loops[l].place == LoopPlace::Inactive
        {
            self.loops[l].place = LoopPlace::Culled;
            self.loop_update();
            return handle;
        }
        if let Some(l) = self.find_loop(handle)
            && self.loops[l].place == LoopPlace::Culled
        {
            self.loop_update();
            return handle;
        }
        0
    }

    fn free_source(&mut self, i: usize) {
        self.sources[i] = Source::default();
    }

    /// `alxStop(handle)`.
    pub fn stop(&mut self, handle: u32) {
        if let Some(i) = self.find(handle) {
            self.free_source(i);
        }
        if (handle as i32) < 0
            && let Some(l) = self.find_loop(handle)
        {
            self.loops.remove(l);
        }
    }

    /// `alxStopAll()`: every source, last first, then every looping image.
    pub fn stop_all(&mut self) {
        for i in (0..self.sources.len()).rev() {
            let h = self.sources[i].handle;
            if h != 0 {
                self.stop(h);
            }
        }
        self.loops.clear();
    }

    /// `alxIsPlaying(handle)`.
    pub fn is_playing(&self, handle: u32) -> bool {
        handle != 0
            && self
                .find(handle)
                .is_some_and(|i| self.sources[i].voice.state == PlayState::Playing)
    }

    /// `cullSource`: the first source of the lowest score below `volume` is taken from its sound (a looping one's image goes
    /// to the culled list, or the inactive list when its source had not played).
    pub fn cull(&mut self, volume: f32) -> Option<usize> {
        let mut best = volume;
        let mut found = None;
        for (i, s) in self.sources.iter().enumerate() {
            if s.score < best {
                best = s.score;
                found = Some(i);
            }
        }
        let i = found?;
        let h = self.sources[i].handle;
        if (h as i32) < 0
            && let Some(l) = self.find_loop(h)
        {
            if h & INACTIVE_BIT == 0 {
                self.loops[l].place = LoopPlace::Culled;
                self.loops[l].handle |= INACTIVE_BIT;
                self.loops[l].culled_at = self.now_ms;
            } else {
                self.loops[l].place = LoopPlace::Inactive;
            }
        }
        self.free_source(i);
        Some(i)
    }

    /// `alxUpdateScores(sourcesOnly)`.
    pub fn update_scores(&mut self, sources_only: bool) {
        let listener = self.listener.position;
        for i in 0..self.sources.len() {
            let s = &self.sources[i];
            let score = if s.handle == 0 {
                0.0
            } else {
                let mut score = self.channel(s.kind) * s.volume;
                if !s.voice.relative {
                    let d = dist(s.voice.position, listener);
                    let (r, m) = (s.voice.reference_distance, s.voice.max_distance);
                    if d >= m {
                        score = 0.0;
                    } else if r < d {
                        score *= (m - d) / (m - r);
                    }
                }
                score
            };
            self.sources[i].score = score;
        }
        if sources_only {
            return;
        }
        let now = self.now_ms;
        for l in 0..self.loops.len() {
            let img = &self.loops[l];
            if img.handle & INACTIVE_BIT == 0 || now.wrapping_sub(img.culled_at) <= 499 {
                continue;
            }
            let mut score = img.desc.volume;
            if img.desc.is_3d
                && let Some(p) = img.position
            {
                let d = dist(p, listener);
                let (r, m) = (img.desc.reference_distance, img.desc.max_distance);
                if d >= m {
                    score = 0.0;
                } else if r < d {
                    score *= (m - d) / (m - r);
                }
            }
            score *= self.channel(img.desc.kind);
            self.loops[l].score = score;
        }
    }

    /// The looping update: culled images restart, loudest first.
    pub fn loop_update(&mut self) {
        let now = self.now_ms;
        let mut ready: Vec<usize> = (0..self.loops.len())
            .filter(|&l| {
                let img = &self.loops[l];
                img.place == LoopPlace::Culled
                    && img.score > self.uncull_gain
                    && now.wrapping_sub(img.culled_at) > 499
            })
            .collect();
        ready.sort_by(|a, b| self.loops[*b].score.total_cmp(&self.loops[*a].score));
        let mut drop = Vec::new();
        for l in ready {
            let index = match self.sources.iter().position(|s| s.handle == 0) {
                Some(i) => i,
                None => match self.cull(self.loops[l].score) {
                    Some(i) => i,
                    None => break,
                },
            };
            let img = self.loops[l].clone();
            let Some(buffer) = img.buffer.clone() else {
                drop.push(img.handle);
                continue;
            };
            let gain = self.curve_gain(img.desc.kind, img.desc.volume);
            // As at creation: the restart builds the transform only for a 3D description, else none (relative).
            let placed = img.position.filter(|_| img.desc.is_3d);
            self.sources[index] = Source {
                handle: img.handle | INACTIVE_BIT,
                kind: img.desc.kind,
                volume: img.desc.volume,
                score: img.score,
                environment: img.environment,
                voice: Voice {
                    buffer: Some(buffer),
                    state: PlayState::Initial,
                    cursor: 0.0,
                    looping: true,
                    pitch: 1.0,
                    gain,
                    relative: placed.is_none(),
                    position: placed.unwrap_or([0.0, 0.0, 1.0]),
                    direction: if placed.is_some() {
                        img.desc.cone_vector
                    } else {
                        [0.0; 3]
                    },
                    velocity: [0.0; 3],
                    reference_distance: img.desc.reference_distance,
                    max_distance: img.desc.max_distance,
                    cone_inner: img.desc.cone_inside_angle,
                    cone_outer: img.desc.cone_outside_angle,
                    cone_outer_gain: img.desc.cone_outside_volume,
                },
            };
            self.loops[l].place = LoopPlace::Active;
            self.loops[l].handle |= INACTIVE_BIT;
            let h = self.sources[index].handle;
            self.play(h);
        }
        for h in drop {
            if let Some(l) = self.find_loop(h) {
                self.loops.remove(l);
            }
        }
    }

    /// `alxUpdate`, when [`UPDATE_PERIOD_MS`] have passed since the last one (the main loop's gate). Returns whether it ran.
    pub fn tick(&mut self, now_ms: u32) -> bool {
        self.now_ms = now_ms;
        if now_ms.wrapping_sub(self.last_update) < UPDATE_PERIOD_MS {
            return false;
        }
        self.last_update = now_ms;
        self.update();
        true
    }

    /// `alxUpdate`'s body.
    pub fn update(&mut self) {
        if !self.initialized {
            return;
        }
        let listener = self.listener.position;
        for i in 0..self.sources.len() {
            let s = &self.sources[i];
            if s.handle == 0 || s.voice.relative {
                continue;
            }
            let beyond = s.voice.max_distance - dist(s.voice.position, listener);
            let gain = if beyond >= 0.0 {
                self.curve_gain(s.kind, s.volume)
            } else {
                0.0
            };
            self.sources[i].voice.gain = gain;
        }
        for i in 0..self.sources.len() {
            let h = self.sources[i].handle;
            if h == 0 || h & LOADING_BIT != 0 || self.sources[i].voice.state == PlayState::Playing {
                continue;
            }
            if h & INACTIVE_BIT == 0
                && (h as i32) < 0
                && let Some(l) = self.find_loop(h)
                && self.loops[l].handle & INACTIVE_BIT == 0
            {
                self.loops[l].place = LoopPlace::Culled;
                self.loops[l].handle |= INACTIVE_BIT;
            }
            self.free_source(i);
        }
        self.update_scores(false);
        self.loop_update();
    }

    /// The voices' play state after `ms` of the host's clock (the null sink: nothing is mixed).
    pub fn advance(&mut self, ms: f64) {
        for s in &mut self.sources {
            let v = &mut s.voice;
            if v.state != PlayState::Playing {
                continue;
            }
            let Some(b) = &v.buffer else {
                v.state = PlayState::Stopped;
                continue;
            };
            let frames = b.pcm.frames() as f64;
            v.cursor += ms / 1000.0 * f64::from(b.pcm.rate) * f64::from(v.pitch.clamp(0.0, 4.0));
            if v.cursor >= frames {
                if v.looping && frames > 0.0 {
                    v.cursor %= frames;
                } else {
                    v.state = PlayState::Stopped;
                }
            }
        }
    }

    /// `alxSetChannelVolume(channel, volume)`: false for a channel above 31; the volume clamped to 0..1; the playing sources
    /// of the channel take their new gain.
    pub fn set_channel_volume(&mut self, channel: u32, volume: f32) -> bool {
        let Some(slot) = self.type_volume.get_mut(channel as usize) else {
            return false;
        };
        *slot = if volume > 1.0 {
            1.0
        } else if volume > 0.0 {
            volume
        } else {
            0.0
        };
        for i in 0..self.sources.len() {
            let s = &self.sources[i];
            if s.handle != 0 && s.kind == channel as i32 && s.voice.state == PlayState::Playing {
                let g = self.curve_gain(s.kind, s.volume);
                self.sources[i].voice.gain = g;
            }
        }
        true
    }

    /// `alxListenerf(enum, value)` (the enum already looked up with mask 0xa2).
    pub fn listener_f(&mut self, e: u32, value: f32) {
        match e {
            al::AL_GAIN_LINEAR => self.listener.gain = self.curve.to_gain(value),
            al::AL_GAIN => self.listener.gain = value,
            _ => {}
        }
    }

    /// `alxGetListenerf(enum)`.
    pub fn get_listener_f(&self, e: u32) -> f32 {
        match e {
            al::AL_GAIN_LINEAR => self.curve.to_linear(self.listener.gain),
            al::AL_GAIN => self.listener.gain,
            _ => 0.0,
        }
    }

    /// `alListener3f(enum, x, y, z)`.
    pub fn listener_3f(&mut self, e: u32, v: [f32; 3]) {
        match e {
            al::AL_POSITION => self.listener.position = v,
            al::AL_VELOCITY => self.listener.velocity = v,
            _ => {}
        }
    }

    /// `alxSourcef(handle, enum, value)`.
    pub fn source_f(&mut self, handle: u32, e: u32, value: f32) {
        let (e, value) = if e == al::AL_GAIN {
            (al::AL_GAIN_LINEAR, self.curve.to_linear(value))
        } else {
            (e, value)
        };
        if let Some(i) = self.find(handle) {
            if e == al::AL_GAIN_LINEAR {
                self.sources[i].volume = value;
                self.sources[i].voice.gain = self.curve_gain(self.sources[i].kind, value);
            } else {
                let voice = &mut self.sources[i].voice;
                match e {
                    al::AL_PITCH => voice.pitch = value,
                    al::AL_REFERENCE_DISTANCE => voice.reference_distance = value,
                    al::AL_MAX_DISTANCE => voice.max_distance = value,
                    al::AL_CONE_OUTER_GAIN => voice.cone_outer_gain = value,
                    _ => {}
                }
            }
        }
        if (handle as i32) < 0
            && let Some(l) = self.find_loop(handle)
        {
            let img = &mut self.loops[l];
            match e {
                al::AL_REFERENCE_DISTANCE => img.desc.reference_distance = value,
                al::AL_PITCH => img.pitch = value,
                // The image keeps this value in its maximum distance.
                al::AL_CONE_OUTER_GAIN => img.desc.max_distance = value,
                al::AL_GAIN_LINEAR => img.desc.volume = value,
                _ => {}
            }
        }
    }

    /// `alxGetSourcef(handle, enum)`; None where there is no answer (an unknown handle).
    pub fn get_source_f(&self, handle: u32, e: u32) -> Option<f32> {
        if let Some(i) = self.find(handle) {
            let s = &self.sources[i];
            return Some(match e {
                al::AL_GAIN_LINEAR => s.volume,
                al::AL_GAIN => self.curve.to_gain(s.volume),
                al::AL_PITCH => s.voice.pitch,
                al::AL_REFERENCE_DISTANCE => s.voice.reference_distance,
                al::AL_MAX_DISTANCE => s.voice.max_distance,
                al::AL_CONE_OUTER_GAIN => s.voice.cone_outer_gain,
                _ => return None,
            });
        }
        if (handle as i32) < 0
            && let Some(l) = self.find_loop(handle)
        {
            let img = &self.loops[l];
            return match e {
                al::AL_REFERENCE_DISTANCE => Some(img.desc.reference_distance),
                al::AL_PITCH => Some(img.pitch),
                al::AL_GAIN => Some(self.curve.to_gain(img.desc.volume)),
                al::AL_CONE_OUTER_GAIN => Some(img.desc.max_distance),
                al::AL_GAIN_LINEAR => Some(img.desc.volume),
                _ => None,
            };
        }
        None
    }

    /// `alxSourcei(handle, enum, value)`.
    pub fn source_i(&mut self, handle: u32, e: u32, value: i32) {
        if let Some(i) = self.find(handle) {
            let voice = &mut self.sources[i].voice;
            match e {
                al::AL_LOOPING => voice.looping = value != 0,
                al::AL_CONE_INNER_ANGLE => voice.cone_inner = value,
                al::AL_CONE_OUTER_ANGLE => voice.cone_outer = value,
                _ => {}
            }
        }
        if (handle as i32) < 0
            && let Some(l) = self.find_loop(handle)
        {
            match e {
                al::AL_CONE_INNER_ANGLE => self.loops[l].desc.cone_inside_angle = value,
                al::AL_CONE_OUTER_ANGLE => self.loops[l].desc.cone_outside_angle = value,
                _ => {}
            }
        }
    }

    /// `alxGetSourcei(handle, enum)`; None where there is no answer.
    pub fn get_source_i(&self, handle: u32, e: u32) -> Option<i32> {
        if let Some(i) = self.find(handle) {
            let v = &self.sources[i].voice;
            return match e {
                al::AL_LOOPING => Some(i32::from(v.looping)),
                al::AL_CONE_INNER_ANGLE => Some(v.cone_inner),
                al::AL_CONE_OUTER_ANGLE => Some(v.cone_outer),
                _ => None,
            };
        }
        if (handle as i32) < 0
            && let Some(l) = self.find_loop(handle)
        {
            let img = &self.loops[l];
            return match e {
                al::AL_CONE_INNER_ANGLE => Some(img.desc.cone_inside_angle),
                al::AL_CONE_OUTER_ANGLE => Some(img.desc.cone_outside_angle),
                al::AL_LOOPING => Some(1),
                _ => None,
            };
        }
        None
    }

    /// `alxSource3f(handle, enum, x, y, z)`.
    pub fn source_3f(&mut self, handle: u32, e: u32, v: [f32; 3]) {
        if let Some(i) = self.find(handle) {
            let voice = &mut self.sources[i].voice;
            match e {
                al::AL_POSITION => voice.position = v,
                al::AL_DIRECTION => voice.direction = v,
                al::AL_VELOCITY => voice.velocity = v,
                _ => {}
            }
        }
        if (handle as i32) < 0
            && e == al::AL_POSITION
            && let Some(l) = self.find_loop(handle)
        {
            self.loops[l].position = Some(v);
        }
    }

    /// How many sources are in use.
    pub fn sources_in_use(&self) -> usize {
        self.sources.iter().filter(|s| s.handle != 0).count()
    }
}

#[cfg(test)]
#[path = "system_tests.rs"]
mod tests;
