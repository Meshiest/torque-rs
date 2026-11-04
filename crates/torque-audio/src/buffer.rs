//! `AudioBuffer`: a sound file as OpenAL holds it, found by name through the resource manager, and `alxGetWaveLen`.
//!
//! - **Find** (`AudioBuffer::find`): the name as given; when no file of that name is there and the name ends in `.wav` (any case),
//!   the same name with `ogg` for its last three letters. Names are the resource manager's (the caller's [`SoundFiles`]). A file of
//!   another extension gives "no buffer", and the audio scripts are expected to name `.wav` and `.ogg` files only.
//! - **Load** (`getALBuffer`): by the found name's extension, `.wav` through [`crate::wav::read_wav`] and `.ogg` through
//!   [`crate::ogg::read_ogg`]; any other extension, or a failed read, gives no OpenAL buffer (the failure is kept: a read has no other
//!   effect but a console line the cache's [`WavRules`] may print for it).
//! - **`alxGetWaveLen(file)`**: no file is 0; a file with no buffer prints `alxGetWaveLen: invalid buffer` (an error) and is 0; else
//!   `size * 8000 / (channels * bits * rate)` cut toward zero (so 1,015.56 ms is 1,015), the divisor a 32-bit product, over the
//!   buffer's OpenAL size, bits, channels and rate.

use std::collections::HashMap;
use std::sync::Arc;

use crate::wav::{Pcm, TgeWav, WavRules, read_wav_with};
use crate::{MAX_CACHED_BUFFERS, MAX_SOUND_FILE_BYTES, MAX_TOTAL_PCM_BYTES, ogg};

/// A loaded sound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Buffer {
    /// The name it was found under (the file's).
    pub name: Vec<u8>,
    pub pcm: Pcm,
}

impl Buffer {
    /// `alxGetWaveLen` of a loaded buffer.
    pub fn wave_len_ms(&self) -> i32 {
        wave_len(
            self.pcm.size,
            self.pcm.bits,
            self.pcm.channels,
            self.pcm.rate,
        )
    }

    /// Seconds of sound at its own rate.
    pub fn seconds(&self) -> f64 {
        self.pcm.frames() as f64 / f64::from(self.pcm.rate.max(1))
    }
}

/// The `alxGetWaveLen` formula over OpenAL's buffer values (0 when one of them is 0: "invalid buffer").
pub fn wave_len(size: usize, bits: u16, channels: u16, rate: u32) -> i32 {
    if rate == 0 || bits == 0 || channels == 0 {
        return 0;
    }
    let divisor = i32::from(channels)
        .wrapping_mul(i32::from(bits))
        .wrapping_mul(rate as i32);
    let ms = ((size as i32) as f64 * 8000.0) / f64::from(divisor);
    ms.trunc() as i64 as i32
}

/// The files as the resource manager sees them, and the budget that pays for reading and decoding them.
pub trait SoundFiles {
    /// The file's bytes (at most [`MAX_SOUND_FILE_BYTES`] are asked for), or None when there is no file of that name.
    fn read(&mut self, name: &[u8]) -> Option<Vec<u8>>;

    /// Charges `units` of work (a byte read or decoded is one): false when the budget cannot pay (the caller stops; the host's
    /// script ends with its step limit). The default pays everything.
    fn charge(&mut self, units: u64) -> bool {
        let _ = units;
        true
    }
}

/// What a lookup found.
#[derive(Debug, Clone)]
pub enum Found {
    /// No file (`alxGetWaveLen` 0, silently).
    Missing,
    /// A file that gives no buffer (`alxGetWaveLen: invalid buffer`).
    Failed(String),
    Loaded(Arc<Buffer>),
    /// The work budget could not pay for the read or the decode (nothing is cached; the script's run ends).
    OverBudget,
}

/// The buffers loaded so far, by name, and the lines their reads printed.
///
/// **Bounded** (sound files are hostile input): at most [`MAX_CACHED_BUFFERS`] names; at most [`MAX_TOTAL_PCM_BYTES`] of samples held
/// by the cache, whoever else holds them too (sources and the profiles' preloads only ever take the cache's buffers). A new buffer
/// that would pass the budget first evicts the least recently used buffers nobody else holds, and is refused ([`Found::Failed`],
/// counted in [`BufferCache::refused`], never cached) when that is not enough. A full name table gives a loaded buffer a slot by evicting
/// the oldest negative answer (or unheld buffer), so every buffer handed out is counted. Keys are [`cache_key`]: one path tried each.
#[derive(Debug)]
pub struct BufferCache {
    entries: HashMap<Vec<u8>, (Found, u64)>,
    clock: u64,
    /// The samples' budget ([`MAX_TOTAL_PCM_BYTES`]).
    pub budget: usize,
    /// Bytes of samples the cached buffers hold.
    pub pcm_bytes: usize,
    /// Buffers evicted for the budget, and loads refused by it.
    pub evicted: u64,
    pub refused: u64,
    /// Console lines of the reads, for the host to print (`(error, text)`).
    pub messages: Vec<(bool, String)>,
    /// The WAV reader's rules ([`TgeWav`] by default).
    pub wav_rules: Arc<dyn WavRules>,
}

impl Default for BufferCache {
    fn default() -> BufferCache {
        BufferCache {
            entries: HashMap::new(),
            clock: 0,
            budget: MAX_TOTAL_PCM_BYTES,
            pcm_bytes: 0,
            evicted: 0,
            refused: 0,
            messages: Vec::new(),
            wav_rules: Arc::new(TgeWav),
        }
    }
}

/// The cache's key of a name: the name as the file host is asked for it, ASCII case folded (the host's only folding), so every key
/// is exactly one path tried and no spelling can stand for another.
pub fn cache_key(name: &[u8]) -> Vec<u8> {
    name.to_ascii_lowercase()
}

fn ends_with_ignore_case(name: &[u8], ext: &[u8]) -> bool {
    name.len() >= ext.len() && name[name.len() - ext.len()..].eq_ignore_ascii_case(ext)
}

/// The `.ogg` twin of a `.wav` name (its last three letters replaced by `ogg`).
pub fn ogg_twin(name: &[u8]) -> Option<Vec<u8>> {
    if name.len() > 3 && ends_with_ignore_case(name, b".wav") {
        let mut n = name.to_vec();
        let at = n.len() - 3;
        n[at..].copy_from_slice(b"ogg");
        Some(n)
    } else {
        None
    }
}

/// Decodes `bytes` by `name`'s extension, a `.wav` file with `rules`.
pub fn decode(
    name: &[u8],
    bytes: &[u8],
    rules: &dyn WavRules,
    messages: &mut Vec<(bool, String)>,
) -> Result<Pcm, String> {
    if bytes.len() > MAX_SOUND_FILE_BYTES {
        return Err(format!(
            "{} bytes (the cap is {MAX_SOUND_FILE_BYTES})",
            bytes.len()
        ));
    }
    if ends_with_ignore_case(name, b".wav") {
        let (pcm, report) = read_wav_with(bytes, rules);
        if report.smpl_stops != 0 {
            let text = String::from_utf8_lossy(name);
            let (dir, file) = text.rsplit_once('/').unwrap_or(("", &text));
            messages.extend(rules.smpl_stop_line(dir, file).map(|line| (true, line)));
        }
        pcm.map_err(|e| e.to_string())
    } else if ends_with_ignore_case(name, b".ogg") {
        ogg::read_ogg(bytes).map_err(|e| e.to_string())
    } else {
        Err("not a .wav or .ogg file".to_string())
    }
}

impl BufferCache {
    /// Finds and loads `name` (see the module).
    pub fn find(&mut self, files: &mut dyn SoundFiles, name: &[u8]) -> Found {
        if name.is_empty() {
            return Found::Missing;
        }
        let key = cache_key(name);
        self.clock += 1;
        let now = self.clock;
        if let Some((f, used)) = self.entries.get_mut(&key) {
            *used = now;
            return f.clone();
        }
        if !files.charge(name.len() as u64) {
            return Found::OverBudget;
        }
        let mut found = Found::Missing;
        let mut candidates = vec![name.to_vec()];
        candidates.extend(ogg_twin(name));
        for candidate in candidates {
            if let Some(bytes) = files.read(&candidate) {
                if !files.charge(bytes.len() as u64) {
                    return Found::OverBudget;
                }
                found = match decode(&candidate, &bytes, &*self.wav_rules, &mut self.messages) {
                    Ok(pcm) => {
                        if !files.charge(pcm.data.len() as u64) {
                            return Found::OverBudget;
                        }
                        // A refusal of the memory budget is not cached: the next request tries again.
                        let Some(b) = self.admit(key.clone(), candidate, pcm, now) else {
                            self.refused += 1;
                            return Found::Failed("over the sound memory budget".to_string());
                        };
                        return Found::Loaded(b);
                    }
                    Err(e) => Found::Failed(e),
                };
                break;
            }
        }
        // A missing file or one that does not decode (the same every time): kept when a slot is free or an older such answer
        // gives up its slot; never at a loaded buffer's cost.
        if self.entries.len() >= MAX_CACHED_BUFFERS && !self.evict_one(false) {
            return found;
        }
        self.entries.insert(key, (found.clone(), now));
        found
    }

    /// Frees one slot: the least recently used answer that is not a buffer, else (`buffers`) the least recently used buffer nobody
    /// else holds. False when there is none.
    fn evict_one(&mut self, buffers: bool) -> bool {
        let pick = |want_loaded: bool, e: &HashMap<Vec<u8>, (Found, u64)>| {
            e.iter()
                .filter(|(_, (f, _))| match f {
                    Found::Loaded(b) => want_loaded && Arc::strong_count(b) == 1,
                    _ => !want_loaded,
                })
                .min_by_key(|(_, (_, used))| *used)
                .map(|(k, _)| k.clone())
        };
        let victim = pick(false, &self.entries).or_else(|| {
            if buffers {
                pick(true, &self.entries)
            } else {
                None
            }
        });
        let Some(victim) = victim else {
            return false;
        };
        if let Some((Found::Loaded(b), _)) = self.entries.remove(&victim) {
            self.pcm_bytes -= b.pcm.data.len();
            self.evicted += 1;
        }
        true
    }

    /// A decoded buffer into the cache within both caps (a slot and the samples' budget), evicting the least recently used buffers
    /// nobody else holds; None when they are not enough. Every buffer the cache hands out is in it and counted in `pcm_bytes`.
    fn admit(&mut self, key: Vec<u8>, name: Vec<u8>, pcm: Pcm, now: u64) -> Option<Arc<Buffer>> {
        let bytes = pcm.data.len();
        if bytes > self.budget {
            return None;
        }
        while self.pcm_bytes + bytes > self.budget {
            let victim = self
                .entries
                .iter()
                .filter(|(_, (f, _))| matches!(f, Found::Loaded(b) if Arc::strong_count(b) == 1))
                .min_by_key(|(_, (_, used))| *used)
                .map(|(k, _)| k.clone())?;
            if let Some((Found::Loaded(b), _)) = self.entries.remove(&victim) {
                self.pcm_bytes -= b.pcm.data.len();
                self.evicted += 1;
            }
        }
        if self.entries.len() >= MAX_CACHED_BUFFERS && !self.evict_one(true) {
            return None;
        }
        let b = Arc::new(Buffer { name, pcm });
        self.pcm_bytes += bytes;
        self.entries.insert(key, (Found::Loaded(b.clone()), now));
        Some(b)
    }

    /// The loaded buffer of `name`, if any.
    pub fn buffer(&mut self, files: &mut dyn SoundFiles, name: &[u8]) -> Option<Arc<Buffer>> {
        match self.find(files, name) {
            Found::Loaded(b) => Some(b),
            _ => None,
        }
    }

    /// How many names are cached.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
#[path = "buffer_tests.rs"]
pub(crate) mod tests;
