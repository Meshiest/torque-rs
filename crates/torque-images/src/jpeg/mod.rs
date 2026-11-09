//! JPEG as TGE 1.4 loads it: `GBitmap::readJPEG` over libjpeg 6b (`6b  27-Mar-1998`). A Rust implementation of the library's
//! decode path, written from the algorithms of libjpeg 6b so that it is texel-exact. No C statement is carried over; the
//! numeric tables the algorithms define (the IDCT's fixed-point factors, the zigzag order, the colour-conversion and smoothing
//! coefficients) are libjpeg's values.
//!
//! This software is based in part on the work of the Independent JPEG Group. `readJPEG` and the data source follow the
//! Torque Game Engine's `dgl/bitmapJpeg.cc` (TGE 1.4, Copyright (C) GarageGames.com, Inc.) and its changed
//! `lib/ljpeg/jdatasrc.c` (Copyright (C) 1994-1996, Thomas G. Lane; the Independent JPEG Group's software). The Torque engine
//! is MIT-licensed as Torque3D, Copyright (c) 2012 GarageGames, LLC. The code here is this crate's own.
//!
//! **What `readJPEG` sets:** nothing beyond `jpeg_read_header`'s defaults: `JDCT_ISLOW` (the slow-but-accurate integer IDCT,
//! `jidctint.c`), fancy upsampling on (`jdsample.c`'s triangle filters for 2:1, so `jdmerge.c` is never used), block smoothing on
//! (progressive files whose AC bands are still incomplete at the end, `jdcoefct.c`'s `decompress_smooth_data`), no colour
//! quantisation, 1:1 scale. The output colour space is libjpeg's guess: RGB for three components (the input taken as YCbCr after a
//! JFIF `APP0`, by an Adobe `APP14`'s transform flag, or by the component ids 1-2-3 / 'R'-'G'-'B'), greyscale for one. `readJPEG`
//! takes greyscale as [`Format::Alpha`] and RGB as [`Format::Rgb`] and returns false for anything else (CMYK, YCCK, 2 or 5+
//! components): [`JpegError::ColorSpace`].
//!
//! **What is covered:** the marker reader (`jdmarker.c`: SOI, SOF0/1/2, the arithmetic SOF9/10 refused as 6b does, DHT, DQT of 8 and
//! 16 bits, DRI, SOS, APP0/APP14 examined and every other APPn and COM skipped, DNL ignored, RSTn and TEM tolerated between
//! segments, the reserved markers refused, garbage before a marker skipped with a warning), sequential Huffman decoding (`jdhuff.c`:
//! the 8-bit lookahead and the slow path, zero bits stuffed after a marker with the "insufficient data" latch that leaves the rest of a
//! restart segment grey), progressive decoding (`jdphuff.c`: DC first and refinement, AC first and refinement, EOB runs, the
//! progression checks), restart markers with `jpeg_resync_to_restart`'s recovery, multi-scan files (a whole-image coefficient buffer,
//! zeroed at the start and never re-zeroed between scans), the coefficient controller's block smoothing, the IDCT with libjpeg's
//! range-limit table (wrapping 32-bit arithmetic, as C's `int` does on common targets), every upsampling method libjpeg selects (full size,
//! h2v1 and h2v2 fancy and plain, the generic integer replication; edge rows duplicated as `jdmainct.c`'s context pointers do), and
//! YCbCr to RGB with `jdcolor.c`'s fixed-point tables.
//!
//! **The data source** is TGE's `jdatasrc.c`: at the end of the file it inserts a fake EOI marker (`FF D9`) on every refill, after a
//! warning, so a truncated file decodes: the entropy decoder meets the marker, stuffs zero bits and the remaining MCUs come out as
//! libjpeg makes them. An empty file is an error (`JERR_INPUT_EMPTY`).
//!
//! **Errors.** Every `ERREXIT` of the path is [`JpegError::Fatal`] with the libjpeg code's name (libjpeg's default `error_exit`,
//! `jpeg_std_error`'s, exits the process). Warnings (`WARNMS`) are counted in [`Info::warnings`] and decoding goes on, as there.
//! In a single-scan file libjpeg reads the markers after the image to EOI in `jpeg_finish_decompress`; an error there is an error here.
//!
//! **Caps** ([`Limits`], defaults [`MAX_JPEG_DIMENSION`], [`MAX_JPEG_PIXELS`], [`MAX_JPEG_SCANS`], [`MAX_JPEG_SAMPLES_PER_BYTE`],
//! [`MAX_JPEG_BLOCK_VISITS_PER_BYTE`]): libjpeg's own limit is 65,500 a side and nothing else; this decoder refuses larger images and
//! more scans than these before allocating (deviations: libjpeg would decode them). The work is tied to the file's size, as PNG's is:
//! a frame may hold at most [`JPEG_SAMPLES_FLOOR`] samples (pixels times components) plus [`Limits::samples_per_byte`] per file byte,
//! so a header with no data behind it allocates nothing; and the scans together may visit at most
//! [`Limits::block_visits_per_byte`] per file byte (plus the floor), so scans that carry no data cannot be repeated for free; the
//! coefficient arrays grow a block row at a time as the scans reach them, so memory follows the data decoded. A scan whose entropy
//! decoder has no input left stops at once.
//!
//! **Known deviation:** an SOS that names one component twice while the scan count makes the file single-scan leaves libjpeg's main
//! buffer rows of the unnamed components uninitialised; here they come out as the IDCT of zero coefficients (mid grey).

mod entropy;
mod idct;
mod markers;
mod output;

#[cfg(test)]
mod tests;

use std::fmt;

use crate::{Bitmap, Format};

use markers::{HuffTable, Source};

/// The largest side this decoder accepts (libjpeg's `JPEG_MAX_DIMENSION` is 65,500).
pub const MAX_JPEG_DIMENSION: u32 = 16_384;
/// The most pixels this decoder accepts.
pub const MAX_JPEG_PIXELS: u64 = 1 << 24;
/// The most scans this decoder accepts (libjpeg has no limit; a typical progressive file has about ten).
pub const MAX_JPEG_SCANS: u32 = 256;
/// The samples (pixels times components) a frame may hold for each byte of the file, beyond [`JPEG_SAMPLES_FLOOR`]. A real file at
/// its best compression stays far under 1,000 pixels a byte.
pub const MAX_JPEG_SAMPLES_PER_BYTE: u64 = 4096;
/// The samples any frame may hold whatever the file's size, so that small honest files always pass.
pub const JPEG_SAMPLES_FLOOR: u64 = 1 << 16;
/// The block visits the scans may make for each byte of the file, beyond [`JPEG_SAMPLES_FLOOR`]. A real file makes a handful of
/// visits for each block (about five for a progressive one) and spends at least an eighth of a byte on each block, so a flat
/// progressive image stays under a quarter of this.
pub const MAX_JPEG_BLOCK_VISITS_PER_BYTE: u64 = 256;

/// libjpeg's fixed limits (`jpeglib.h`, `jmorecfg.h`).
const MAX_COMPONENTS: usize = 10;
const MAX_COMPS_IN_SCAN: usize = 4;
const MAX_SAMP_FACTOR: u8 = 4;
const D_MAX_BLOCKS_IN_MCU: usize = 10;
const NUM_TABLES: usize = 4;
const LIBJPEG_MAX_DIMENSION: u32 = 65_500;

/// The caps a decode runs under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_dimension: u32,
    pub max_pixels: u64,
    pub max_scans: u32,
    /// Samples a frame may hold for each byte of the file (on top of [`JPEG_SAMPLES_FLOOR`]).
    pub samples_per_byte: u64,
    /// Block visits the scans may make for each byte of the file (on top of [`JPEG_SAMPLES_FLOOR`]).
    pub block_visits_per_byte: u64,
}

impl Limits {
    /// The samples a file of `len` bytes may hold.
    fn sample_allowance(&self, len: usize) -> u64 {
        JPEG_SAMPLES_FLOOR.saturating_add(self.samples_per_byte.saturating_mul(len as u64))
    }
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            max_dimension: MAX_JPEG_DIMENSION,
            max_pixels: MAX_JPEG_PIXELS,
            max_scans: MAX_JPEG_SCANS,
            samples_per_byte: MAX_JPEG_SAMPLES_PER_BYTE,
            block_visits_per_byte: MAX_JPEG_BLOCK_VISITS_PER_BYTE,
        }
    }
}

/// Why a JPEG did not decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JpegError {
    /// libjpeg's `error_exit`, by the name of its message code (`JERR_...`).
    Fatal(&'static str),
    /// `readJPEG` returns false: the output colour space is neither greyscale nor RGB.
    ColorSpace(&'static str),
    /// A size above one of the [`Limits`].
    Cap {
        what: &'static str,
        value: u64,
        cap: u64,
    },
}

impl fmt::Display for JpegError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JpegError::Fatal(code) => write!(f, "libjpeg error {code}"),
            JpegError::ColorSpace(cs) => write!(
                f,
                "output colour space {cs} (readJPEG takes greyscale or RGB)"
            ),
            JpegError::Cap { what, value, cap } => {
                write!(f, "{what}: {value} is above the limit of {cap}")
            }
        }
    }
}

impl std::error::Error for JpegError {}

/// What a decode saw (for corpus statistics and reports).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Info {
    pub progressive: bool,
    /// Sampling factors `(h, v)` per component, in frame order.
    pub sampling: Vec<(u8, u8)>,
    pub scans: u32,
    /// A non-zero restart interval was in force for some scan.
    pub restarts: bool,
    /// The colour space libjpeg took the file to be in: `gray`, `ycc` or `rgb`.
    pub color_space: &'static str,
    /// Block smoothing ran.
    pub smoothed: bool,
    /// libjpeg warnings (corrupt data, a fake EOI, a bogus progression, ...).
    pub warnings: u32,
    /// Coefficient blocks allocated: those the scans reached (a frame with no data behind it allocates none past its first row).
    pub coefficient_blocks: usize,
}

/// The size the frame header gives (see [`crate::size_of`]): the markers after `SOI` are walked as libjpeg's `next_marker` walks
/// them (fill bytes and stray bytes skipped, the stand-alone markers taken alone, every other segment skipped by its length) to the
/// first `SOF`; only the frame types the decoder takes count, within the default [`Limits`] (the samples-per-byte tie included).
pub fn header_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(..2)? != [0xff, 0xd8] {
        return None;
    }
    let mut at = 2usize;
    loop {
        // Skip to a 0xff, then past the fill bytes.
        while *bytes.get(at)? != 0xff {
            at += 1;
        }
        while *bytes.get(at)? == 0xff {
            at += 1;
        }
        let marker = *bytes.get(at)?;
        at += 1;
        match marker {
            // Stand-alone markers (TEM, RSTn, SOI): no length.
            0x01 | 0xd0..=0xd8 => continue,
            // EOI before a frame header, or SOS: no size.
            0xd9 | 0xda => return None,
            0xc0 | 0xc1 | 0xc2 | 0xc9 | 0xca => {
                let s = bytes.get(at..at + 8)?;
                let h = u32::from(u16::from_be_bytes([s[3], s[4]]));
                let w = u32::from(u16::from_be_bytes([s[5], s[6]]));
                let limits = Limits::default();
                let samples = u64::from(w) * u64::from(h) * u64::from(s[7]);
                let ok = w > 0
                    && h > 0
                    && w <= limits.max_dimension
                    && h <= limits.max_dimension
                    && u64::from(w) * u64::from(h) <= limits.max_pixels
                    && samples <= limits.sample_allowance(bytes.len());
                return ok.then_some((w, h));
            }
            // The frame types the decoder refuses.
            0xc3 | 0xc5..=0xc7 | 0xcb | 0xcd..=0xcf => return None,
            _ => {
                let s = bytes.get(at..at + 2)?;
                let len = usize::from(u16::from_be_bytes([s[0], s[1]]));
                if len < 2 {
                    return None;
                }
                at += len;
            }
        }
    }
}

/// Decodes a JPEG into the bits `readJPEG` leaves.
pub fn decode(bytes: &[u8]) -> Result<Bitmap, JpegError> {
    decode_with(bytes, &Limits::default()).map(|(b, _)| b)
}

/// [`decode`] under other caps, with what was seen.
pub fn decode_with(bytes: &[u8], limits: &Limits) -> Result<(Bitmap, Info), JpegError> {
    let mut d = Decoder::new(bytes, *limits);
    d.run()
}

/// The colour spaces of `jpeglib.h` the decoder can meet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColorSpace {
    Gray,
    Ycc,
    Rgb,
}

/// One component of the frame (`jpeg_component_info`).
#[derive(Debug, Clone)]
struct Component {
    id: u8,
    h: u8,
    v: u8,
    quant_no: u8,
    dc_tbl: u8,
    ac_tbl: u8,
    width_in_blocks: usize,
    height_in_blocks: usize,
    downsampled_width: usize,
    downsampled_height: usize,
    /// The coefficient array's size in blocks: padded to whole MCUs (`jround_up(width_in_blocks, h)`, the same for the rows).
    blocks_w: usize,
    blocks_h: usize,
    /// The quantisation table latched at the first scan of the component.
    quant: Option<[u16; 64]>,
    /// The coefficients, `blocks_w * blocks_h` blocks in natural order.
    coefs: Vec<[i16; 64]>,
}

/// The parameters of the scan being decoded.
#[derive(Debug, Clone, Default)]
struct Scan {
    /// Indices into the frame's components, in SOS order.
    comps: Vec<usize>,
    ss: u8,
    se: u8,
    ah: u8,
    al: u8,
}

/// The decompressor: the state libjpeg keeps in `jpeg_decompress_struct` and its modules, for one image.
struct Decoder<'a> {
    src: Source<'a>,
    limits: Limits,
    info: Info,
    /// A marker read and not yet processed (0 = none).
    unread_marker: u8,
    saw_soi: bool,
    saw_sof: bool,
    restart_interval: u32,
    saw_jfif: bool,
    saw_adobe: bool,
    adobe_transform: u8,
    quant: [Option<[u16; 64]>; NUM_TABLES],
    dc_tables: [Option<HuffTable>; NUM_TABLES],
    ac_tables: [Option<HuffTable>; NUM_TABLES],
    progressive: bool,
    arith: bool,
    precision: u8,
    width: u32,
    height: u32,
    comps: Vec<Component>,
    scan: Scan,
    max_h: u8,
    max_v: u8,
    has_multiple_scans: bool,
    /// Per component, the successive-approximation bit each coefficient is known to (`coef_bits`, -1 = not yet); progressive only.
    coef_bits: Vec<[i32; 64]>,
    entropy: entropy::State,
    /// The block visits the scans have made so far, and the most they may make (see [`Limits`]; set when the frame is accepted).
    visits: u64,
    visit_cap: u64,
}

impl<'a> Decoder<'a> {
    fn new(bytes: &'a [u8], limits: Limits) -> Decoder<'a> {
        Decoder {
            src: Source::new(bytes),
            limits,
            info: Info::default(),
            unread_marker: 0,
            saw_soi: false,
            saw_sof: false,
            restart_interval: 0,
            saw_jfif: false,
            saw_adobe: false,
            adobe_transform: 0,
            quant: [None; NUM_TABLES],
            dc_tables: [None, None, None, None],
            ac_tables: [None, None, None, None],
            progressive: false,
            arith: false,
            precision: 0,
            width: 0,
            height: 0,
            comps: Vec::new(),
            scan: Scan::default(),
            max_h: 1,
            max_v: 1,
            has_multiple_scans: false,
            coef_bits: Vec::new(),
            entropy: entropy::State::default(),
            visits: 0,
            visit_cap: 0,
        }
    }

    fn warn(&mut self) {
        self.info.warnings = self.info.warnings.saturating_add(1);
    }

    /// `jpeg_read_header`, `jpeg_start_decompress`, the scanline reads and `jpeg_finish_decompress`, as `readJPEG` calls them.
    fn run(&mut self) -> Result<(Bitmap, Info), JpegError> {
        if self.src.is_empty() {
            return Err(JpegError::Fatal("JERR_INPUT_EMPTY"));
        }
        // jpeg_read_header(cinfo, TRUE): to the first SOS.
        match self.read_markers()? {
            markers::Reached::Sos => {}
            markers::Reached::Eoi => {
                return Err(JpegError::Fatal(if self.saw_sof {
                    "JERR_SOF_NO_SOS"
                } else {
                    "JERR_NO_IMAGE"
                }));
            }
        }
        self.initial_setup()?;
        let (jpeg_cs, format) = self.default_color_space()?;
        self.master_selection()?;
        // The first scan, then (multi-scan) every scan to EOI; a single-scan file reads its trailing markers to EOI in
        // `jpeg_finish_decompress`. Either way the whole stream to EOI is read before the result counts.
        self.info.scans = 1;
        self.decode_scan()?;
        loop {
            match self.read_markers()? {
                markers::Reached::Eoi => break,
                markers::Reached::Sos => {
                    if !self.has_multiple_scans {
                        return Err(JpegError::Fatal("JERR_EOI_EXPECTED"));
                    }
                    self.info.scans += 1;
                    if self.info.scans > self.limits.max_scans {
                        return Err(JpegError::Cap {
                            what: "jpeg scans",
                            value: u64::from(self.info.scans),
                            cap: u64::from(self.limits.max_scans),
                        });
                    }
                    self.decode_scan()?;
                }
            }
        }
        let bits = output::render(self, jpeg_cs)?;
        self.info.progressive = self.progressive;
        self.info.coefficient_blocks = self.comps.iter().map(|c| c.coefs.len()).sum();
        self.info.sampling = self.comps.iter().map(|c| (c.h, c.v)).collect();
        self.info.color_space = match jpeg_cs {
            ColorSpace::Gray => "gray",
            ColorSpace::Ycc => "ycc",
            ColorSpace::Rgb => "rgb",
        };
        Ok((
            Bitmap {
                width: self.width,
                height: self.height,
                format,
                bits,
            },
            self.info.clone(),
        ))
    }

    /// `initial_setup` (jdinput.c), at the first SOS: the frame's checks and the component sizes.
    fn initial_setup(&mut self) -> Result<(), JpegError> {
        if self.height > LIBJPEG_MAX_DIMENSION || self.width > LIBJPEG_MAX_DIMENSION {
            return Err(JpegError::Fatal("JERR_IMAGE_TOO_BIG"));
        }
        if self.precision != 8 {
            return Err(JpegError::Fatal("JERR_BAD_PRECISION"));
        }
        if self.comps.len() > MAX_COMPONENTS {
            return Err(JpegError::Fatal("JERR_COMPONENT_COUNT"));
        }
        let (mut max_h, mut max_v) = (1u8, 1u8);
        for c in &self.comps {
            if c.h == 0 || c.h > MAX_SAMP_FACTOR || c.v == 0 || c.v > MAX_SAMP_FACTOR {
                return Err(JpegError::Fatal("JERR_BAD_SAMPLING"));
            }
            max_h = max_h.max(c.h);
            max_v = max_v.max(c.v);
        }
        self.max_h = max_h;
        self.max_v = max_v;
        let (w, h) = (self.width as usize, self.height as usize);
        let (mh, mv) = (usize::from(max_h), usize::from(max_v));
        for c in &mut self.comps {
            let (ch, cv) = (usize::from(c.h), usize::from(c.v));
            c.width_in_blocks = (w * ch).div_ceil(mh * 8);
            c.height_in_blocks = (h * cv).div_ceil(mv * 8);
            c.downsampled_width = (w * ch).div_ceil(mh);
            c.downsampled_height = (h * cv).div_ceil(mv);
            c.blocks_w = c.width_in_blocks.div_ceil(ch) * ch;
            c.blocks_h = c.height_in_blocks.div_ceil(cv) * cv;
            c.quant = None;
        }
        self.has_multiple_scans = self.scan.comps.len() < self.comps.len() || self.progressive;
        Ok(())
    }

    /// `default_decompress_parms` (jdapimin.c) and `readJPEG`'s check of the output colour space.
    fn default_color_space(&mut self) -> Result<(ColorSpace, Format), JpegError> {
        match self.comps.len() {
            1 => Ok((ColorSpace::Gray, Format::Alpha)),
            3 => {
                let cs = if self.saw_jfif {
                    ColorSpace::Ycc
                } else if self.saw_adobe {
                    match self.adobe_transform {
                        0 => ColorSpace::Rgb,
                        1 => ColorSpace::Ycc,
                        _ => {
                            self.warn();
                            ColorSpace::Ycc
                        }
                    }
                } else {
                    let ids = [self.comps[0].id, self.comps[1].id, self.comps[2].id];
                    if ids == [82, 71, 66] {
                        ColorSpace::Rgb
                    } else {
                        ColorSpace::Ycc
                    }
                };
                Ok((cs, Format::Rgb))
            }
            4 => {
                if self.saw_adobe && !matches!(self.adobe_transform, 0 | 2) {
                    self.warn();
                }
                Err(JpegError::ColorSpace("CMYK"))
            }
            _ => Err(JpegError::ColorSpace("unknown")),
        }
    }

    /// The checks `master_selection` (jdmaster.c) makes before decoding, this decoder's caps, and the coefficient buffers.
    fn master_selection(&mut self) -> Result<(), JpegError> {
        let limits = self.limits;
        for (what, side) in [("jpeg width", self.width), ("jpeg height", self.height)] {
            if side > limits.max_dimension {
                return Err(JpegError::Cap {
                    what,
                    value: u64::from(side),
                    cap: u64::from(limits.max_dimension),
                });
            }
        }
        let pixels = u64::from(self.width) * u64::from(self.height);
        if pixels > limits.max_pixels {
            return Err(JpegError::Cap {
                what: "jpeg pixels",
                value: pixels,
                cap: limits.max_pixels,
            });
        }
        let samples = pixels.saturating_mul(self.comps.len() as u64);
        let allowed = limits.sample_allowance(self.src.len());
        if samples > allowed {
            return Err(JpegError::Cap {
                what: "jpeg samples for the file's size",
                value: samples,
                cap: allowed,
            });
        }
        self.visit_cap = limits
            .block_visits_per_byte
            .saturating_mul(self.src.len() as u64)
            .saturating_add(JPEG_SAMPLES_FLOOR);
        // The upsampler's method per component (jinit_upsampler); a fractional ratio is an error.
        for ci in 0..self.comps.len() {
            output::method(self, ci)?;
        }
        if self.arith {
            return Err(JpegError::Fatal("JERR_ARITH_NOTIMPL"));
        }
        // The whole-image coefficient arrays, pre-zeroed in libjpeg (jinit_d_coef_controller), grow here a block row at a time as
        // the scans reach them (`decode_scan`); a block never reached reads as zeros, which is what the zeroed array holds. So the
        // memory is what the scans decoded, and a frame with no data behind it allocates none. Only one or three components get
        // here (the colour-space check), and each array is at most (width / 8 + 4) x (height / 8 + 4) blocks.
        if self.progressive {
            self.coef_bits = vec![[-1i32; 64]; self.comps.len()];
        }
        Ok(())
    }

    /// Whether the latch can no longer end in this scan: the input is all consumed, or the marker the entropy data ran into is not
    /// a restart marker (`jpeg_resync_to_restart` keeps such a marker unread and the latch set).
    fn restart_is_out_of_reach(&self) -> bool {
        self.src.exhausted()
            || (self.unread_marker != 0 && !(0xd0..=0xd7).contains(&self.unread_marker))
    }

    /// `start_input_pass` and the scan's MCUs (`consume_data` / `decompress_onepass` input side).
    fn decode_scan(&mut self) -> Result<(), JpegError> {
        // per_scan_setup
        let interleaved = self.scan.comps.len() > 1;
        let (mcus_per_row, mcu_rows) = if interleaved {
            let mut blocks = 0usize;
            for &ci in &self.scan.comps {
                blocks += usize::from(self.comps[ci].h) * usize::from(self.comps[ci].v);
                if blocks > D_MAX_BLOCKS_IN_MCU {
                    return Err(JpegError::Fatal("JERR_BAD_MCU_SIZE"));
                }
            }
            (
                (self.width as usize).div_ceil(usize::from(self.max_h) * 8),
                (self.height as usize).div_ceil(usize::from(self.max_v) * 8),
            )
        } else {
            let c = &self.comps[self.scan.comps[0]];
            (c.width_in_blocks, c.height_in_blocks)
        };
        // latch_quant_tables
        for k in 0..self.scan.comps.len() {
            let ci = self.scan.comps[k];
            if self.comps[ci].quant.is_none() {
                let no = usize::from(self.comps[ci].quant_no);
                match self.quant.get(no).copied().flatten() {
                    Some(q) => self.comps[ci].quant = Some(q),
                    None => return Err(JpegError::Fatal("JERR_NO_QUANT_TABLE")),
                }
            }
        }
        if self.restart_interval != 0 {
            self.info.restarts = true;
        }
        let tables = self.start_entropy_pass()?;
        // The blocks of each MCU, as (component, block row, block column), in MCU order.
        let mut layout: Vec<(usize, usize, usize)> = Vec::with_capacity(D_MAX_BLOCKS_IN_MCU);
        let mut mcu = [[0i16; 64]; D_MAX_BLOCKS_IN_MCU];
        let mut membership = [0usize; D_MAX_BLOCKS_IN_MCU];
        let blocks_per_mcu = if interleaved {
            self.scan
                .comps
                .iter()
                .map(|&ci| usize::from(self.comps[ci].h) * usize::from(self.comps[ci].v))
                .sum()
        } else {
            1
        };
        let dc_refine = self.progressive && self.scan.ss == 0 && self.scan.ah != 0;
        'rows: for mr in 0..mcu_rows {
            let visits = (mcus_per_row as u64).saturating_mul(blocks_per_mcu as u64);
            self.visits = self.visits.saturating_add(visits);
            if self.visits > self.visit_cap {
                return Err(JpegError::Cap {
                    what: "jpeg block visits for the file's size",
                    value: self.visits,
                    cap: self.visit_cap,
                });
            }
            for &ci in &self.scan.comps {
                let c = &mut self.comps[ci];
                let band = if interleaved { usize::from(c.v) } else { 1 };
                let rows = ((mr + 1) * band).min(c.blocks_h);
                if c.coefs.len() < rows * c.blocks_w {
                    c.coefs.resize(rows * c.blocks_w, [0; 64]);
                }
            }
            for mc in 0..mcus_per_row {
                // Zero bits change nothing once the entropy decoder has run out of data (a DC refinement scan does not look at the
                // latch, but zero bits change nothing there either). Only a restart marker later in the input can end the latch.
                if self.entropy.insufficient
                    && (self.restart_is_out_of_reach()
                        || (self.restart_interval == 0 && !dc_refine))
                {
                    break 'rows;
                }
                layout.clear();
                if interleaved {
                    for (k, &ci) in self.scan.comps.iter().enumerate() {
                        let c = &self.comps[ci];
                        for yi in 0..usize::from(c.v) {
                            for xi in 0..usize::from(c.h) {
                                membership[layout.len()] = k;
                                layout.push((
                                    ci,
                                    mr * usize::from(c.v) + yi,
                                    mc * usize::from(c.h) + xi,
                                ));
                            }
                        }
                    }
                } else {
                    membership[0] = 0;
                    layout.push((self.scan.comps[0], mr, mc));
                }
                for (slot, &(ci, r, col)) in layout.iter().enumerate() {
                    let c = &self.comps[ci];
                    mcu[slot] = c
                        .coefs
                        .get(r * c.blocks_w + col)
                        .copied()
                        .unwrap_or([0; 64]);
                }
                let n = layout.len();
                self.decode_mcu(&tables, &mut mcu[..n], &membership[..n])?;
                for (slot, &(ci, r, col)) in layout.iter().enumerate() {
                    let c = &mut self.comps[ci];
                    let bw = c.blocks_w;
                    if let Some(b) = c.coefs.get_mut(r * bw + col) {
                        *b = mcu[slot];
                    }
                }
            }
        }
        Ok(())
    }
}

/// `jpeg_natural_order` with libjpeg's 16 extra entries of 63, which catch a corrupt run past the end of a block.
pub(crate) const NATURAL_ORDER: [usize; 80] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63, 63, 63, 63, 63, 63, 63, 63, 63, 63, 63,
    63, 63, 63, 63, 63, 63,
];

fn natural(k: i32) -> usize {
    usize::try_from(k)
        .ok()
        .and_then(|k| NATURAL_ORDER.get(k).copied())
        .unwrap_or(63)
}
