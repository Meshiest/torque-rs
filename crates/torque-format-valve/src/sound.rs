//! Sounds: wave files (RIFF WAVE: PCM of 8 or 16 bits and Microsoft ADPCM, with the `cue ` and `smpl` chunks that mark a
//! looping sound's loop) and MP3 files decoded to 16-bit samples; soundscripts (`scripts/game_sounds*.txt`: named entries with a
//! channel, volume, pitch, sound level and one or more waves); soundscapes (`scripts/soundscapes*.txt`: looping and random sounds
//! and nested soundscapes); and the sound level's falloff with distance.
//!
//! **Wave files** (Microsoft's RIFF and multimedia format specifications): `RIFF`, `WAVE`, then chunks of a four-letter id and
//! a length. `fmt `: the format tag (1 PCM, 2 ADPCM), channels, rate, block alignment, bits; ADPCM adds the samples a block and
//! its predictor coefficients. ADPCM blocks start with each channel's predictor, step and two samples, then 4-bit codes (high
//! half first; channels alternate) each adapting the step by a fixed table. `cue `: points (the first's sample offset starts the
//! loop: a cued sound loops from there to its end, as the engine plays cued waves); `smpl`: loops (start and end frames).
//!
//! **Names.** A wave name may start with characters that tell the engine how to mix it (`*` streamed, `#` past the effects, `@`
//! not directional, `)` spatial stereo, `<`, `>`, `^`, `}`, `!` sentence, `?` voice; public SDK `soundchars.h`): they are taken off
//! before the file is looked up under `sound/`.
//!
//! **Sound levels** (public SDK `soundflags.h`): a level in dB (`SNDLVL_75dB`, `SNDLVL_NORM` 75, ...) gives an attenuation
//! `20 / (level - 50)` (4 at 50 dB or less; 0 for `SNDLVL_NONE`: heard everywhere). [`falloff`] takes the gain at a distance as
//! one less the distance times the attenuation over the engine's nominal clip distance of 1000 units: an approximation of the
//! engine's curve (its mixer is not public), which reaches silence at `1000 / attenuation` units.
//!
//! Every reader is capped (file size, channels, rate, length) and gives a value or an error, never a panic.

use crate::kv::{self, Pair, Value};
use crate::read::{Error, Result, cap, i16_at, invalid, slice, u16_at, u32_at};

/// The largest sound file read.
pub const MAX_FILE_BYTES: u64 = 64 << 20;
/// The longest sound decoded (frames: ten minutes at 48 kHz).
pub const MAX_FRAMES: usize = 48_000 * 600;
/// The highest rate read.
pub const MAX_RATE: u32 = 96_000;
/// The largest soundscript or soundscape file read.
pub const MAX_SCRIPT_BYTES: u64 = 8 << 20;
/// The most waves one entry lists.
const MAX_WAVES: usize = 256;

/// A decoded sound: interleaved 16-bit samples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wave {
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<i16>,
    /// The frame a looping sound loops back to (its end being the sound's end), when the file marks one.
    pub loop_start: Option<usize>,
}

impl Wave {
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels.max(1))
    }

    /// Seconds long.
    pub fn duration(&self) -> f32 {
        self.frames() as f32 / self.rate.max(1) as f32
    }
}

const ADAPTATION: [i32; 16] = [
    230, 230, 230, 230, 307, 409, 512, 614, 768, 614, 512, 409, 307, 230, 230, 230,
];

/// Reads a wave file.
pub fn read_wav(b: &[u8]) -> Result<Wave> {
    cap("the sound file's size", b.len() as u64, MAX_FILE_BYTES)?;
    if slice(b, 0, 4, "the RIFF header")? != b"RIFF"
        || slice(b, 8, 4, "the RIFF header")? != b"WAVE"
    {
        return invalid("not a RIFF WAVE file");
    }
    let mut at = 12usize;
    let mut fmt: Option<&[u8]> = None;
    let mut data: Option<&[u8]> = None;
    let mut loop_start = None;
    // Chunks until the end (a chunk past the end is cut to what there is; each is padded to an even length).
    for _ in 0..4096 {
        if at + 8 > b.len() {
            break;
        }
        let id = slice(b, at, 4, "a chunk")?;
        let len = u32_at(b, at + 4, "a chunk")? as usize;
        let start = at + 8;
        let end = start.saturating_add(len).min(b.len());
        let body = &b[start..end];
        match id {
            b"fmt " => fmt = Some(body),
            b"data" => data = Some(body),
            b"cue " => {
                if u32_at(body, 0, "a cue chunk").unwrap_or(0) > 0 {
                    loop_start = u32_at(body, 4 + 20, "a cue point").ok().map(|s| s as usize);
                }
            }
            b"smpl" if u32_at(body, 28, "a sample chunk").unwrap_or(0) > 0 => {
                loop_start = u32_at(body, 36 + 8, "a sample loop")
                    .ok()
                    .map(|s| s as usize)
                    .or(loop_start);
            }
            _ => {}
        }
        at = start.saturating_add(len).saturating_add(len & 1);
    }
    let fmt = fmt.ok_or(Error::Invalid("a wave file without a fmt chunk".into()))?;
    let data = data.ok_or(Error::Invalid("a wave file without a data chunk".into()))?;
    let tag = u16_at(fmt, 0, "the format")?;
    let channels = u16_at(fmt, 2, "the format")?;
    let rate = u32_at(fmt, 4, "the format")?;
    let align = usize::from(u16_at(fmt, 12, "the format")?);
    let bits = u16_at(fmt, 14, "the format")?;
    if !(1..=2).contains(&channels) {
        return invalid(format!("a wave of {channels} channels"));
    }
    if !(1000..=MAX_RATE).contains(&rate) {
        return invalid(format!("a wave at {rate} Hz"));
    }
    let ch = usize::from(channels);
    let samples = match (tag, bits) {
        (1, 8) => {
            cap(
                "the wave's frames",
                (data.len() / ch) as u64,
                MAX_FRAMES as u64,
            )?;
            data.iter().map(|&s| (i16::from(s) - 128) << 8).collect()
        }
        (1, 16) => {
            cap(
                "the wave's frames",
                (data.len() / 2 / ch) as u64,
                MAX_FRAMES as u64,
            )?;
            data.as_chunks::<2>()
                .0
                .iter()
                .map(|s| i16::from_le_bytes(*s))
                .collect()
        }
        (2, 4) => decode_adpcm(fmt, data, ch, align)?,
        _ => return invalid(format!("wave format {tag} at {bits} bits is not read")),
    };
    let frames = samples.len() / ch;
    Ok(Wave {
        rate,
        channels,
        samples,
        loop_start: loop_start.filter(|&s| s < frames),
    })
}

/// Microsoft ADPCM blocks into samples.
fn decode_adpcm(fmt: &[u8], data: &[u8], ch: usize, align: usize) -> Result<Vec<i16>> {
    let per_block = usize::from(u16_at(fmt, 18, "the ADPCM format")?);
    let ncoef = usize::from(u16_at(fmt, 20, "the ADPCM format")?);
    if ncoef == 0 || ncoef > 256 {
        return invalid(format!("an ADPCM format with {ncoef} coefficients"));
    }
    let coef: Vec<(i32, i32)> = (0..ncoef)
        .map(|k| {
            Ok((
                i32::from(i16_at(fmt, 22 + k * 4, "an ADPCM coefficient")?),
                i32::from(i16_at(fmt, 24 + k * 4, "an ADPCM coefficient")?),
            ))
        })
        .collect::<Result<_>>()?;
    let header = 7 * ch;
    if align <= header || per_block < 2 {
        return invalid("an ADPCM block too small");
    }
    // The samples a full block holds: two from its header, two a byte after it per channel.
    let fits = 2 + (align - header) * 2 / ch;
    let per_block = per_block.min(fits);
    let blocks = data.len().div_ceil(align);
    cap(
        "the wave's frames",
        (blocks * per_block) as u64,
        MAX_FRAMES as u64,
    )?;
    let mut out = Vec::with_capacity(blocks * per_block * ch);
    for block in data.chunks(align) {
        if block.len() < header {
            break;
        }
        let mut pred = [0usize; 2];
        let mut delta = [0i32; 2];
        let mut s1 = [0i32; 2];
        let mut s2 = [0i32; 2];
        for c in 0..ch {
            pred[c] = usize::from(block[c]);
            if pred[c] >= ncoef {
                return invalid(format!(
                    "an ADPCM block names coefficient {} of {ncoef}",
                    pred[c]
                ));
            }
            // The step: as decoders keep it, 16 to 32767.
            delta[c] = i32::from(i16::from_le_bytes([
                block[ch + c * 2],
                block[ch + c * 2 + 1],
            ]))
            .clamp(16, 32767);
            s1[c] = i32::from(i16::from_le_bytes([
                block[3 * ch + c * 2],
                block[3 * ch + c * 2 + 1],
            ]));
            s2[c] = i32::from(i16::from_le_bytes([
                block[5 * ch + c * 2],
                block[5 * ch + c * 2 + 1],
            ]));
        }
        out.extend(s2[..ch].iter().map(|&v| v as i16));
        out.extend(s1[..ch].iter().map(|&v| v as i16));
        let mut made = 2;
        let mut c = 0;
        'codes: for &byte in &block[header..] {
            for nibble in [byte >> 4, byte & 0xf] {
                if made >= per_block {
                    break 'codes;
                }
                let signed = if nibble & 8 != 0 {
                    i32::from(nibble) - 16
                } else {
                    i32::from(nibble)
                };
                let (c1, c2) = coef[pred[c]];
                // In 64 bits: hostile coefficients and steps cannot overflow.
                let predicted =
                    (i64::from(s1[c]) * i64::from(c1) + i64::from(s2[c]) * i64::from(c2)) >> 8;
                let sample = (predicted + i64::from(signed) * i64::from(delta[c]))
                    .clamp(-32768, 32767) as i32;
                out.push(sample as i16);
                s2[c] = s1[c];
                s1[c] = sample;
                delta[c] = ((i64::from(ADAPTATION[usize::from(nibble)]) * i64::from(delta[c])) >> 8)
                    .clamp(16, 32767) as i32;
                c += 1;
                if c == ch {
                    c = 0;
                    made += 1;
                }
            }
        }
    }
    Ok(out)
}

/// Reads an MP3 file (MPEG layer III through `nanomp3`, a pure-Rust port of minimp3: tags skipped, the encoder's delay and
/// padding trimmed), mono or stereo.
pub fn read_mp3(b: &[u8]) -> Result<Wave> {
    cap("the sound file's size", b.len() as u64, MAX_FILE_BYTES)?;
    let mut reader = nanomp3::SliceReader::<i16>::new(b);
    let mut out: Vec<i16> = Vec::new();
    loop {
        match reader.read_frame() {
            Ok(Some(frame)) => out.extend_from_slice(frame),
            Ok(None) => break,
            Err(e) => return invalid(format!("an MP3 file that does not decode: {e:?}")),
        }
        let channels = reader.channels().map_or(1, |c| usize::from(c.num()));
        cap(
            "the sound's frames",
            (out.len() / channels.max(1)) as u64,
            MAX_FRAMES as u64,
        )?;
    }
    let channels = reader.channels().map_or(0, |c| u16::from(c.num()));
    let rate = reader.sample_rate();
    if out.is_empty() || !(1..=2).contains(&channels) || !(1000..=MAX_RATE).contains(&rate) {
        return invalid(format!(
            "an MP3 of {channels} channels at {rate} Hz with {} samples",
            out.len()
        ));
    }
    Ok(Wave {
        rate,
        channels,
        samples: out,
        loop_start: None,
    })
}

/// Reads a sound file by its name's extension (`.wav` or `.mp3`).
pub fn read_sound(path: &str, b: &[u8]) -> Result<Wave> {
    if path.to_ascii_lowercase().ends_with(".mp3") {
        read_mp3(b)
    } else {
        read_wav(b)
    }
}

/// A wave name as its file's path: the mixing characters taken off its start, `sound/` before it, the slashes and case
/// normalized.
pub fn wave_path(name: &str) -> String {
    let n = name.trim().trim_start_matches([
        '*', '#', '@', ')', '(', '<', '>', '^', '}', '!', '?', '$', '&', '~', '`', '+', '%',
    ]);
    let n = n.replace('\\', "/").to_ascii_lowercase();
    let n = n.trim_start_matches('/');
    let n = n.strip_prefix("sound/").unwrap_or(n);
    format!("sound/{n}")
}

/// A range (`"0.5"`, `"0.4, 0.6"`, or a named constant) as its low and high ends.
fn range(s: &str, named: &dyn Fn(&str) -> Option<f32>) -> Option<(f32, f32)> {
    let mut v = s.split(',').map(|t| {
        let t = t.trim();
        t.parse::<f32>().ok().or_else(|| named(t))
    });
    let a = v.next()??;
    let b = v.next().flatten().unwrap_or(a);
    Some((a.min(b), a.max(b)))
}

fn volume_name(s: &str) -> Option<f32> {
    s.eq_ignore_ascii_case("VOL_NORM").then_some(1.0)
}

fn pitch_name(s: &str) -> Option<f32> {
    match s.to_ascii_uppercase().as_str() {
        "PITCH_NORM" => Some(100.0),
        "PITCH_LOW" => Some(95.0),
        "PITCH_HIGH" => Some(120.0),
        _ => None,
    }
}

/// A sound level's name or number in dB (`SNDLVL_75dB`, `SNDLVL_NORM`, `ATTN_STATIC`, `85`).
pub fn sound_level(s: &str) -> Option<f32> {
    let u = s.trim().to_ascii_uppercase();
    if let Ok(v) = u.parse::<f32>() {
        return Some(v);
    }
    let named = match u.as_str() {
        "SNDLVL_NONE" => Some(0.0),
        "SNDLVL_IDLE" => Some(60.0),
        "SNDLVL_STATIC" => Some(66.0),
        "SNDLVL_NORM" => Some(75.0),
        "SNDLVL_TALKING" => Some(80.0),
        "SNDLVL_GUNFIRE" => Some(140.0),
        // The older attenuations, as levels (ATTN_TO_SNDLVL: 50 + 20 / attenuation).
        "ATTN_NONE" => Some(0.0),
        "ATTN_NORM" => Some(75.0),
        "ATTN_IDLE" => Some(60.0),
        "ATTN_STATIC" => Some(66.0),
        "ATTN_RICOCHET" => Some(63.3),
        "ATTN_GUNFIRE" => Some(124.0),
        _ => None,
    };
    named.or_else(|| {
        u.strip_prefix("SNDLVL_")
            .and_then(|r| r.strip_suffix("DB"))
            .and_then(|n| n.parse().ok())
    })
}

/// The gain of a sound of `level` dB at `distance` units (module docs; 1 within reach of a level of 0, heard everywhere).
pub fn falloff(level: f32, distance: f32) -> f32 {
    if level <= 0.0 {
        return 1.0;
    }
    let attn = if level > 50.0 {
        20.0 / (level - 50.0)
    } else {
        4.0
    };
    (1.0 - distance.max(0.0) * attn / 1000.0).clamp(0.0, 1.0)
}

/// A soundscript entry.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundEntry {
    pub name: String,
    pub channel: String,
    pub volume: (f32, f32),
    /// Percent (100 the sound as it is).
    pub pitch: (f32, f32),
    /// dB.
    pub level: f32,
    /// The waves' names (one picked at random each time).
    pub waves: Vec<String>,
}

/// The waves of an entry's `wave` and `rndwave` keys.
fn waves_of(block: &[Pair]) -> Vec<String> {
    let mut out = Vec::new();
    for p in block {
        if p.key.eq_ignore_ascii_case("wave") {
            if let Some(w) = p.str() {
                out.push(w.to_string());
            }
        } else if p.key.eq_ignore_ascii_case("rndwave")
            && let Some(b) = p.block()
        {
            out.extend(
                b.iter()
                    .filter(|q| q.key.eq_ignore_ascii_case("wave"))
                    .filter_map(|q| q.str().map(str::to_string)),
            );
        }
    }
    out.truncate(MAX_WAVES);
    out
}

/// Reads a soundscript file's entries.
pub fn read_soundscript(text: &[u8]) -> Result<Vec<SoundEntry>> {
    cap(
        "the soundscript's size",
        text.len() as u64,
        MAX_SCRIPT_BYTES,
    )?;
    Ok(kv::parse(text)?
        .iter()
        .filter_map(|p| {
            let b = p.block()?;
            let get = |k: &str| kv::find_str(b, k);
            Some(SoundEntry {
                name: p.key.clone(),
                channel: get("channel").unwrap_or("CHAN_AUTO").to_string(),
                volume: get("volume")
                    .and_then(|v| range(v, &volume_name))
                    .unwrap_or((1.0, 1.0)),
                pitch: get("pitch")
                    .and_then(|v| range(v, &pitch_name))
                    .unwrap_or((100.0, 100.0)),
                level: get("soundlevel")
                    .or(get("attenuation"))
                    .and_then(sound_level)
                    .unwrap_or(75.0),
                waves: waves_of(b),
            })
        })
        .collect())
}

/// The files a manifest lists (`precache_file` and `preload_file` values of its first block).
pub fn read_manifest(text: &[u8]) -> Result<Vec<String>> {
    cap("the manifest's size", text.len() as u64, MAX_SCRIPT_BYTES)?;
    let kv = kv::parse(text)?;
    Ok(kv
        .first()
        .and_then(Pair::block)
        .map(|b| {
            b.iter()
                .filter(|p| {
                    let k = p.key.to_ascii_lowercase();
                    k == "precache_file" || k == "preload_file" || k == "file"
                })
                .filter_map(|p| p.str().map(|s| s.replace('\\', "/").to_ascii_lowercase()))
                .collect()
        })
        .unwrap_or_default())
}

/// One rule of a soundscape.
#[derive(Debug, Clone, PartialEq)]
pub enum ScapeRule {
    /// A sound played over and over.
    Looping {
        waves: Vec<String>,
        volume: f32,
        pitch: f32,
        level: f32,
        /// The position the map's soundscape entity names (`position N`), or none: everywhere.
        position: Option<usize>,
    },
    /// Sounds played at random times.
    Random {
        waves: Vec<String>,
        time: (f32, f32),
        volume: (f32, f32),
        pitch: (f32, f32),
        level: f32,
        position: Option<usize>,
    },
    /// Another soundscape inside this one, at a volume.
    Nested { name: String, volume: f32 },
}

/// A soundscape: its rules.
#[derive(Debug, Clone, PartialEq)]
pub struct Soundscape {
    pub name: String,
    pub dsp: i32,
    pub rules: Vec<ScapeRule>,
}

/// Reads a soundscape file.
pub fn read_soundscapes(text: &[u8]) -> Result<Vec<Soundscape>> {
    cap(
        "the soundscape file's size",
        text.len() as u64,
        MAX_SCRIPT_BYTES,
    )?;
    Ok(kv::parse(text)?
        .iter()
        .filter_map(|p| {
            let b = p.block()?;
            let mut rules = Vec::new();
            for r in b {
                let Value::Block(rb) = &r.value else { continue };
                let get = |k: &str| kv::find_str(rb, k);
                let position = get("position").and_then(|v| v.trim().parse().ok());
                let level = get("soundlevel").and_then(sound_level).unwrap_or(0.0);
                match r.key.to_ascii_lowercase().as_str() {
                    "playlooping" => rules.push(ScapeRule::Looping {
                        waves: waves_of(rb),
                        volume: get("volume")
                            .and_then(|v| range(v, &volume_name))
                            .map_or(1.0, |r| r.1),
                        pitch: get("pitch")
                            .and_then(|v| range(v, &pitch_name))
                            .map_or(100.0, |r| r.1),
                        level,
                        position,
                    }),
                    "playrandom" => rules.push(ScapeRule::Random {
                        waves: waves_of(rb),
                        time: get("time")
                            .and_then(|v| range(v, &|_| None))
                            .unwrap_or((5.0, 10.0)),
                        volume: get("volume")
                            .and_then(|v| range(v, &volume_name))
                            .unwrap_or((1.0, 1.0)),
                        pitch: get("pitch")
                            .and_then(|v| range(v, &pitch_name))
                            .unwrap_or((100.0, 100.0)),
                        level,
                        position,
                    }),
                    "playsoundscape" => {
                        if let Some(n) = get("name") {
                            rules.push(ScapeRule::Nested {
                                name: n.to_string(),
                                volume: get("volume")
                                    .and_then(|v| v.trim().parse().ok())
                                    .unwrap_or(1.0),
                            });
                        }
                    }
                    _ => {}
                }
            }
            Some(Soundscape {
                name: p.key.clone(),
                dsp: kv::find_str(b, "dsp")
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0),
                rules,
            })
        })
        .collect())
}

/// A wave file of `samples` (16-bit, interleaved) for tests and fuzz seeds, with a cue point at `cue` when given.
pub fn write_wav(rate: u32, channels: u16, samples: &[i16], cue: Option<u32>) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF\0\0\0\0WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&channels.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
    b.extend_from_slice(&(channels * 2).to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&((samples.len() * 2) as u32).to_le_bytes());
    for s in samples {
        b.extend_from_slice(&s.to_le_bytes());
    }
    if let Some(c) = cue {
        b.extend_from_slice(b"cue ");
        b.extend_from_slice(&28u32.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        for v in [1u32, c, u32::from_le_bytes(*b"data"), 0, 0, c] {
            b.extend_from_slice(&v.to_le_bytes());
        }
    }
    let len = (b.len() - 8) as u32;
    b[4..8].copy_from_slice(&len.to_le_bytes());
    b
}

/// A mono Microsoft ADPCM wave file of one block (predictor 0, step 16, start samples 0) whose codes are `codes`, for tests.
pub fn write_adpcm(codes: &[u8]) -> Vec<u8> {
    let coefs: [(i16, i16); 7] = [
        (256, 0),
        (512, -256),
        (0, 0),
        (192, 64),
        (240, 0),
        (460, -208),
        (392, -232),
    ];
    let mut fmt = Vec::new();
    let align = 7 + codes.len() as u16;
    for v in [2u16, 1] {
        fmt.extend_from_slice(&v.to_le_bytes());
    }
    fmt.extend_from_slice(&22050u32.to_le_bytes());
    fmt.extend_from_slice(&11025u32.to_le_bytes());
    for v in [align, 4, 32, 2 + 2 * codes.len() as u16, 7] {
        fmt.extend_from_slice(&v.to_le_bytes());
    }
    for (a, c) in coefs {
        fmt.extend_from_slice(&a.to_le_bytes());
        fmt.extend_from_slice(&c.to_le_bytes());
    }
    let mut block = vec![0u8];
    block.extend_from_slice(&16i16.to_le_bytes());
    block.extend_from_slice(&[0, 0, 0, 0]);
    block.extend_from_slice(codes);
    let mut b = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
    b.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    b.extend_from_slice(&fmt);
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(block.len() as u32).to_le_bytes());
    b.extend_from_slice(&block);
    let len = (b.len() - 8) as u32;
    b[4..8].copy_from_slice(&len.to_le_bytes());
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_pcm_with_a_loop() {
        let w = read_wav(&write_wav(22050, 2, &[1, -1, 300, -300, 7, 8], Some(1))).unwrap();
        assert_eq!((w.rate, w.channels, w.frames()), (22050, 2, 3));
        assert_eq!(w.samples, vec![1, -1, 300, -300, 7, 8]);
        assert_eq!(w.loop_start, Some(1));
        let w = read_wav(&write_wav(11025, 1, &[5], Some(9))).unwrap();
        assert_eq!(w.loop_start, None, "a cue past the end is no loop");
    }

    #[test]
    fn decodes_adpcm() {
        // Code 1 three times: the step grows from 16 (16, then 16 x 230 / 256 floored at 16).
        let w = read_wav(&write_adpcm(&[0x11, 0x10])).unwrap();
        assert_eq!(w.samples[..2], [0, 0]);
        assert_eq!(w.samples[2], 16);
        assert!(w.samples[3] > w.samples[2]);
        assert_eq!(w.samples.len(), 6);
    }

    #[test]
    fn adpcm_with_hostile_coefficients_and_steps_stays_in_range() {
        let mut b = write_adpcm(&[0x77, 0x77, 0x88, 0x88, 0x7f, 0xf7]);
        // The first coefficient pair at its extremes, the step at its extreme.
        let fmt = 20;
        b[fmt + 22..fmt + 26].copy_from_slice(&[0xff, 0x7f, 0x00, 0x80]);
        let block = b.len() - 6 - 7;
        b[block + 1..block + 3].copy_from_slice(&i16::MAX.to_le_bytes());
        b[block + 3..block + 5].copy_from_slice(&i16::MAX.to_le_bytes());
        b[block + 5..block + 7].copy_from_slice(&i16::MIN.to_le_bytes());
        let w = read_wav(&b).unwrap();
        assert_eq!(w.samples.len(), 14);
        // A predictor past the coefficients is refused.
        let mut b = write_adpcm(&[0x11]);
        let block = b.len() - 1 - 7;
        b[block] = 200;
        assert!(read_wav(&b).is_err());
    }

    #[test]
    fn refuses_hostile_waves() {
        assert!(read_wav(b"RIFF").is_err());
        let mut b = write_wav(22050, 1, &[1, 2], None);
        b[22..24].copy_from_slice(&9u16.to_le_bytes());
        assert!(read_wav(&b).is_err(), "nine channels");
        let mut b = write_wav(22050, 1, &[1, 2], None);
        b[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(read_wav(&b).is_err(), "a rate past the cap");
        let mut b = write_adpcm(&[0x11]);
        let at = b.len() - 8;
        b.truncate(at);
        assert!(read_wav(&b).is_ok() || read_wav(&b).is_err());
        assert!(read_mp3(b"not an mp3 at all").is_err());
        assert!(read_mp3(&[0xff; 4096]).is_err());
    }

    #[test]
    fn names_levels_and_falloff() {
        assert_eq!(
            wave_path("*#weapons\\Rifle\\rifle-1.wav"),
            "sound/weapons/rifle/rifle-1.wav"
        );
        assert_eq!(wave_path(")ambient/wind.wav"), "sound/ambient/wind.wav");
        assert_eq!(sound_level("SNDLVL_85dB"), Some(85.0));
        assert_eq!(sound_level("SNDLVL_NORM"), Some(75.0));
        assert_eq!(falloff(0.0, 1e6), 1.0);
        assert_eq!(falloff(75.0, 0.0), 1.0);
        assert_eq!(falloff(75.0, 1250.0), 0.0);
        assert!((falloff(75.0, 625.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn reads_scripts_and_scapes() {
        let s = read_soundscript(
            br#""Rifle.Clipout" { "channel" "CHAN_ITEM" "volume" "0.7,0.9" "pitch" "PITCH_NORM" "soundlevel" "SNDLVL_65dB" "wave" "weapons/rifle/rifle_clipout.wav" }
                "Ambient.Wind" { "volume" "VOL_NORM" "rndwave" { "wave" "a.wav" "wave" "b.wav" } }"#,
        )
        .unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].volume, (0.7, 0.9));
        assert_eq!(s[0].pitch, (100.0, 100.0));
        assert_eq!(s[0].level, 65.0);
        assert_eq!(s[1].waves, vec!["a.wav", "b.wav"]);
        let m = read_manifest(br#"game_sounds_manifest { "precache_file" "scripts/game_sounds.txt" "preload_file" "scripts\Game_Sounds_Weapons.txt" }"#)
            .unwrap();
        assert_eq!(
            m,
            vec!["scripts/game_sounds.txt", "scripts/game_sounds_weapons.txt"]
        );
        let sc = read_soundscapes(
            br#""Town.Outside" { "dsp" "1" "playlooping" { "volume" "0.4" "wave" "ambient/wind.wav" "pitch" "100" }
               "playrandom" { "time" "5,10" "volume" "0.3,0.5" "rndwave" { "wave" "bird1.wav" } "position" "0" }
               "playsoundscape" { "name" "Generic.Outside" "volume" "0.5" } }"#,
        )
        .unwrap();
        assert_eq!(sc[0].dsp, 1);
        assert_eq!(sc[0].rules.len(), 3);
        assert!(matches!(
            &sc[0].rules[1],
            ScapeRule::Random {
                time: (5.0, 10.0),
                position: Some(0),
                ..
            }
        ));
        assert!(matches!(&sc[0].rules[2], ScapeRule::Nested { volume, .. } if *volume == 0.5));
    }
}
