//! The Vorbis header checks run before `lewton` sees a header. `lewton` sizes several allocations from header fields before it
//! has read the data they describe, so a small hostile file could make it reserve gigabytes, or terabytes. A fuzz-found 342-byte
//! file whose comment header claims 2,761,721,856 comments asked for 132,562,649,088 bytes (48 per comment) and aborted the
//! process.
//!
//! - **The comment header** is checked here and never given to `lewton` (no comment is read). It is walked exactly as
//!   `lewton::header::read_header_comment` reads it: type 3 and `vorbis`, the vendor length and that many bytes, which must be UTF-8, the
//!   comment count, then each comment's length and bytes, then a framing byte of 1. Every length is checked against the bytes left
//!   first, so a file is refused for any header that `lewton` would refuse, and nothing is allocated.
//! - **The setup header's codebooks** are walked bit for bit as `lewton::header::read_codebook` reads them: the sync pattern,
//!   dimensions, entries, the codeword lengths (ordered or not, sparse or not), the lookup type, and the lookup values (`lookup1_values`
//!   or entries x dimensions) of `value_bits` each. `lewton` reserves the entries' lengths, the multiplicands, entries x dimensions
//!   floats of lookup vectors and a Huffman tree of about two nodes per entry before it reads them. So the walk refuses: multiplicands
//!   whose bits are not all in the packet; more than [`MAX_CODEBOOK_ENTRIES`] entries over all codebooks; and more than
//!   [`MAX_VQ_VALUES`] lookup-vector values over all codebooks.
//!   The rest of the setup header (floors, residues, mappings, modes) allocates by counts of at most 8 bits; the audio packets allocate
//!   by the block size (at most 2^13) and the channels (at most 255).
//!
//! Ordinary music files sit far below the caps (a few thousand entries and lookup values over all their codebooks).

/// The most codebook entries a setup header may declare over all its codebooks (`lewton` keeps a length and about two Huffman
/// nodes per entry).
pub const MAX_CODEBOOK_ENTRIES: u64 = 1 << 20;
/// The most lookup-vector values (entries x dimensions of the codebooks with a lookup table) over all codebooks: 16 MiB of `f32`.
pub const MAX_VQ_VALUES: u64 = 1 << 22;
/// The sync pattern that opens every codebook: `BCV`, 24 bits.
const CODEBOOK_SYNC: u32 = u32::from_le_bytes([b'B', b'C', b'V', 0]);

/// What a setup header's codebooks declare in total.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CodebookTotals {
    pub codebooks: u32,
    pub entries: u64,
    pub vq_values: u64,
}

/// Vorbis's bit packing: least significant bit first.
struct Bits<'a> {
    bytes: &'a [u8],
    at: u64,
}

impl Bits<'_> {
    fn left(&self) -> u64 {
        (self.bytes.len() as u64 * 8).saturating_sub(self.at)
    }

    fn read(&mut self, n: u32) -> Result<u32, String> {
        debug_assert!(n <= 32);
        if u64::from(n) > self.left() {
            return Err("the setup header ends early".into());
        }
        let mut v: u64 = 0;
        for i in 0..u64::from(n) {
            let bit = self.at + i;
            let b = (self.bytes[(bit / 8) as usize] >> (bit % 8)) & 1;
            v |= u64::from(b) << i;
        }
        self.at += u64::from(n);
        Ok(v as u32)
    }

    fn skip(&mut self, n: u64) -> Result<(), String> {
        if n > self.left() {
            return Err("the setup header ends early".into());
        }
        self.at += n;
        Ok(())
    }
}

/// The bits of `x` (Vorbis's `ilog`: 0 for 0).
fn ilog(x: u64) -> u32 {
    64 - x.leading_zeros()
}

/// `lookup1_values` as `lewton` computes it: the largest `r` with `r^dimensions <= entries` (`u32::MAX` for 0 dimensions and some
/// entries, 0 or 1 from 32 dimensions up).
pub fn lookup1_values(entries: u32, dimensions: u16) -> u32 {
    if dimensions == 0 {
        return if entries == 0 { 0 } else { u32::MAX };
    }
    if dimensions >= 32 {
        return u32::from(entries != 0);
    }
    let fits = |r: u64| {
        let mut p: u64 = 1;
        for _ in 0..dimensions {
            p = match p.checked_mul(r) {
                Some(p) if p <= u64::from(entries) => p,
                _ => return false,
            };
        }
        true
    };
    // r <= entries (dimensions >= 1); binary search the largest that fits.
    let (mut lo, mut hi) = (0u64, u64::from(entries));
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        if fits(mid) { lo = mid } else { hi = mid - 1 }
    }
    lo as u32
}

/// Checks a comment header packet as `lewton` would read it (see the module).
pub fn check_comment(packet: &[u8]) -> Result<(), String> {
    let mut at = 0usize;
    let mut take = |n: usize| -> Result<&[u8], String> {
        let end = at
            .checked_add(n)
            .filter(|e| *e <= packet.len())
            .ok_or("the comment header ends early")?;
        let s = &packet[at..end];
        at = end;
        Ok(s)
    };
    let head = take(7)?;
    if head[0] & 1 == 0 {
        return Err("the comment header is an audio packet".into());
    }
    if &head[1..7] != b"vorbis" {
        return Err("not a Vorbis header".into());
    }
    if head[0] != 3 {
        return Err(format!("header type {} where the comments belong", head[0]));
    }
    let u32_at = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize;
    let vendor_len = u32_at(take(4)?);
    if std::str::from_utf8(take(vendor_len)?).is_err() {
        return Err("the vendor string is not UTF-8".into());
    }
    let count = u32_at(take(4)?);
    for _ in 0..count {
        // Each comment takes at least its 4-byte length: the loop ends with the packet.
        let n = u32_at(take(4)?);
        take(n)?;
    }
    if take(1)?[0] != 1 {
        return Err("the comment header's framing bit".into());
    }
    Ok(())
}

/// Walks a setup header's codebooks as `lewton` would read them and checks what they would allocate (see the module).
pub fn check_setup(packet: &[u8]) -> Result<CodebookTotals, String> {
    if packet.len() < 7 || packet[0] & 1 == 0 || &packet[1..7] != b"vorbis" {
        return Err("not a Vorbis header".into());
    }
    if packet[0] != 5 {
        return Err(format!("header type {} where the setup belongs", packet[0]));
    }
    let mut r = Bits {
        bytes: &packet[7..],
        at: 0,
    };
    let mut t = CodebookTotals {
        codebooks: r.read(8)? + 1,
        ..CodebookTotals::default()
    };
    for _ in 0..t.codebooks {
        if r.read(24)? != CODEBOOK_SYNC {
            return Err("a codebook's sync pattern".into());
        }
        let dimensions = r.read(16)? as u16;
        let entries = r.read(24)?;
        t.entries += u64::from(entries);
        if t.entries > MAX_CODEBOOK_ENTRIES {
            return Err(format!(
                "codebooks of {} entries (the cap is {MAX_CODEBOOK_ENTRIES})",
                t.entries
            ));
        }
        let ordered = r.read(1)? == 1;
        if !ordered {
            let sparse = r.read(1)? == 1;
            for _ in 0..entries {
                if !sparse || r.read(1)? == 1 {
                    r.read(5)?;
                }
            }
        } else {
            let mut current: u64 = 0;
            r.read(5)?;
            while current < u64::from(entries) {
                let n = r.read(ilog(u64::from(entries) - current))?;
                current += u64::from(n);
                if current > u64::from(entries) {
                    return Err("an ordered codebook's lengths overrun its entries".into());
                }
            }
        }
        let lookup = r.read(4)?;
        if lookup > 2 {
            return Err(format!("lookup type {lookup}"));
        }
        if lookup != 0 {
            r.skip(64)?; // minimum and delta
            let value_bits = u64::from(r.read(4)? + 1);
            r.read(1)?; // sequence_p
            let values = if lookup == 1 {
                u64::from(lookup1_values(entries, dimensions))
            } else {
                u64::from(entries) * u64::from(dimensions)
            };
            // The multiplicands are read one by one: all of them must be in the packet before lewton reserves them.
            let bits = values
                .checked_mul(value_bits)
                .ok_or("a codebook's lookup values overflow")?;
            if bits > r.left() {
                return Err(format!(
                    "a codebook with {values} lookup values of {value_bits} bits in {} bits",
                    r.left()
                ));
            }
            r.skip(bits)?;
            t.vq_values += u64::from(entries) * u64::from(dimensions);
            if t.vq_values > MAX_VQ_VALUES {
                return Err(format!(
                    "codebooks of {} lookup-vector values (the cap is {MAX_VQ_VALUES})",
                    t.vq_values
                ));
            }
        }
    }
    Ok(t)
}

#[cfg(test)]
#[path = "vorbis_check_tests.rs"]
mod tests;
