//! `AudioBuffer::readWAV`, a lenient reader: no `RIFF`/`WAVE` check, chunks walked by the RIFF size, the last `fmt ` chunk deciding
//! the format, the last `data` chunk the samples.
//!
//! The walk:
//! - It reads 4 bytes (the RIFF id), the 32-bit RIFF size, 4 bytes (the form type); `left = ((size + 1) & !1) - 4` (wrapping), then the
//!   first chunk's id and size; `rest = size + (size & 1)`.
//! - While `left != 0` and the stream has not hit its end:
//!   - `fmt `: format, channels (16 bits each), rate, bytes per second (32 bits each), block align, bits (16 bits each). Format 1 (PCM):
//!     the rate is kept, the AL format is mono or stereo by `channels == 1`, 8 or 16 bits by `bits == 8`, and `rest -= 16`; any other
//!     format reads the 4-byte extension (`WAVFmtExHdr`) and `rest -= 4`, its size alone.
//!   - `data`: when the last `fmt ` said PCM, the chunk's own size is allocated and read and `rest -= size`; otherwise the chunk is
//!     skipped. A later `data` chunk replaces an earlier one.
//!   - `smpl`: the 60-byte `WAVSmplHdr` is read and `rest -= 60`, whatever the chunk holds. A host's [`WavRules`] may stop the walk
//!     at a `smpl` chunk instead (the samples read so far are kept; [`WavReport::smpl_stops`] counts it).
//!   - Then `rest` bytes are skipped (a signed compare: a negative `rest` skips nothing), `left -= 8 + ((size + 1) & !1)` with this
//!     chunk's size, and the next chunk's id and size are read.
//! - No samples: the read fails. Otherwise `alBufferData(format, samples, size, rate)`.
//!
//! Where the file leaves a value undefined: a read past the end gives what is there and the stream is at its end ("EOS", what TGE's
//! streams do); the rest of a `data` chunk that the file does not hold is zeros; a field read at the end keeps its previous value. A
//! `data` chunk larger than [`MAX_PCM_BYTES`] fails the read. `alBufferData` is OpenAL's: a rate of 0 is refused
//! ([`WavError::BadRate`]), as the OpenAL 1.0 specification asks.

use crate::MAX_PCM_BYTES;

/// The size of a `smpl` chunk's header (`WAVSmplHdr`: nine 32-bit fields and one 24-byte loop).
pub const SMPL_HEADER_BYTES: u64 = 60;

/// One decoded sound: interleaved little-endian samples (`bits` 8, unsigned, or 16, signed), as `alBufferData` received them. `size`
/// is the size given to OpenAL (the `data` chunk's own, or the Ogg total's); `data` holds the bytes the file had, at most `size` (the
/// rest is silence), so a short file that claims a large chunk costs no memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcm {
    pub channels: u16,
    pub bits: u16,
    pub rate: u32,
    pub data: Vec<u8>,
    pub size: usize,
}

impl Pcm {
    /// Bytes of one frame.
    pub fn frame_bytes(&self) -> usize {
        usize::from(self.channels) * usize::from(self.bits / 8).max(1)
    }

    /// Whole frames in the buffer (its OpenAL size).
    pub fn frames(&self) -> usize {
        self.size / self.frame_bytes().max(1)
    }
}

/// Why a sound file gives no buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WavError {
    /// No `data` chunk after a PCM `fmt ` chunk (also: every format but PCM, which reads as no samples).
    NoSamples,
    /// A `data` chunk larger than [`MAX_PCM_BYTES`].
    TooLarge(u32),
    /// A rate of 0 (refused by `alBufferData`).
    BadRate,
}

impl std::fmt::Display for WavError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WavError::NoSamples => write!(f, "no PCM samples"),
            WavError::TooLarge(n) => {
                write!(f, "a data chunk of {n} bytes (the cap is {MAX_PCM_BYTES})")
            }
            WavError::BadRate => write!(f, "a sample rate of 0"),
        }
    }
}

/// What the walk met beside its result.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WavReport {
    /// Walks a [`WavRules::reads_smpl`] answer stopped (0 or 1).
    pub smpl_stops: u32,
}

/// A host's rules for the reader. The defaults are TGE's `readWAV`.
pub trait WavRules: std::fmt::Debug + Send + Sync {
    /// Whether the walk reads the header of a `smpl` chunk that has `rest` bytes left (true: TGE reads it whatever the chunk
    /// holds). False ends the walk there, keeping the samples read so far.
    fn reads_smpl(&self, _rest: u32) -> bool {
        true
    }

    /// The console line (an error) for a walk stopped at a `smpl` chunk of the file `dir/file`; none by default.
    fn smpl_stop_line(&self, _dir: &str, _file: &str) -> Option<String> {
        None
    }
}

/// TGE's rules.
#[derive(Debug, Clone, Copy, Default)]
pub struct TgeWav;

impl WavRules for TgeWav {}

/// TGE's stream over a file: a read past the end gives what is left and puts the stream at its end.
struct Stream<'a> {
    bytes: &'a [u8],
    at: usize,
    eos: bool,
}

impl Stream<'_> {
    /// Reads into `out`; past the end the rest of `out` is left as it was.
    fn read_into(&mut self, out: &mut [u8]) {
        let left = self.bytes.len().saturating_sub(self.at);
        let n = out.len().min(left);
        out[..n].copy_from_slice(&self.bytes[self.at..self.at + n]);
        self.at += n;
        if n < out.len() {
            self.eos = true;
        }
    }

    fn u16(&mut self, old: u16) -> u16 {
        let mut b = old.to_le_bytes();
        self.read_into(&mut b);
        u16::from_le_bytes(b)
    }

    fn u32(&mut self, old: u32) -> u32 {
        let mut b = old.to_le_bytes();
        self.read_into(&mut b);
        u32::from_le_bytes(b)
    }

    fn id(&mut self, old: [u8; 4]) -> [u8; 4] {
        let mut b = old;
        self.read_into(&mut b);
        b
    }

    fn skip(&mut self, n: u64) {
        let left = (self.bytes.len() - self.at) as u64;
        if n > left {
            self.at = self.bytes.len();
            self.eos = true;
        } else {
            self.at += n as usize;
        }
    }
}

/// Reads a WAV file with TGE's rules (see the module).
pub fn read_wav(bytes: &[u8]) -> (Result<Pcm, WavError>, WavReport) {
    read_wav_with(bytes, &TgeWav)
}

/// Reads a WAV file with a host's rules.
pub fn read_wav_with(bytes: &[u8], rules: &dyn WavRules) -> (Result<Pcm, WavError>, WavReport) {
    let mut report = WavReport::default();
    let mut s = Stream {
        bytes,
        at: 0,
        eos: false,
    };
    let _riff = s.id([0; 4]);
    let riff_size = s.u32(0);
    let _wave = s.id([0; 4]);
    let mut left = (riff_size.wrapping_add(1) & !1).wrapping_sub(4);
    let mut id = s.id([0; 4]);
    let mut size = s.u32(0);
    let mut rest: u32 = size.wrapping_add(size & 1);
    // The last fmt chunk's values (the format starts as something that is not PCM).
    let mut format: u16 = 0;
    let (mut channels, mut rate, mut bits): (u16, u32, u16) = (0, 0, 0);
    let mut al_format: Option<(u16, u16)> = None;
    let mut freq: u32 = 22_050;
    let mut data: Option<(Vec<u8>, usize)> = None;
    loop {
        if left == 0 || s.eos {
            break;
        }
        if &id == b"fmt " {
            format = s.u16(format);
            channels = s.u16(channels);
            rate = s.u32(rate);
            let _bytes_per_sec = s.u32(0);
            let _block = s.u16(0);
            bits = s.u16(bits);
            if format == 1 {
                freq = rate;
                al_format = Some((
                    if channels == 1 { 1 } else { 2 },
                    if bits == 8 { 8 } else { 16 },
                ));
                rest = rest.wrapping_sub(16);
            } else {
                let _ex = s.u32(0);
                rest = rest.wrapping_sub(4);
            }
        } else if &id == b"data" {
            if format == 1 {
                if size as usize > MAX_PCM_BYTES {
                    return (Err(WavError::TooLarge(size)), report);
                }
                let have = (size as usize).min(s.bytes.len() - s.at);
                let mut samples = vec![0u8; have];
                s.read_into(&mut samples);
                if have < size as usize {
                    s.eos = true;
                }
                data = Some((samples, size as usize));
                rest = rest.wrapping_sub(size);
            }
        } else if &id == b"smpl" {
            if !rules.reads_smpl(rest) {
                report.smpl_stops += 1;
                break;
            }
            s.skip(SMPL_HEADER_BYTES);
            rest = rest.wrapping_sub(SMPL_HEADER_BYTES as u32);
        }
        if (rest as i32) > 0 {
            s.skip(u64::from(rest));
        }
        left = left.wrapping_sub(8u32.wrapping_add(size.wrapping_add(1) & !1));
        id = s.id(id);
        size = s.u32(size);
        rest = size.wrapping_add(size & 1);
    }
    let Some((data, size)) = data else {
        return (Err(WavError::NoSamples), report);
    };
    let Some((channels, bits)) = al_format else {
        return (Err(WavError::NoSamples), report);
    };
    if freq == 0 {
        return (Err(WavError::BadRate), report);
    }
    (
        Ok(Pcm {
            channels,
            bits,
            rate: freq,
            data,
            size,
        }),
        report,
    )
}

#[cfg(test)]
#[path = "wav_tests.rs"]
pub(crate) mod tests;
