//! KTX 2.0 textures (the Khronos container): a 2D texture's levels in one of the formats of [`Ktx2Format`], each level's
//! bytes stored as they are or as a Zstandard stream ([`Supercompression`]), with the file's key/value data.
//!
//! [`write()`] makes a file that other KTX 2.0 readers take: the header and level index, a basic data format descriptor of the
//! format, the key/value data sorted by key (with `KTXwriter` added when absent), and the levels smallest first, each aligned
//! as the specification asks when not supercompressed. [`read`] takes its own files and any other of the formats here.
//!
//! **Every file is untrusted.** [`header`] checks everything the file says about itself before anything is allocated for it:
//! the identifier; the format and its type size; the sides (1 to [`Limits::max_side`]), depth, layers and faces (one 2D
//! texture); the level count (at most the chain of the sides); the supercompression scheme; every range (descriptor,
//! key/value data, supercompression data, each level) inside the file; each level's uncompressed length its format's size
//! at its sides, and their sum at most [`Limits::max_bytes`]; the key/value data at most [`Limits::max_key_value_bytes`]
//! and well formed. [`read`] then decodes a Zstandard level into at most its declared length (the decoder's window is held to
//! that length's order too), and refuses a stream that is shorter or longer.

use std::io::Read;

use crate::bc::BlockFormat;

/// The formats a file's levels may hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ktx2Format {
    /// `VK_FORMAT_R8G8B8A8_UNORM`.
    Rgba8,
    /// `VK_FORMAT_R8G8B8A8_SRGB`.
    Rgba8Srgb,
    /// `VK_FORMAT_R16_UNORM`.
    R16,
    /// `VK_FORMAT_BC4_UNORM_BLOCK`.
    Bc4,
    /// `VK_FORMAT_BC5_UNORM_BLOCK`.
    Bc5,
    /// `VK_FORMAT_BC7_UNORM_BLOCK`.
    Bc7,
    /// `VK_FORMAT_BC7_SRGB_BLOCK`.
    Bc7Srgb,
}

impl Ktx2Format {
    pub const ALL: [Ktx2Format; 7] = [
        Ktx2Format::Rgba8,
        Ktx2Format::Rgba8Srgb,
        Ktx2Format::R16,
        Ktx2Format::Bc4,
        Ktx2Format::Bc5,
        Ktx2Format::Bc7,
        Ktx2Format::Bc7Srgb,
    ];

    /// Its `VkFormat`.
    pub const fn vk(self) -> u32 {
        match self {
            Ktx2Format::Rgba8 => 37,
            Ktx2Format::Rgba8Srgb => 43,
            Ktx2Format::R16 => 70,
            Ktx2Format::Bc4 => 139,
            Ktx2Format::Bc5 => 141,
            Ktx2Format::Bc7 => 145,
            Ktx2Format::Bc7Srgb => 146,
        }
    }

    pub fn of_vk(vk: u32) -> Option<Ktx2Format> {
        Ktx2Format::ALL.into_iter().find(|f| f.vk() == vk)
    }

    /// The block format of a compressed one.
    pub const fn block(self) -> Option<BlockFormat> {
        match self {
            Ktx2Format::Bc4 => Some(BlockFormat::Bc4),
            Ktx2Format::Bc5 => Some(BlockFormat::Bc5),
            Ktx2Format::Bc7 | Ktx2Format::Bc7Srgb => Some(BlockFormat::Bc7),
            Ktx2Format::Rgba8 | Ktx2Format::Rgba8Srgb | Ktx2Format::R16 => None,
        }
    }

    /// Whether its color is sRGB-encoded.
    pub const fn srgb(self) -> bool {
        matches!(self, Ktx2Format::Rgba8Srgb | Ktx2Format::Bc7Srgb)
    }

    /// The header's `typeSize`: 2 for R16, else 1.
    const fn type_size(self) -> u32 {
        match self {
            Ktx2Format::R16 => 2,
            _ => 1,
        }
    }

    /// The bytes of a texel block (a texel's of an uncompressed format).
    const fn block_bytes(self) -> u32 {
        match self {
            Ktx2Format::Rgba8 | Ktx2Format::Rgba8Srgb => 4,
            Ktx2Format::R16 => 2,
            Ktx2Format::Bc4 => 8,
            Ktx2Format::Bc5 | Ktx2Format::Bc7 | Ktx2Format::Bc7Srgb => 16,
        }
    }

    /// The bytes of a level of `w` x `h`.
    pub const fn level_bytes(self, w: u32, h: u32) -> u64 {
        match self.block() {
            Some(b) => b.level_bytes(w, h),
            None => w as u64 * h as u64 * self.block_bytes() as u64,
        }
    }
}

/// How a file's levels are stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Supercompression {
    None,
    /// A Zstandard stream a level.
    Zstandard,
}

impl Supercompression {
    const fn scheme(self) -> u32 {
        match self {
            Supercompression::None => 0,
            Supercompression::Zstandard => 2,
        }
    }
}

/// A 2D texture: its format, level 0's sides, its levels (level 0 first, each its format's bytes at its sides) and its
/// key/value data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ktx2 {
    pub format: Ktx2Format,
    pub width: u32,
    pub height: u32,
    pub levels: Vec<Vec<u8>>,
    /// Keys (UTF-8, no NUL) and values, in any order: [`write()`] sorts them.
    pub key_values: Vec<(String, Vec<u8>)>,
}

/// What [`header`] and [`read`] take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The largest side.
    pub max_side: u32,
    /// The most bytes of all levels together, uncompressed.
    pub max_bytes: u64,
    /// The most bytes of key/value data.
    pub max_key_value_bytes: u32,
}

impl Default for Limits {
    /// A side of 4,096, 128 MiB of levels (a whole RGBA8 chain of 4,096 x 4,096 is 85.3 MiB), 64 KiB of key/value data.
    fn default() -> Limits {
        Limits {
            max_side: 4096,
            max_bytes: 128 << 20,
            max_key_value_bytes: 64 << 10,
        }
    }
}

/// What a file says about itself, checked against its [`Limits`] ([`header`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub format: Ktx2Format,
    pub width: u32,
    pub height: u32,
    pub level_count: u32,
    pub supercompression: Supercompression,
    /// Every level's bytes together, uncompressed.
    pub bytes: u64,
}

/// Why a file was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Ktx2Error {
    #[error("not a KTX 2.0 file")]
    NotKtx2,
    #[error("the file ends inside its {0}")]
    Truncated(&'static str),
    #[error("format {0} is not one of the formats taken")]
    Format(u32),
    #[error("the header's {field} is {value}, not what a 2D texture of its format has")]
    Header { field: &'static str, value: u64 },
    #[error("{w} x {h} is empty or past the {max} a side taken")]
    Size { w: u32, h: u32, max: u32 },
    #[error("supercompression scheme {0} is not taken (none and Zstandard are)")]
    Scheme(u32),
    #[error("the {0} lies outside the file")]
    Range(&'static str),
    #[error("level {level} holds {got} bytes, not its {want}")]
    Level { level: u32, got: u64, want: u64 },
    #[error("the levels hold {0} bytes, past the {1} taken")]
    TooBig(u64, u64),
    #[error("the key/value data is malformed: {0}")]
    KeyValue(&'static str),
    #[error("level {level}'s Zstandard stream did not decode: {error}")]
    Zstandard { level: u32, error: String },
}

const IDENTIFIER: [u8; 12] = [
    0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A,
];
/// The identifier, header and index.
const FIXED: usize = 80;
const LEVEL_ENTRY: usize = 24;
const WRITER: &str = "KTXwriter";
const WRITER_NAME: &[u8] = b"torque-rs\0";

/// The chain's length from a level of `w` x `h` down to 1 x 1.
pub const fn chain_len(w: u32, h: u32) -> u32 {
    let m = if w > h { w } else { h };
    32 - (if m == 0 { 1 } else { m }).leading_zeros()
}

fn u32_at(b: &[u8], at: usize, what: &'static str) -> Result<u32, Ktx2Error> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or(Ktx2Error::Truncated(what))
}

fn u64_at(b: &[u8], at: usize, what: &'static str) -> Result<u64, Ktx2Error> {
    Ok(u64::from(u32_at(b, at, what)?) | u64::from(u32_at(b, at + 4, what)?) << 32)
}

/// `offset` and `len` as a range inside a file of `n` bytes.
fn range(
    offset: u64,
    len: u64,
    n: usize,
    what: &'static str,
) -> Result<std::ops::Range<usize>, Ktx2Error> {
    let end = offset.checked_add(len).ok_or(Ktx2Error::Range(what))?;
    if end > n as u64 {
        return Err(Ktx2Error::Range(what));
    }
    Ok(offset as usize..end as usize)
}

/// A file's checked header and where its parts are.
struct Parsed {
    header: Header,
    /// Each level's stored range and uncompressed length, level 0 first.
    levels: Vec<(std::ops::Range<usize>, u64)>,
    key_values: std::ops::Range<usize>,
}

fn parse(b: &[u8], limits: &Limits) -> Result<Parsed, Ktx2Error> {
    if b.len() < IDENTIFIER.len() || b[..IDENTIFIER.len()] != IDENTIFIER {
        return Err(Ktx2Error::NotKtx2);
    }
    let field = |i: usize| u32_at(b, 12 + 4 * i, "header");
    let vk = field(0)?;
    let format = Ktx2Format::of_vk(vk).ok_or(Ktx2Error::Format(vk))?;
    let header_field = |name: &'static str, value: u32, ok: bool| {
        if ok {
            Ok(())
        } else {
            Err(Ktx2Error::Header {
                field: name,
                value: u64::from(value),
            })
        }
    };
    let type_size = field(1)?;
    header_field("typeSize", type_size, type_size == format.type_size())?;
    let (w, h) = (field(2)?, field(3)?);
    if w == 0 || h == 0 || w > limits.max_side || h > limits.max_side {
        return Err(Ktx2Error::Size {
            w,
            h,
            max: limits.max_side,
        });
    }
    let depth = field(4)?;
    header_field("pixelDepth", depth, depth == 0)?;
    let layers = field(5)?;
    header_field("layerCount", layers, layers == 0)?;
    let faces = field(6)?;
    header_field("faceCount", faces, faces == 1)?;
    // A count of 0 asks the reader to make the chain: one level is stored.
    let level_count = field(7)?.max(1);
    header_field("levelCount", level_count, level_count <= chain_len(w, h))?;
    let scheme = field(8)?;
    let supercompression = match scheme {
        0 => Supercompression::None,
        2 => Supercompression::Zstandard,
        s => return Err(Ktx2Error::Scheme(s)),
    };
    let index = |i: usize| u32_at(b, 48 + 4 * i, "index");
    let dfd = range(
        index(0)?.into(),
        index(1)?.into(),
        b.len(),
        "data format descriptor",
    )?;
    let (kvd_offset, kvd_len) = (index(2)?, index(3)?);
    if kvd_len > limits.max_key_value_bytes {
        return Err(Ktx2Error::TooBig(
            kvd_len.into(),
            limits.max_key_value_bytes.into(),
        ));
    }
    let key_values = range(kvd_offset.into(), kvd_len.into(), b.len(), "key/value data")?;
    range(
        u64_at(b, 64, "index")?,
        u64_at(b, 72, "index")?,
        b.len(),
        "supercompression data",
    )?;
    if dfd.len() >= 4 && u32_at(b, dfd.start, "data format descriptor")? as usize != dfd.len() {
        return Err(Ktx2Error::Range("data format descriptor"));
    }
    let n = level_count as usize;
    if b.len() < FIXED + LEVEL_ENTRY * n {
        return Err(Ktx2Error::Truncated("level index"));
    }
    let mut levels = Vec::with_capacity(n);
    let mut bytes = 0u64;
    for k in 0..level_count {
        let at = FIXED + LEVEL_ENTRY * k as usize;
        let (offset, len, raw) = (
            u64_at(b, at, "level index")?,
            u64_at(b, at + 8, "level index")?,
            u64_at(b, at + 16, "level index")?,
        );
        let want = format.level_bytes((w >> k).max(1), (h >> k).max(1));
        if raw != want || (supercompression == Supercompression::None && len != want) {
            return Err(Ktx2Error::Level {
                level: k,
                got: if raw == want { len } else { raw },
                want,
            });
        }
        bytes += want;
        if bytes > limits.max_bytes {
            return Err(Ktx2Error::TooBig(bytes, limits.max_bytes));
        }
        levels.push((range(offset, len, b.len(), "level data")?, want));
    }
    key_value_entries(&b[key_values.clone()])?;
    Ok(Parsed {
        header: Header {
            format,
            width: w,
            height: h,
            level_count,
            supercompression,
            bytes,
        },
        levels,
        key_values,
    })
}

/// The key/value data's entries, each checked: a length, a key of UTF-8 ended by a NUL, the value, padding to 4 bytes.
fn key_value_entries(mut d: &[u8]) -> Result<Vec<(&str, &[u8])>, Ktx2Error> {
    let mut out = Vec::new();
    while !d.is_empty() {
        let len = u32_at(d, 0, "key/value data")? as usize;
        let entry = d
            .get(4..)
            .and_then(|r| r.get(..len))
            .ok_or(Ktx2Error::KeyValue("an entry runs past the data"))?;
        let nul = entry
            .iter()
            .position(|&c| c == 0)
            .ok_or(Ktx2Error::KeyValue("a key has no NUL"))?;
        let key = std::str::from_utf8(&entry[..nul])
            .map_err(|_| Ktx2Error::KeyValue("a key is not UTF-8"))?;
        if key.is_empty() {
            return Err(Ktx2Error::KeyValue("a key is empty"));
        }
        out.push((key, &entry[nul + 1..]));
        let next = (4 + len).next_multiple_of(4);
        d = d.get(next..).unwrap_or(&[]);
    }
    Ok(out)
}

/// The file's header, checked against `limits` (nothing is decoded or allocated for its levels).
///
/// # Errors
/// The first check the file fails (see the module's list).
pub fn header(bytes: &[u8], limits: &Limits) -> Result<Header, Ktx2Error> {
    parse(bytes, limits).map(|p| p.header)
}

/// The value of the file's key `key`, the file checked as [`header`] checks it (no level decoded).
///
/// # Errors
/// A check of [`header`].
pub fn value_of(bytes: &[u8], key: &str, limits: &Limits) -> Result<Option<Vec<u8>>, Ktx2Error> {
    let p = parse(bytes, limits)?;
    Ok(key_value_entries(&bytes[p.key_values])?
        .into_iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.to_vec()))
}

/// The file's texture.
///
/// # Errors
/// A check of [`header`], or a Zstandard level that does not decode to its length.
pub fn read(bytes: &[u8], limits: &Limits) -> Result<Ktx2, Ktx2Error> {
    let p = parse(bytes, limits)?;
    let key_values = key_value_entries(&bytes[p.key_values])?
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_vec()))
        .collect();
    let levels = p
        .levels
        .into_iter()
        .enumerate()
        .map(|(k, (r, want))| match p.header.supercompression {
            Supercompression::None => Ok(bytes[r].to_vec()),
            Supercompression::Zstandard => {
                unzstd(&bytes[r], want).map_err(|error| Ktx2Error::Zstandard {
                    level: k as u32,
                    error,
                })
            }
        })
        .collect::<Result<_, _>>()?;
    Ok(Ktx2 {
        format: p.header.format,
        width: p.header.width,
        height: p.header.height,
        levels,
        key_values,
    })
}

/// A Zstandard stream decoded into exactly `want` bytes: the decoder's window is held to `want`'s order (at least 1 MiB) and it
/// is read for at most one byte past `want`.
fn unzstd(src: &[u8], want: u64) -> Result<Vec<u8>, String> {
    let window = want.next_power_of_two().clamp(1 << 20, 1 << 27);
    let mut d = ruzstd::decoding::StreamingDecoder::new_with_max_window_size(src, window)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(want as usize);
    (&mut d)
        .take(want + 1)
        .read_to_end(&mut out)
        .map_err(|e| e.to_string())?;
    if out.len() as u64 != want {
        return Err(format!("{} bytes, not {want}", out.len()));
    }
    Ok(out)
}

/// A descriptor sample: bit offset, bit length less 1, channel (with its qualifiers), lower and upper.
type Sample = (u16, u8, u8, u32, u32);

/// The basic data format descriptor of `format` (the file's `dfdTotalSize` first).
fn descriptor(format: Ktx2Format, supercompressed: bool) -> Vec<u8> {
    // The color model, the block's side less 1, the samples.
    let (model, side, samples): (u8, u8, Vec<Sample>) = match format {
        Ktx2Format::Rgba8 | Ktx2Format::Rgba8Srgb => (
            1,
            0,
            (0..4)
                .map(|c| {
                    // Alpha is channel 15; in an sRGB format it is linear (the qualifier's bit).
                    let channel = if c == 3 {
                        15 | if format.srgb() { 0x10 } else { 0 }
                    } else {
                        c as u8
                    };
                    (8 * c, 7, channel, 0, 255)
                })
                .collect(),
        ),
        Ktx2Format::R16 => (1, 0, vec![(0, 15, 0, 0, 65_535)]),
        Ktx2Format::Bc4 => (131, 3, vec![(0, 63, 0, 0, u32::MAX)]),
        Ktx2Format::Bc5 => (
            132,
            3,
            vec![(0, 63, 0, 0, u32::MAX), (64, 63, 1, 0, u32::MAX)],
        ),
        Ktx2Format::Bc7 | Ktx2Format::Bc7Srgb => (134, 3, vec![(0, 127, 0, 0, u32::MAX)]),
    };
    let block = 24 + 16 * samples.len();
    let mut d = Vec::with_capacity(4 + block);
    d.extend_from_slice(&(4 + block as u32).to_le_bytes());
    d.extend_from_slice(&0u32.to_le_bytes());
    d.extend_from_slice(&(2u32 | (block as u32) << 16).to_le_bytes());
    // Model, BT.709 primaries, the transfer (sRGB 2, linear 1), straight alpha.
    d.extend_from_slice(&[model, 1, if format.srgb() { 2 } else { 1 }, 0]);
    d.extend_from_slice(&[side, side, 0, 0]);
    let plane0 = if supercompressed {
        0
    } else {
        format.block_bytes() as u8
    };
    d.extend_from_slice(&[plane0, 0, 0, 0, 0, 0, 0, 0]);
    for (offset, len, channel, lower, upper) in samples {
        d.extend_from_slice(&offset.to_le_bytes());
        d.extend_from_slice(&[len, channel, 0, 0, 0, 0]);
        d.extend_from_slice(&lower.to_le_bytes());
        d.extend_from_slice(&upper.to_le_bytes());
    }
    d
}

/// `t` as a KTX 2.0 file, its levels stored by `sc`.
///
/// # Errors
/// Empty sides, more levels than the chain or none, a level not its format's bytes at its sides, or a key that is empty,
/// holds a NUL or is given twice.
pub fn write(t: &Ktx2, sc: Supercompression) -> Result<Vec<u8>, Ktx2Error> {
    let (w, h, f) = (t.width, t.height, t.format);
    if w == 0 || h == 0 {
        return Err(Ktx2Error::Size {
            w,
            h,
            max: u32::MAX,
        });
    }
    let n = t.levels.len() as u32;
    if n == 0 || n > chain_len(w, h) {
        return Err(Ktx2Error::Header {
            field: "levelCount",
            value: n.into(),
        });
    }
    for (k, l) in t.levels.iter().enumerate() {
        let want = f.level_bytes((w >> k).max(1), (h >> k).max(1));
        if l.len() as u64 != want {
            return Err(Ktx2Error::Level {
                level: k as u32,
                got: l.len() as u64,
                want,
            });
        }
    }
    let mut kv: Vec<(&str, &[u8])> = t
        .key_values
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_slice()))
        .collect();
    if !kv.iter().any(|(k, _)| *k == WRITER) {
        kv.push((WRITER, WRITER_NAME));
    }
    kv.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    if kv.iter().any(|(k, _)| k.is_empty() || k.contains('\0'))
        || kv.windows(2).any(|p| p[0].0 == p[1].0)
    {
        return Err(Ktx2Error::KeyValue(
            "a key is empty, holds a NUL or is given twice",
        ));
    }
    let mut kvd = Vec::new();
    for (k, v) in kv {
        kvd.extend_from_slice(&((k.len() + 1 + v.len()) as u32).to_le_bytes());
        kvd.extend_from_slice(k.as_bytes());
        kvd.push(0);
        kvd.extend_from_slice(v);
        kvd.resize(kvd.len().next_multiple_of(4), 0);
    }
    let stored: Vec<std::borrow::Cow<'_, [u8]>> = t
        .levels
        .iter()
        .map(|l| match sc {
            Supercompression::None => std::borrow::Cow::Borrowed(l.as_slice()),
            Supercompression::Zstandard => ruzstd::encoding::compress_to_vec(
                l.as_slice(),
                ruzstd::encoding::CompressionLevel::Fastest,
            )
            .into(),
        })
        .collect();
    let dfd = descriptor(f, sc != Supercompression::None);
    let dfd_offset = FIXED + LEVEL_ENTRY * n as usize;
    let kvd_offset = dfd_offset + dfd.len();
    let mut out = Vec::new();
    out.extend_from_slice(&IDENTIFIER);
    for v in [f.vk(), f.type_size(), w, h, 0, 0, 1, n, sc.scheme()] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    let kvd_at = if kvd.is_empty() { 0 } else { kvd_offset as u32 };
    for v in [
        dfd_offset as u32,
        dfd.len() as u32,
        kvd_at,
        kvd.len() as u32,
    ] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&[0u8; 16]);
    // The level index, filled in as the levels are placed (smallest first).
    out.resize(dfd_offset, 0);
    out.extend_from_slice(&dfd);
    out.extend_from_slice(&kvd);
    let align = match sc {
        Supercompression::None => (f.block_bytes() as usize).max(4).next_power_of_two(),
        Supercompression::Zstandard => 1,
    };
    for k in (0..n as usize).rev() {
        out.resize(out.len().next_multiple_of(align), 0);
        let at = FIXED + LEVEL_ENTRY * k;
        let entry = [
            out.len() as u64,
            stored[k].len() as u64,
            t.levels[k].len() as u64,
        ];
        for (i, v) in entry.into_iter().enumerate() {
            out[at + 8 * i..at + 8 * i + 8].copy_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&stored[k]);
    }
    Ok(out)
}

#[cfg(test)]
#[path = "ktx2_tests.rs"]
pub(crate) mod tests;
