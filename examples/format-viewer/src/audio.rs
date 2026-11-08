//! Sound for the pictures, rendered offline (no device): soundscript entries and waves through the engine's mixer
//! (`torque_audio::Audio::render`), positioned in the map's units with the sound level's falloff
//! (`torque_format_valve::sound::falloff`), soundscapes chosen by the map's `env_soundscape` entities, `ambient_generic`
//! sounds, and a sequence's sound events. The mix is written as a 16-bit stereo WAV file to put beside the frames.
//!
//! `sound <out.wav> <soundscript entry or wave path> [seconds]`: one sound into a file.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use torque_audio::wav::Pcm;
use torque_audio::{Audio, Buffer, Description, GainTable};
use torque_format_valve::bsp::{Entity, value, vec3};
use torque_format_valve::sound::{
    ScapeRule, SoundEntry, Soundscape, falloff, read_manifest, read_sound, read_soundscapes,
    read_soundscript, wave_path,
};

use crate::Content;

/// The output rate.
pub const RATE: u32 = 44_100;
/// The deepest nesting of soundscapes followed.
const MAX_NESTING: usize = 8;
/// The most soundscapes one soundscape expands into, nested ones included (a soundscape naming several others, each
/// naming several more, would otherwise grow by its width to the power of the depth).
const MAX_EXPANSIONS: usize = 256;
/// The most samples a mix holds (ten minutes of stereo).
const MAX_MIX_SAMPLES: usize = RATE as usize * 2 * 600;

/// The game's soundscript entries and soundscapes, and the waves decoded so far.
pub struct SoundBank {
    entries: HashMap<String, SoundEntry>,
    scapes: HashMap<String, Soundscape>,
    buffers: HashMap<String, Option<Arc<Buffer>>>,
}

/// The files a manifest lists, else every file under `scripts/` that starts with `prefix`.
fn script_files(c: &Content, manifest: &str, prefix: &str) -> Vec<String> {
    let listed = c
        .files
        .read_path(manifest)
        .and_then(|r| r.ok())
        .and_then(|b| read_manifest(&b).ok())
        .unwrap_or_default();
    if listed.is_empty() {
        c.files.list(&format!("scripts/{prefix}"), ".txt")
    } else {
        listed
    }
}

impl SoundBank {
    pub fn load(c: &Content) -> SoundBank {
        let mut entries = HashMap::new();
        for f in script_files(c, "scripts/game_sounds_manifest.txt", "game_sounds") {
            let Some(Ok(b)) = c.files.read_path(&f) else {
                continue;
            };
            for e in read_soundscript(&b).unwrap_or_default() {
                entries.entry(e.name.to_ascii_lowercase()).or_insert(e);
            }
        }
        let mut scapes = HashMap::new();
        for f in script_files(c, "scripts/soundscapes_manifest.txt", "soundscapes") {
            let Some(Ok(b)) = c.files.read_path(&f) else {
                continue;
            };
            for s in read_soundscapes(&b).unwrap_or_default() {
                scapes.entry(s.name.to_ascii_lowercase()).or_insert(s);
            }
        }
        println!(
            "sounds: {} soundscript entries, {} soundscapes",
            entries.len(),
            scapes.len()
        );
        SoundBank {
            entries,
            scapes,
            buffers: HashMap::new(),
        }
    }

    pub fn entry(&self, name: &str) -> Option<&SoundEntry> {
        self.entries.get(&name.to_ascii_lowercase())
    }

    pub fn scape(&self, name: &str) -> Option<&Soundscape> {
        self.scapes.get(&name.to_ascii_lowercase())
    }

    /// A wave decoded (once) as the mixer's buffer.
    pub fn buffer(&mut self, c: &Content, wave: &str) -> Option<Arc<Buffer>> {
        let path = wave_path(wave);
        if let Some(b) = self.buffers.get(&path) {
            return b.clone();
        }
        let made =
            c.files
                .read_path(&path)
                .and_then(|r| r.ok())
                .and_then(|bytes| match read_sound(&path, &bytes) {
                    Ok(w) => {
                        let data: Vec<u8> =
                            w.samples.iter().flat_map(|s| s.to_le_bytes()).collect();
                        let size = data.len();
                        Some(Arc::new(Buffer {
                            name: path.clone().into_bytes(),
                            pcm: Pcm {
                                channels: w.channels,
                                bits: 16,
                                rate: w.rate,
                                data,
                                size,
                            },
                        }))
                    }
                    Err(e) => {
                        eprintln!("{path}: {e}");
                        None
                    }
                });
        if made.is_none() {
            eprintln!("no sound {path}");
        }
        self.buffers.insert(path, made.clone());
        made
    }
}

/// How a sound plays: its volume (0..1), pitch (percent), sound level (dB; 0 heard everywhere), the radius it is heard to
/// instead of its level's falloff, and whether it loops.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct How {
    pub volume: f32,
    pub pitch: f32,
    pub level: f32,
    pub radius: Option<f32>,
    pub looping: bool,
}

impl How {
    pub fn new(volume: f32, pitch: f32, level: f32, looping: bool) -> How {
        How {
            volume,
            pitch,
            level,
            radius: None,
            looping,
        }
    }
}

/// A voice the mix moves: its handle and what sets its gain each step.
struct Voice {
    handle: u32,
    position: Option<[f32; 3]>,
    volume: f32,
    level: f32,
    /// A radius past which it is silent (an ambient sound's), none: the sound level's falloff.
    radius: Option<f32>,
}

/// The offline mix.
pub struct Mix {
    audio: Audio,
    voices: Vec<Voice>,
    pub samples: Vec<f32>,
    rng: u64,
    listener: [f32; 3],
}

impl Default for Mix {
    fn default() -> Mix {
        Mix::new()
    }
}

impl Mix {
    pub fn new() -> Mix {
        let mut audio = Audio::new(GainTable::default());
        audio.init();
        Mix {
            audio,
            voices: Vec::new(),
            samples: Vec::new(),
            rng: 0x2545_f491_4f6c_dd1d,
            listener: [0.0; 3],
        }
    }

    pub fn random(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        (x >> 40) as f32 / (1u64 << 24) as f32
    }

    /// The listener at `position` facing `forward` (the map's units and frame).
    pub fn listen(&mut self, position: [f32; 3], forward: [f32; 3]) {
        self.listener = position;
        self.audio.listener.position = position;
        self.audio.listener.orientation = [forward[0], forward[1], forward[2], 0.0, 0.0, 1.0];
    }

    /// Plays a buffer (at `position`, or everywhere), its gain set every step from its volume and the falloff.
    pub fn play(&mut self, buffer: Arc<Buffer>, position: Option<[f32; 3]>, how: How) {
        let How {
            volume,
            pitch,
            level,
            radius,
            looping,
        } = how;
        let desc = Description {
            volume: volume.clamp(0.0, 1.0),
            is_looping: looping,
            is_3d: position.is_some(),
            // The mixer's own distance model is kept out (the gain is set from the falloff), its panning kept.
            reference_distance: 1e9,
            max_distance: 1e9,
            ..Description::default()
        };
        let handle = self.audio.create_source(&desc, Some(buffer), position, 0);
        if handle == 0 {
            return;
        }
        self.audio.source_f(
            handle,
            torque_audio::al::AL_PITCH,
            (pitch / 100.0).clamp(0.25, 4.0),
        );
        self.audio.play(handle);
        self.voices.push(Voice {
            handle,
            position,
            volume,
            level,
            radius,
        });
    }

    /// Plays a soundscript entry (or a wave by path): one of its waves, at a volume and pitch in its ranges.
    pub fn play_entry(
        &mut self,
        bank: &mut SoundBank,
        c: &Content,
        name: &str,
        position: Option<[f32; 3]>,
        looping: bool,
    ) {
        let (wave, volume, pitch, level) = match bank.entry(name).cloned() {
            Some(e) if !e.waves.is_empty() => {
                let k = ((self.random() * e.waves.len() as f32) as usize).min(e.waves.len() - 1);
                let v = e.volume.0 + (e.volume.1 - e.volume.0) * self.random();
                let p = e.pitch.0 + (e.pitch.1 - e.pitch.0) * self.random();
                (e.waves[k].clone(), v, p, e.level)
            }
            _ => (name.to_string(), 1.0, 100.0, 75.0),
        };
        if let Some(b) = bank.buffer(c, &wave) {
            self.play(b, position, How::new(volume, pitch, level, looping));
        }
    }

    /// Mixes `seconds` more, the voices' gains set from the listener's place first.
    pub fn advance(&mut self, seconds: f32) {
        self.voices.retain(|v| self.audio.find(v.handle).is_some());
        for v in &self.voices {
            let gain = match v.position {
                None => v.volume,
                Some(p) => {
                    let d = ((p[0] - self.listener[0]).powi(2)
                        + (p[1] - self.listener[1]).powi(2)
                        + (p[2] - self.listener[2]).powi(2))
                    .sqrt();
                    v.volume
                        * match v.radius {
                            Some(r) => (1.0 - d / r.max(1.0)).clamp(0.0, 1.0),
                            None => falloff(v.level, d),
                        }
                }
            };
            self.audio
                .source_f(v.handle, torque_audio::al::AL_GAIN, gain.clamp(0.0, 1.0));
            if let Some(p) = v.position {
                self.audio
                    .source_3f(v.handle, torque_audio::al::AL_POSITION, p);
            }
        }
        let frames = (seconds.max(0.0) * RATE as f32).round() as usize;
        let at = self.samples.len();
        if at + frames * 2 > MAX_MIX_SAMPLES {
            return;
        }
        self.samples.resize(at + frames * 2, 0.0);
        self.audio.render(&mut self.samples[at..], 2, RATE);
        // Sources that finished are freed for the next sounds.
        self.audio.update();
    }

    /// Stops every voice.
    pub fn stop_all(&mut self) {
        self.audio.stop_all();
        self.voices.clear();
    }

    /// The mix as a 16-bit stereo WAV file, normalized.
    pub fn write_wav(&self, path: &Path) -> Result<(), String> {
        let peak = self.samples.iter().fold(0f32, |m, s| m.max(s.abs()));
        // Normalized to a peak of 0.9 (a quiet mix raised, a loud one brought under full scale).
        let scale = if peak > 1e-6 { 0.9 / peak } else { 1.0 };
        let pcm: Vec<i16> = self
            .samples
            .iter()
            .map(|s| (s * scale * 32767.0) as i16)
            .collect();
        let bytes = torque_format_valve::sound::write_wav(RATE, 2, &pcm, None);
        std::fs::write(path, bytes).map_err(|e| e.to_string())
    }
}

/// The soundscape the listener is in: the nearest `env_soundscape` (or `env_soundscape_triggerable`) whose radius reaches it.
pub fn soundscape_at(entities: &[Entity], p: [f32; 3]) -> Option<&Entity> {
    entities
        .iter()
        .filter(|e| value(e, "classname").is_some_and(|c| c.eq_ignore_ascii_case("env_soundscape")))
        .filter_map(|e| {
            let o = value(e, "origin").and_then(vec3)?;
            let r = value(e, "radius")
                .and_then(|r| r.trim().parse::<f32>().ok())
                .unwrap_or(128.0);
            let d = ((o[0] - p[0]).powi(2) + (o[1] - p[1]).powi(2) + (o[2] - p[2]).powi(2)).sqrt();
            (r < 0.0 || d <= r).then_some((d, e))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, e)| e)
}

/// A soundscape playing: its looping voices started, its random rules timed.
pub struct ScapePlayer {
    pub name: Option<String>,
    randoms: Vec<(ScapeRule, f32, Option<[f32; 3]>, f32)>,
}

impl Default for ScapePlayer {
    fn default() -> ScapePlayer {
        ScapePlayer::new()
    }
}

impl ScapePlayer {
    pub fn new() -> ScapePlayer {
        ScapePlayer {
            name: None,
            randoms: Vec::new(),
        }
    }

    /// Starts soundscape `entity` names (stopping the one before), its positioned rules at the `positionN` targets it names.
    pub fn start(
        &mut self,
        mix: &mut Mix,
        bank: &mut SoundBank,
        c: &Content,
        entity: &Entity,
        entities: &[Entity],
        now: f32,
    ) {
        let Some(name) = value(entity, "soundscape") else {
            return;
        };
        if self
            .name
            .as_deref()
            .is_some_and(|n| n.eq_ignore_ascii_case(name))
        {
            return;
        }
        mix.stop_all();
        self.randoms.clear();
        self.name = Some(name.to_string());
        let target = |n: usize| -> Option<[f32; 3]> {
            let t = value(entity, &format!("position{n}"))?;
            entities
                .iter()
                .find(|e| value(e, "targetname").is_some_and(|v| v.eq_ignore_ascii_case(t)))
                .and_then(|e| value(e, "origin").and_then(vec3))
        };
        for (rule, scale) in expand(|n| bank.scape(n), name) {
            match &rule {
                ScapeRule::Looping {
                    waves,
                    volume,
                    pitch,
                    level,
                    position,
                } => {
                    let Some(w) = waves.first() else { continue };
                    if let Some(b) = bank.buffer(c, w) {
                        let at = position.and_then(target);
                        mix.play(b, at, How::new(volume * scale, *pitch, *level, true));
                    }
                }
                ScapeRule::Random { time, position, .. } => {
                    let at = position.and_then(target);
                    let first = now + time.0 + (time.1 - time.0) * mix.random();
                    self.randoms.push((rule.clone(), scale, at, first));
                }
                ScapeRule::Nested { .. } => {}
            }
        }
    }

    /// Plays the random rules whose time has come by `now`.
    pub fn update(&mut self, mix: &mut Mix, bank: &mut SoundBank, c: &Content, now: f32) {
        for k in 0..self.randoms.len() {
            let (rule, scale, at, next) = self.randoms[k].clone();
            if now < next {
                continue;
            }
            if let ScapeRule::Random {
                waves,
                time,
                volume,
                pitch,
                level,
                ..
            } = &rule
            {
                if !waves.is_empty() {
                    let w =
                        &waves[((mix.random() * waves.len() as f32) as usize).min(waves.len() - 1)];
                    if let Some(b) = bank.buffer(c, w) {
                        let v = volume.0 + (volume.1 - volume.0) * mix.random();
                        let p = pitch.0 + (pitch.1 - pitch.0) * mix.random();
                        mix.play(b, at, How::new(v * scale, p, *level, false));
                    }
                }
                self.randoms[k].3 =
                    now + time.0.max(0.1) + (time.1 - time.0).max(0.0) * mix.random();
            }
        }
    }
}

/// Starts the map's `ambient_generic` sounds that start on (not marked to start silent): looping ones unless marked not
/// looped, everywhere when marked so, else at their place out to their radius, at their volume (`health`, 0 to 10).
pub fn start_ambients(
    mix: &mut Mix,
    bank: &mut SoundBank,
    c: &Content,
    entities: &[Entity],
) -> usize {
    let mut started = 0;
    for e in entities.iter().filter(|e| {
        value(e, "classname").is_some_and(|c| c.eq_ignore_ascii_case("ambient_generic"))
    }) {
        let flags: u32 = value(e, "spawnflags")
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
        if flags & 16 != 0 {
            continue;
        }
        let Some(message) = value(e, "message") else {
            continue;
        };
        let everywhere = flags & 1 != 0;
        let looping = flags & 32 == 0;
        let at = if everywhere {
            None
        } else {
            value(e, "origin").and_then(vec3)
        };
        let volume = value(e, "health")
            .and_then(|v| v.trim().parse::<f32>().ok())
            .unwrap_or(10.0)
            / 10.0;
        let pitch = value(e, "pitch")
            .and_then(|v| v.trim().parse::<f32>().ok())
            .unwrap_or(100.0);
        let radius = value(e, "radius")
            .and_then(|v| v.trim().parse::<f32>().ok())
            .unwrap_or(1250.0);
        let wave = bank
            .entry(message)
            .and_then(|en| en.waves.first().cloned())
            .unwrap_or(message.to_string());
        if let Some(b) = bank.buffer(c, &wave) {
            mix.play(
                b,
                at,
                How {
                    radius: Some(radius),
                    ..How::new(volume, pitch, 0.0, looping)
                },
            );
            started += 1;
        }
    }
    started
}

/// `sound <out.wav> <name> [seconds]`.
pub fn run(c: &Content, args: &[String]) -> Result<(), String> {
    let usage = "sound <out.wav> <soundscript entry or wave path> [seconds]";
    let out = args.first().ok_or(usage)?;
    let name = args.get(1).ok_or(usage)?;
    let seconds: f32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(4.0);
    let mut bank = SoundBank::load(c);
    let mut mix = Mix::new();
    if let Some(e) = bank.entry(name) {
        println!(
            "{}: {:?}, volume {:?}, pitch {:?}, level {} dB, waves {:?}",
            e.name, e.channel, e.volume, e.pitch, e.level, e.waves
        );
    }
    mix.play_entry(&mut bank, c, name, None, false);
    let mut t = 0.0;
    while t < seconds {
        mix.advance(0.1);
        t += 0.1;
    }
    mix.write_wav(Path::new(out))?;
    println!("wrote {out} ({seconds} s)");
    Ok(())
}

/// The rules of the soundscape `name` and of those it nests (`MAX_NESTING` deep, `MAX_EXPANSIONS` soundscapes in all), each
/// with its volume scale; nested rules themselves are not returned.
fn expand<'a>(scape: impl Fn(&str) -> Option<&'a Soundscape>, name: &str) -> Vec<(ScapeRule, f32)> {
    let mut out = Vec::new();
    let mut stack = vec![(name.to_string(), 1.0f32, 0usize)];
    let mut expanded = 0;
    while let Some((n, scale, depth)) = stack.pop() {
        let Some(s) = scape(&n) else {
            eprintln!("no soundscape {n}");
            continue;
        };
        expanded += 1;
        if expanded > MAX_EXPANSIONS {
            eprintln!(
                "soundscape {name} nests more than {MAX_EXPANSIONS} soundscapes; the rest are left out"
            );
            break;
        }
        for rule in &s.rules {
            match rule {
                ScapeRule::Nested { name, volume } if depth < MAX_NESTING => {
                    stack.push((name.clone(), scale * volume, depth + 1));
                }
                ScapeRule::Nested { .. } => {}
                _ => out.push((rule.clone(), scale)),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_soundscapes_cannot_fan_out() {
        // A soundscape nesting itself eight times over, with one looping sound: eight to the eighth expansions uncapped.
        let mut text = String::from(r#""Loop" { "playlooping" { "wave" "a.wav" }"#);
        for _ in 0..8 {
            text.push_str(r#" "playsoundscape" { "name" "Loop" "volume" "1" }"#);
        }
        text.push('}');
        let scapes = read_soundscapes(text.as_bytes()).unwrap();
        let rules = expand(
            |n| scapes.iter().find(|s| s.name.eq_ignore_ascii_case(n)),
            "loop",
        );
        assert_eq!(rules.len(), MAX_EXPANSIONS);
    }
}
