//! The data source and the marker reader (libjpeg 6b `jdatasrc.c` as TGE changed it, `jdmarker.c`, the marker half of `jdinput.c`).
//! Based in part on the work of the Independent JPEG Group; see [`super`] for the credits.

use super::{Component, Decoder, JpegError, MAX_COMPS_IN_SCAN, NUM_TABLES, Scan, natural};

/// The byte stream as TGE's `jdatasrc.c` delivers it: the file's bytes, then a fake EOI marker (`FF D9`) for every refill after the
/// end (each refill past the end gives two bytes and a warning). A skip past the end consumes fake bytes, as the "dumb" skipper
/// refilling in a loop does, so the stream is the file followed by `FF D9` repeated forever.
pub(super) struct Source<'a> {
    data: &'a [u8],
    pos: usize,
    /// Whether the end was passed (one warning, `JWRN_JPEG_EOF`; libjpeg gives one per refill).
    pub(super) hit_eof: bool,
}

impl<'a> Source<'a> {
    pub(super) fn new(data: &'a [u8]) -> Source<'a> {
        Source {
            data,
            pos: 0,
            hit_eof: false,
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Whether every byte of the file has been consumed (what follows is the fake EOI).
    pub(super) fn exhausted(&self) -> bool {
        self.pos >= self.data.len()
    }

    /// The file's length in bytes (the fake EOIs after it do not count).
    pub(super) fn len(&self) -> usize {
        self.data.len()
    }

    pub(super) fn byte(&mut self) -> u8 {
        let b = match self.data.get(self.pos) {
            Some(&b) => b,
            None => {
                self.hit_eof = true;
                if (self.pos - self.data.len()).is_multiple_of(2) {
                    0xff
                } else {
                    0xd9
                }
            }
        };
        self.pos = self.pos.saturating_add(1);
        b
    }

    fn skip(&mut self, n: usize) {
        self.pos = self.pos.saturating_add(n);
        if self.pos > self.data.len() {
            self.hit_eof = true;
        }
    }
}

/// A Huffman table as a DHT defines it (`JHUFF_TBL`): the code counts per length (`bits[1..=16]`) and the symbols.
#[derive(Debug, Clone, Copy)]
pub(super) struct HuffTable {
    pub(super) bits: [u8; 17],
    pub(super) huffval: [u8; 256],
}

/// What `read_markers` stopped at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Reached {
    Sos,
    Eoi,
}

const M_SOF0: u8 = 0xc0;
const M_SOF1: u8 = 0xc1;
const M_SOF2: u8 = 0xc2;
const M_DHT: u8 = 0xc4;
const M_SOF9: u8 = 0xc9;
const M_SOF10: u8 = 0xca;
const M_DAC: u8 = 0xcc;
const M_RST0: u8 = 0xd0;
const M_RST7: u8 = 0xd7;
const M_SOI: u8 = 0xd8;
const M_EOI: u8 = 0xd9;
const M_SOS: u8 = 0xda;
const M_DQT: u8 = 0xdb;
const M_DNL: u8 = 0xdc;
const M_DRI: u8 = 0xdd;
const M_APP0: u8 = 0xe0;
const M_APP14: u8 = 0xee;
const M_APP15: u8 = 0xef;
const M_COM: u8 = 0xfe;
const M_TEM: u8 = 0x01;

impl Decoder<'_> {
    fn byte(&mut self) -> u8 {
        self.src.byte()
    }

    fn two_bytes(&mut self) -> i32 {
        let hi = i32::from(self.byte());
        let lo = i32::from(self.byte());
        (hi << 8) | lo
    }

    /// `read_markers`: markers until SOS or EOI.
    pub(super) fn read_markers(&mut self) -> Result<Reached, JpegError> {
        loop {
            if self.unread_marker == 0 {
                if !self.saw_soi {
                    self.first_marker()?;
                } else {
                    self.next_marker();
                }
            }
            match self.unread_marker {
                M_SOI => self.get_soi()?,
                M_SOF0 | M_SOF1 => self.get_sof(false, false)?,
                M_SOF2 => self.get_sof(true, false)?,
                M_SOF9 => self.get_sof(false, true)?,
                M_SOF10 => self.get_sof(true, true)?,
                0xc3 | 0xc5 | 0xc6 | 0xc7 | 0xc8 | 0xcb | 0xcd | 0xce | 0xcf => {
                    return Err(JpegError::Fatal("JERR_SOF_UNSUPPORTED"));
                }
                M_SOS => {
                    self.get_sos()?;
                    self.unread_marker = 0;
                    return Ok(Reached::Sos);
                }
                M_EOI => {
                    self.unread_marker = 0;
                    return Ok(Reached::Eoi);
                }
                // 6b is built without arithmetic coding: a DAC is skipped (its SOF9/10 is refused later).
                M_DAC => self.skip_variable(),
                M_DHT => self.get_dht()?,
                M_DQT => self.get_dqt()?,
                M_DRI => self.get_dri()?,
                M_APP0 | M_APP14 => self.get_interesting_appn(),
                0xe1..=M_APP15 | M_COM => self.skip_variable(),
                M_RST0..=M_RST7 | M_TEM => {}
                M_DNL => self.skip_variable(),
                _ => return Err(JpegError::Fatal("JERR_UNKNOWN_MARKER")),
            }
            self.unread_marker = 0;
        }
    }

    /// `first_marker`: the file must start with `FF D8`.
    fn first_marker(&mut self) -> Result<(), JpegError> {
        let c = self.byte();
        let c2 = self.byte();
        if c != 0xff || c2 != M_SOI {
            return Err(JpegError::Fatal("JERR_NO_SOI"));
        }
        self.unread_marker = c2;
        Ok(())
    }

    /// `next_marker`: skips to the next `FF xx` with `xx` neither 0 nor FF (garbage before it is a warning; `FF 00` is skipped).
    pub(super) fn next_marker(&mut self) {
        let mut discarded = false;
        let c = loop {
            let mut c = self.byte();
            while c != 0xff {
                discarded = true;
                c = self.byte();
            }
            loop {
                c = self.byte();
                if c != 0xff {
                    break;
                }
            }
            if c != 0 {
                break c;
            }
            discarded = true;
        };
        if discarded || self.entropy.discarded_pending {
            self.warn();
        }
        self.entropy.discarded_pending = false;
        self.unread_marker = c;
    }

    fn get_soi(&mut self) -> Result<(), JpegError> {
        if self.saw_soi {
            return Err(JpegError::Fatal("JERR_SOI_DUPLICATE"));
        }
        self.restart_interval = 0;
        self.saw_jfif = false;
        self.saw_adobe = false;
        self.adobe_transform = 0;
        self.saw_soi = true;
        Ok(())
    }

    fn get_sof(&mut self, progressive: bool, arith: bool) -> Result<(), JpegError> {
        self.progressive = progressive;
        self.arith = arith;
        let length = self.two_bytes();
        let precision = self.byte();
        let height = self.two_bytes();
        let width = self.two_bytes();
        let n = usize::from(self.byte());
        if self.saw_sof {
            return Err(JpegError::Fatal("JERR_SOF_DUPLICATE"));
        }
        self.precision = precision;
        self.height = height as u32;
        self.width = width as u32;
        if height <= 0 || width <= 0 || n == 0 {
            return Err(JpegError::Fatal("JERR_EMPTY_IMAGE"));
        }
        if length - 8 != (n * 3) as i32 {
            return Err(JpegError::Fatal("JERR_BAD_LENGTH"));
        }
        self.comps.clear();
        for _ in 0..n {
            let id = self.byte();
            let hv = self.byte();
            let quant_no = self.byte();
            self.comps.push(Component {
                id,
                h: (hv >> 4) & 15,
                v: hv & 15,
                quant_no,
                dc_tbl: 0,
                ac_tbl: 0,
                width_in_blocks: 0,
                height_in_blocks: 0,
                downsampled_width: 0,
                downsampled_height: 0,
                blocks_w: 0,
                blocks_h: 0,
                quant: None,
                coefs: Vec::new(),
            });
        }
        self.saw_sof = true;
        Ok(())
    }

    fn get_sos(&mut self) -> Result<(), JpegError> {
        if !self.saw_sof {
            return Err(JpegError::Fatal("JERR_SOS_NO_SOF"));
        }
        let length = self.two_bytes();
        let n = usize::from(self.byte());
        if length != (n * 2 + 6) as i32 || !(1..=MAX_COMPS_IN_SCAN).contains(&n) {
            return Err(JpegError::Fatal("JERR_BAD_LENGTH"));
        }
        let mut comps = Vec::with_capacity(n);
        for _ in 0..n {
            let cc = self.byte();
            let c = self.byte();
            // The first component with that id (a duplicated id in the frame finds the first).
            let ci = self
                .comps
                .iter()
                .position(|comp| comp.id == cc)
                .ok_or(JpegError::Fatal("JERR_BAD_COMPONENT_ID"))?;
            self.comps[ci].dc_tbl = (c >> 4) & 15;
            self.comps[ci].ac_tbl = c & 15;
            comps.push(ci);
        }
        let ss = self.byte();
        let se = self.byte();
        let a = self.byte();
        self.scan = Scan {
            comps,
            ss,
            se,
            ah: (a >> 4) & 15,
            al: a & 15,
        };
        self.entropy.next_restart_num = 0;
        Ok(())
    }

    fn get_dht(&mut self) -> Result<(), JpegError> {
        let mut length = self.two_bytes() - 2;
        while length > 16 {
            let index = i32::from(self.byte());
            let mut bits = [0u8; 17];
            let mut count = 0i32;
            for b in bits.iter_mut().skip(1) {
                *b = self.byte();
                count += i32::from(*b);
            }
            length -= 1 + 16;
            if count > 256 || count > length {
                return Err(JpegError::Fatal("JERR_BAD_HUFF_TABLE"));
            }
            let mut huffval = [0u8; 256];
            for v in huffval.iter_mut().take(count as usize) {
                *v = self.byte();
            }
            length -= count;
            let (ac, index) = if index & 0x10 != 0 {
                (true, index - 0x10)
            } else {
                (false, index)
            };
            let slot = usize::try_from(index)
                .ok()
                .filter(|&i| i < NUM_TABLES)
                .ok_or(JpegError::Fatal("JERR_DHT_INDEX"))?;
            let table = Some(HuffTable { bits, huffval });
            if ac {
                self.ac_tables[slot] = table;
            } else {
                self.dc_tables[slot] = table;
            }
        }
        if length != 0 {
            return Err(JpegError::Fatal("JERR_BAD_LENGTH"));
        }
        Ok(())
    }

    fn get_dqt(&mut self) -> Result<(), JpegError> {
        let mut length = self.two_bytes() - 2;
        while length > 0 {
            let n = self.byte();
            let prec = n >> 4;
            let n = usize::from(n & 0x0f);
            if n >= NUM_TABLES {
                return Err(JpegError::Fatal("JERR_DQT_INDEX"));
            }
            let mut table = self.quant[n].unwrap_or([0; 64]);
            for i in 0..64 {
                let v = if prec != 0 {
                    self.two_bytes() as u16
                } else {
                    u16::from(self.byte())
                };
                table[natural(i)] = v;
            }
            self.quant[n] = Some(table);
            length -= 64 + 1;
            if prec != 0 {
                length -= 64;
            }
        }
        if length != 0 {
            return Err(JpegError::Fatal("JERR_BAD_LENGTH"));
        }
        Ok(())
    }

    fn get_dri(&mut self) -> Result<(), JpegError> {
        if self.two_bytes() != 4 {
            return Err(JpegError::Fatal("JERR_BAD_LENGTH"));
        }
        self.restart_interval = self.two_bytes() as u32;
        Ok(())
    }

    /// APP0 and APP14: the first 14 bytes are examined for JFIF and Adobe, the rest skipped.
    fn get_interesting_appn(&mut self) {
        let mut length = self.two_bytes() - 2;
        let count = length.clamp(0, 14) as usize;
        let mut b = [0u8; 14];
        for v in b.iter_mut().take(count) {
            *v = self.byte();
        }
        length -= count as i32;
        let data = &b[..count];
        if self.unread_marker == M_APP0 {
            if data.len() >= 14 && data[..5] == *b"JFIF\0" {
                self.saw_jfif = true;
                if data[5] != 1 {
                    self.warn();
                }
            }
        } else if data.len() >= 12 && data[..5] == *b"Adobe" {
            self.saw_adobe = true;
            self.adobe_transform = data[11];
        }
        if length > 0 {
            self.src.skip(length as usize);
        }
    }

    fn skip_variable(&mut self) {
        let length = self.two_bytes() - 2;
        if length > 0 {
            self.src.skip(length as usize);
        }
    }

    /// `read_restart_marker`: the expected RSTn, else `jpeg_resync_to_restart`'s recovery.
    pub(super) fn read_restart_marker(&mut self) {
        if self.unread_marker == 0 {
            self.next_marker();
        }
        let desired = self.entropy.next_restart_num;
        if self.unread_marker == M_RST0 + desired {
            self.unread_marker = 0;
        } else {
            self.resync_to_restart(desired);
        }
        self.entropy.next_restart_num = (desired + 1) & 7;
    }

    /// `jpeg_resync_to_restart`: discard the marker (1), scan to the next one and decide again (2), or leave it unread so the decoder
    /// reads an empty segment (3).
    fn resync_to_restart(&mut self, desired: u8) {
        self.warn();
        let rst = |k: u8| M_RST0 + (k & 7);
        loop {
            let marker = self.unread_marker;
            let action = if marker < M_SOF0 {
                2
            } else if !(M_RST0..=M_RST7).contains(&marker)
                || marker == rst(desired.wrapping_add(1))
                || marker == rst(desired.wrapping_add(2))
            {
                // A valid non-restart marker, or one of the next two restarts: leave it unread.
                3
            } else if marker == rst(desired.wrapping_sub(1))
                || marker == rst(desired.wrapping_sub(2))
            {
                2
            } else {
                1
            };
            match action {
                1 => {
                    self.unread_marker = 0;
                    return;
                }
                2 => self.next_marker(),
                _ => return,
            }
        }
    }
}
