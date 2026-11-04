//! `AudioBuffer::readOgg`: the whole stream decoded at once with `lewton`.
//!
//! The stream is opened (failure: no buffer); the channels and rate are those of the current link's identification header; `size =
//! total * (channels == 1 ? 2 : 4)` bytes are reserved, `total` being the stream's length in samples per channel; samples are decoded
//! (16-bit, signed, little-endian) until `size` bytes or the stream ends; then the buffer is `MONO16` or `STEREO16` (`channels == 1`
//! or not) of `size` bytes at `rate`. So a stream of three or more channels is buffered as stereo of `size` bytes of its interleaved
//! samples, and a stream that decodes short of its total keeps its size (the rest is silence).
//!
//! The total is the last page's granule position of the stream (the first logical stream's: a chained file is cut at its first
//! link, a cap of this crate). A total above [`MAX_PCM_BYTES`] fails the read.
//! `lewton` is a third-party decoder of hostile input: a panic inside it is caught and is a failed read ([`OggError::DecoderPanic`]),
//! and the headers it would size allocations from are checked first ([`crate::vorbis_check`]: the comment header is never given
//! to it, the setup header's codebooks are walked and capped). The packets are read with the `ogg` crate `lewton` itself uses, and
//! decoded packet by packet as `OggStreamReader::read_dec_packet_itl` does for one link (so a chained file's later headers never reach
//! `lewton` either).

use std::io::Cursor;
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::MAX_PCM_BYTES;
use crate::wav::Pcm;

/// The most packets one stream is decoded for (a hard cap: about 6 hours of 44.1 kHz audio at 1,024 samples a packet).
pub const MAX_OGG_PACKETS: usize = 1 << 20;

/// Why an Ogg file gives no buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OggError {
    /// Not an Ogg Vorbis stream: the decoder's reason.
    Open(String),
    /// A rate or channel count of 0.
    BadHeader,
    /// No page with a granule position: no total.
    NoTotal,
    /// A total above [`MAX_PCM_BYTES`].
    TooLarge(u64),
    /// The decoder panicked (caught).
    DecoderPanic,
}

impl std::fmt::Display for OggError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OggError::Open(e) => write!(f, "not an Ogg Vorbis stream ({e})"),
            OggError::BadHeader => write!(f, "a rate or channel count of 0"),
            OggError::NoTotal => write!(f, "no granule position"),
            OggError::TooLarge(n) => write!(f, "{n} samples (above the cap)"),
            OggError::DecoderPanic => write!(f, "the decoder failed"),
        }
    }
}

/// The granule position of the last page of the first logical stream (its length in samples per channel), from the raw pages.
pub fn last_granule(bytes: &[u8]) -> Option<u64> {
    let mut at = 0usize;
    let mut serial: Option<u32> = None;
    let mut last: Option<u64> = None;
    while at + 27 <= bytes.len() {
        if &bytes[at..at + 4] != b"OggS" || bytes[at + 4] != 0 {
            // Resynchronise on the next capture pattern, as vorbisfile does.
            match bytes[at + 1..].windows(4).position(|w| w == b"OggS") {
                Some(p) => {
                    at += 1 + p;
                    continue;
                }
                None => break,
            }
        }
        let granule = u64::from_le_bytes(bytes[at + 6..at + 14].try_into().ok()?);
        let this = u32::from_le_bytes(bytes[at + 14..at + 18].try_into().ok()?);
        let segments = usize::from(bytes[at + 26]);
        let table_end = at + 27 + segments;
        if table_end > bytes.len() {
            break;
        }
        let body: usize = bytes[at + 27..table_end]
            .iter()
            .map(|b| usize::from(*b))
            .sum();
        match serial {
            None => serial = Some(this),
            Some(s) if s != this => break,
            _ => {}
        }
        if granule != u64::MAX {
            last = Some(granule);
        }
        at = table_end + body;
    }
    last
}

/// Reads an Ogg Vorbis file (see the module).
pub fn read_ogg(bytes: &[u8]) -> Result<Pcm, OggError> {
    match catch_unwind(AssertUnwindSafe(|| decode(bytes))) {
        Ok(r) => r,
        Err(_) => Err(OggError::DecoderPanic),
    }
}

fn decode(bytes: &[u8]) -> Result<Pcm, OggError> {
    let open = |e: String| OggError::Open(e);
    let header = |e: lewton::header::HeaderReadError| {
        OggError::Open(lewton::VorbisError::BadHeader(e).to_string())
    };
    let mut reader = ::ogg::PacketReader::new(Cursor::new(bytes));
    // The three headers of the first logical stream, as `lewton::inside_ogg::read_headers` takes them; the comment header is
    // checked and never parsed (`crate::vorbis_check`), the setup header's codebooks checked before `lewton` parses them.
    let first = reader
        .read_packet_expected()
        .map_err(|e| open(e.to_string()))?;
    let ident = lewton::header::read_header_ident(&first.data).map_err(header)?;
    let serial = first.stream_serial();
    let comment = next_packet(&mut reader, serial)?;
    crate::vorbis_check::check_comment(&comment.data).map_err(open)?;
    let setup_packet = next_packet(&mut reader, serial)?;
    crate::vorbis_check::check_setup(&setup_packet.data).map_err(open)?;
    let setup = lewton::header::read_header_setup(
        &setup_packet.data,
        ident.audio_channels,
        (ident.blocksize_0, ident.blocksize_1),
    )
    .map_err(header)?;
    drop((comment, setup_packet));
    let channels = ident.audio_channels;
    let rate = ident.audio_sample_rate;
    if channels == 0 || rate == 0 {
        return Err(OggError::BadHeader);
    }
    let total = last_granule(bytes).ok_or(OggError::NoTotal)?;
    let frame = if channels == 1 { 2u64 } else { 4 };
    let size = total
        .checked_mul(frame)
        .filter(|n| *n <= MAX_PCM_BYTES as u64)
        .ok_or(OggError::TooLarge(total))? as usize;
    let mut data: Vec<u8> = Vec::new();
    let mut pwr = lewton::audio::PreviousWindowRight::new();
    let mut absgp: Option<u64> = None;
    let mut packets = 0usize;
    while data.len() < size && packets < MAX_OGG_PACKETS {
        packets += 1;
        // `OggStreamReader::read_dec_packet_itl` over one link: another stream's packets are skipped, a new stream (a chained file)
        // ends the read (the first link only: a cap of this crate, as `last_granule`).
        let pck = match reader.read_packet() {
            Ok(Some(p)) if p.stream_serial() == serial => p,
            Ok(Some(p)) if p.first_in_stream() => break,
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => break,
        };
        let mut decoded: lewton::samples::InterleavedSamples<i16> =
            match lewton::audio::read_audio_packet_generic(&ident, &setup, &pck.data, &mut pwr) {
                Ok(d) => d,
                Err(_) => break,
            };
        if let (Some(at), true) = (absgp, pck.last_in_stream()) {
            let target = pck.absgp_page().saturating_sub(at) as usize;
            lewton::samples::Samples::truncate(&mut decoded, target);
        }
        if pck.last_in_page() {
            absgp = Some(pck.absgp_page());
        } else if let Some(at) = absgp.as_mut() {
            *at += lewton::samples::Samples::num_samples(&decoded) as u64;
        }
        let want = (size - data.len()).min(decoded.samples.len() * 2);
        if data.try_reserve(want).is_err() {
            break;
        }
        for s in decoded.samples {
            if data.len() >= size {
                break;
            }
            data.extend_from_slice(&s.to_le_bytes());
        }
    }
    Ok(Pcm {
        channels: if channels == 1 { 1 } else { 2 },
        bits: 16,
        rate,
        data,
        size,
    })
}

/// The next packet of the stream `serial` (`read_headers`' loop: other streams' packets are skipped).
fn next_packet(
    reader: &mut ::ogg::PacketReader<Cursor<&[u8]>>,
    serial: u32,
) -> Result<::ogg::Packet, OggError> {
    loop {
        let p = reader
            .read_packet_expected()
            .map_err(|e| OggError::Open(e.to_string()))?;
        if p.stream_serial() == serial {
            return Ok(p);
        }
    }
}

#[cfg(test)]
#[path = "ogg_tests.rs"]
pub(crate) mod tests;
