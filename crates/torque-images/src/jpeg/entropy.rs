//! The entropy decoders: the bit reader and the derived Huffman tables (`jdhuff.c`), sequential MCUs (`jdhuff.c`'s `decode_mcu`) and
//! the four progressive kinds (`jdphuff.c`). The data source never suspends, so every "suspend" path of the C is absent; a marker met
//! inside the entropy-coded data ends the segment: zero bits are stuffed and the "insufficient data" latch leaves the following MCUs of
//! the segment untouched (zero coefficients in a fresh block: mid grey).
//! Based in part on the work of the Independent JPEG Group; see [`super`] for the credits.

use super::markers::HuffTable;
use super::{Decoder, JpegError, NUM_TABLES, natural};

/// `MIN_GET_BITS` for a 32-bit bit buffer.
const MIN_GET_BITS: i32 = 25;
/// `HUFF_LOOKAHEAD`.
const LOOKAHEAD: i32 = 8;

/// The entropy decoder's state across MCUs.
#[derive(Debug, Clone, Default)]
pub(super) struct State {
    get_buffer: u32,
    bits_left: i32,
    insufficient: bool,
    restarts_to_go: u32,
    pub(super) next_restart_num: u8,
    eobrun: u32,
    last_dc: [i32; 4],
    /// Bytes thrown away at a restart (part of the next `next_marker` warning).
    pub(super) discarded_pending: bool,
}

/// `d_derived_tbl`: the decoding tables of one Huffman table.
#[derive(Debug, Clone)]
pub(super) struct Derived {
    maxcode: [i32; 18],
    valoffset: [i32; 17],
    look_nbits: [u8; 256],
    look_sym: [u8; 256],
    huffval: [u8; 256],
}

/// `jpeg_make_d_derived_tbl`.
fn make_derived(table: Option<&HuffTable>, is_dc: bool) -> Result<Derived, JpegError> {
    let t = table.ok_or(JpegError::Fatal("JERR_NO_HUFF_TABLE"))?;
    let bad = JpegError::Fatal("JERR_BAD_HUFF_TABLE");
    // Figure C.1: the code length of each symbol.
    let mut huffsize = [0u8; 257];
    let mut p = 0usize;
    for l in 1..=16u8 {
        let count = usize::from(t.bits[usize::from(l)]);
        if p + count > 256 {
            return Err(bad);
        }
        for _ in 0..count {
            huffsize[p] = l;
            p += 1;
        }
    }
    huffsize[p] = 0;
    let numsymbols = p;
    // Figure C.2: the codes, checking that each length's codes fit in it.
    let mut huffcode = [0u32; 257];
    let mut code = 0u32;
    let mut si = u32::from(huffsize[0]);
    p = 0;
    while huffsize[p] != 0 {
        while u32::from(huffsize[p]) == si {
            huffcode[p] = code;
            p += 1;
            code += 1;
        }
        if u64::from(code) >= (1u64 << si) {
            return Err(bad);
        }
        code <<= 1;
        si += 1;
    }
    // Figure F.15: the bit-serial decoding tables.
    let mut d = Derived {
        maxcode: [0; 18],
        valoffset: [0; 17],
        look_nbits: [0; 256],
        look_sym: [0; 256],
        huffval: t.huffval,
    };
    p = 0;
    for l in 1..=16usize {
        let count = usize::from(t.bits[l]);
        if count != 0 {
            d.valoffset[l] = p as i32 - huffcode[p] as i32;
            p += count;
            d.maxcode[l] = huffcode[p - 1] as i32;
        } else {
            d.maxcode[l] = -1;
        }
    }
    d.maxcode[17] = 0xfffff;
    // The 8-bit lookahead tables.
    p = 0;
    for l in 1..=LOOKAHEAD as usize {
        for _ in 0..usize::from(t.bits[l]) {
            let first = (huffcode[p] as usize) << (LOOKAHEAD as usize - l);
            for lookbits in first..first + (1usize << (LOOKAHEAD as usize - l)) {
                if let Some(n) = d.look_nbits.get_mut(lookbits) {
                    *n = l as u8;
                }
                if let Some(s) = d.look_sym.get_mut(lookbits) {
                    *s = t.huffval[p];
                }
            }
            p += 1;
        }
    }
    if is_dc && t.huffval[..numsymbols].iter().any(|&s| s > 15) {
        return Err(bad);
    }
    Ok(d)
}

/// The tables a scan decodes with, per component of the scan (in SOS order).
pub(super) struct Tables {
    dc: Vec<Option<Derived>>,
    ac: Vec<Option<Derived>>,
}

/// `HUFF_EXTEND`.
fn extend(r: i32, s: i32) -> i32 {
    if s == 0 {
        return r;
    }
    if r < (1 << (s - 1)) {
        r + ((-1i32) << s) + 1
    } else {
        r
    }
}

impl Decoder<'_> {
    /// `jpeg_fill_bit_buffer`: at least 25 bits, unless a marker is met; then, if `nbits` are wanted and not there, zeros.
    fn fill_bit_buffer(&mut self, nbits: i32) {
        if self.unread_marker == 0 {
            while self.entropy.bits_left < MIN_GET_BITS {
                let mut c = self.src.byte();
                if c == 0xff {
                    loop {
                        c = self.src.byte();
                        if c != 0xff {
                            break;
                        }
                    }
                    if c == 0 {
                        c = 0xff;
                    } else {
                        self.unread_marker = c;
                        self.no_more_bytes(nbits);
                        return;
                    }
                }
                self.entropy.get_buffer = (self.entropy.get_buffer << 8) | u32::from(c);
                self.entropy.bits_left += 8;
            }
        } else {
            self.no_more_bytes(nbits);
        }
    }

    fn no_more_bytes(&mut self, nbits: i32) {
        if nbits > self.entropy.bits_left {
            if !self.entropy.insufficient {
                self.warn();
                self.entropy.insufficient = true;
            }
            let shift = (MIN_GET_BITS - self.entropy.bits_left) as u32;
            self.entropy.get_buffer = self.entropy.get_buffer.checked_shl(shift).unwrap_or(0);
            self.entropy.bits_left = MIN_GET_BITS;
        }
    }

    fn check_bits(&mut self, nbits: i32) {
        if self.entropy.bits_left < nbits {
            self.fill_bit_buffer(nbits);
        }
    }

    fn get_bits(&mut self, nbits: i32) -> i32 {
        self.entropy.bits_left -= nbits;
        let v = self
            .entropy
            .get_buffer
            .checked_shr(self.entropy.bits_left as u32)
            .unwrap_or(0);
        (v & ((1u32 << nbits) - 1)) as i32
    }

    fn peek_bits(&self, nbits: i32) -> usize {
        let v = self
            .entropy
            .get_buffer
            .checked_shr((self.entropy.bits_left - nbits) as u32)
            .unwrap_or(0);
        (v & ((1u32 << nbits) - 1)) as usize
    }

    /// `HUFF_DECODE`: the lookahead when 8 bits are there, else (or for a longer code) `jpeg_huff_decode`.
    fn huff_decode(&mut self, t: &Derived) -> i32 {
        if self.entropy.bits_left < LOOKAHEAD {
            self.fill_bit_buffer(0);
            if self.entropy.bits_left < LOOKAHEAD {
                return self.huff_decode_slow(t, 1);
            }
        }
        let look = self.peek_bits(LOOKAHEAD);
        let nb = i32::from(t.look_nbits[look]);
        if nb != 0 {
            self.entropy.bits_left -= nb;
            i32::from(t.look_sym[look])
        } else {
            self.huff_decode_slow(t, LOOKAHEAD + 1)
        }
    }

    /// `jpeg_huff_decode`: the rest of the code one bit at a time; a code past 16 bits is garbage and decodes as 0 (a warning).
    fn huff_decode_slow(&mut self, t: &Derived, min_bits: i32) -> i32 {
        let mut l = min_bits;
        self.check_bits(l);
        let mut code = self.get_bits(l);
        while l < 17 && code > t.maxcode[l as usize] {
            code <<= 1;
            self.check_bits(1);
            code |= self.get_bits(1);
            l += 1;
        }
        if l > 16 {
            self.warn();
            return 0;
        }
        let i = code.wrapping_add(t.valoffset[l as usize]);
        usize::try_from(i)
            .ok()
            .and_then(|i| t.huffval.get(i))
            .map_or(0, |&v| i32::from(v))
    }

    /// The entropy decoder's `start_pass` (sequential or progressive): the checks, the derived tables, the state reset.
    pub(super) fn start_entropy_pass(&mut self) -> Result<Tables, JpegError> {
        let n = self.scan.comps.len();
        let mut tables = Tables {
            dc: vec![None; n],
            ac: vec![None; n],
        };
        let dc_of = |d: &Decoder<'_>, k: usize| {
            let no = usize::from(d.comps[d.scan.comps[k]].dc_tbl);
            if no >= NUM_TABLES {
                return Err(JpegError::Fatal("JERR_NO_HUFF_TABLE"));
            }
            make_derived(d.dc_tables[no].as_ref(), true)
        };
        let ac_of = |d: &Decoder<'_>, k: usize| {
            let no = usize::from(d.comps[d.scan.comps[k]].ac_tbl);
            if no >= NUM_TABLES {
                return Err(JpegError::Fatal("JERR_NO_HUFF_TABLE"));
            }
            make_derived(d.ac_tables[no].as_ref(), false)
        };
        let (ss, se, ah, al) = (self.scan.ss, self.scan.se, self.scan.ah, self.scan.al);
        if self.progressive {
            let is_dc = ss == 0;
            let mut bad = false;
            if is_dc {
                bad |= se != 0;
            } else {
                bad |= ss > se || se >= 64;
                bad |= n != 1;
            }
            if ah != 0 && al != ah.wrapping_sub(1) {
                bad = true;
            }
            bad |= al > 13;
            if bad {
                return Err(JpegError::Fatal("JERR_BAD_PROGRESSION"));
            }
            for k in 0..n {
                let ci = self.scan.comps[k];
                let mut warned = 0;
                if let Some(bits) = self.coef_bits.get_mut(ci) {
                    if !is_dc && bits[0] < 0 {
                        warned += 1;
                    }
                    for bit in &mut bits[usize::from(ss)..=usize::from(se)] {
                        if i32::from(ah) != (*bit).max(0) {
                            warned += 1;
                        }
                        *bit = i32::from(al);
                    }
                }
                for _ in 0..warned {
                    self.warn();
                }
            }
            for k in 0..n {
                if is_dc {
                    if ah == 0 {
                        tables.dc[k] = Some(dc_of(self, k)?);
                    }
                } else {
                    tables.ac[k] = Some(ac_of(self, k)?);
                }
            }
        } else {
            if ss != 0 || se != 63 || ah != 0 || al != 0 {
                self.warn();
            }
            for k in 0..n {
                tables.dc[k] = Some(dc_of(self, k)?);
                tables.ac[k] = Some(ac_of(self, k)?);
            }
        }
        self.entropy.last_dc = [0; 4];
        self.entropy.bits_left = 0;
        self.entropy.get_buffer = 0;
        self.entropy.insufficient = false;
        self.entropy.eobrun = 0;
        self.entropy.restarts_to_go = self.restart_interval;
        Ok(tables)
    }

    /// `process_restart`.
    fn process_restart(&mut self) {
        if self.entropy.bits_left >= 8 {
            self.entropy.discarded_pending = true;
        }
        self.entropy.bits_left = 0;
        self.read_restart_marker();
        self.entropy.last_dc = [0; 4];
        self.entropy.eobrun = 0;
        self.entropy.restarts_to_go = self.restart_interval;
        if self.unread_marker == 0 {
            self.entropy.insufficient = false;
        }
    }

    /// One MCU of the current scan. `mcu` holds the MCU's blocks (their current coefficients), `membership` the scan component of each.
    pub(super) fn decode_mcu(
        &mut self,
        tables: &Tables,
        mcu: &mut [[i16; 64]],
        membership: &[usize],
    ) -> Result<(), JpegError> {
        if self.restart_interval != 0 && self.entropy.restarts_to_go == 0 {
            self.process_restart();
        }
        let (ss, se, ah, al) = (
            i32::from(self.scan.ss),
            i32::from(self.scan.se),
            i32::from(self.scan.ah),
            i32::from(self.scan.al),
        );
        if !self.progressive {
            if !self.entropy.insufficient {
                self.mcu_sequential(tables, mcu, membership);
            }
        } else if ss == 0 {
            if ah == 0 {
                if !self.entropy.insufficient {
                    self.mcu_dc_first(tables, mcu, membership, al);
                }
            } else {
                // DC refinement does not check the latch: zero bits change nothing.
                self.mcu_dc_refine(mcu, al);
            }
        } else if let Some(t) = tables.ac.first().and_then(|t| t.as_ref())
            && !self.entropy.insufficient
            && let Some(block) = mcu.first_mut()
        {
            if ah == 0 {
                self.mcu_ac_first(t, block, ss, se, al);
            } else {
                self.mcu_ac_refine(t, block, ss, se, al);
            }
        }
        self.entropy.restarts_to_go = self.entropy.restarts_to_go.wrapping_sub(1);
        Ok(())
    }

    /// `decode_mcu` (jdhuff.c): DC difference and the AC coefficients of every block.
    fn mcu_sequential(&mut self, tables: &Tables, mcu: &mut [[i16; 64]], membership: &[usize]) {
        for (block, &k) in mcu.iter_mut().zip(membership) {
            let (Some(Some(dc)), Some(Some(ac))) = (tables.dc.get(k), tables.ac.get(k)) else {
                continue;
            };
            let mut s = self.huff_decode(dc);
            if s != 0 {
                self.check_bits(s);
                let r = self.get_bits(s);
                s = extend(r, s);
            }
            let last = &mut self.entropy.last_dc[k.min(3)];
            s = s.wrapping_add(*last);
            *last = s;
            block[0] = s as i16;
            let mut k = 1i32;
            while k < 64 {
                let rs = self.huff_decode(ac);
                let r = rs >> 4;
                let s = rs & 15;
                if s != 0 {
                    k += r;
                    self.check_bits(s);
                    let v = self.get_bits(s);
                    block[natural(k)] = extend(v, s) as i16;
                } else {
                    if r != 15 {
                        break;
                    }
                    k += 15;
                }
                k += 1;
            }
        }
    }

    fn mcu_dc_first(
        &mut self,
        tables: &Tables,
        mcu: &mut [[i16; 64]],
        membership: &[usize],
        al: i32,
    ) {
        for (block, &k) in mcu.iter_mut().zip(membership) {
            let Some(Some(dc)) = tables.dc.get(k) else {
                continue;
            };
            let mut s = self.huff_decode(dc);
            if s != 0 {
                self.check_bits(s);
                let r = self.get_bits(s);
                s = extend(r, s);
            }
            let last = &mut self.entropy.last_dc[k.min(3)];
            s = s.wrapping_add(*last);
            *last = s;
            block[0] = s.wrapping_shl(al as u32) as i16;
        }
    }

    fn mcu_dc_refine(&mut self, mcu: &mut [[i16; 64]], al: i32) {
        let p1 = 1i32 << al;
        for block in mcu.iter_mut() {
            self.check_bits(1);
            if self.get_bits(1) != 0 {
                block[0] = (i32::from(block[0]) | p1) as i16;
            }
        }
    }

    fn mcu_ac_first(&mut self, t: &Derived, block: &mut [i16; 64], ss: i32, se: i32, al: i32) {
        let mut eobrun = self.entropy.eobrun;
        if eobrun > 0 {
            eobrun -= 1;
        } else {
            let mut k = ss;
            while k <= se {
                let rs = self.huff_decode(t);
                let mut r = rs >> 4;
                let s = rs & 15;
                if s != 0 {
                    k += r;
                    self.check_bits(s);
                    r = self.get_bits(s);
                    let v = extend(r, s);
                    block[natural(k)] = v.wrapping_shl(al as u32) as i16;
                } else if r == 15 {
                    k += 15;
                } else {
                    eobrun = 1u32 << r;
                    if r != 0 {
                        self.check_bits(r);
                        eobrun = eobrun.wrapping_add(self.get_bits(r) as u32);
                    }
                    eobrun -= 1;
                    break;
                }
                k += 1;
            }
        }
        self.entropy.eobrun = eobrun;
    }

    fn mcu_ac_refine(&mut self, t: &Derived, block: &mut [i16; 64], ss: i32, se: i32, al: i32) {
        let p1 = 1i32 << al;
        let minus1 = (-1i32) << al;
        let mut eobrun = self.entropy.eobrun;
        let mut k = ss;
        // A correction bit for an already non-zero coefficient: 1 means its magnitude grows by p1 (once).
        let correct = |d: &mut Decoder<'_>, c: &mut i16| {
            d.check_bits(1);
            if d.get_bits(1) != 0 && i32::from(*c) & p1 == 0 {
                let v = i32::from(*c);
                *c = if v >= 0 { v + p1 } else { v + minus1 } as i16;
            }
        };
        if eobrun == 0 {
            while k <= se {
                let rs = self.huff_decode(t);
                let mut r = rs >> 4;
                let mut s = rs & 15;
                if s != 0 {
                    if s != 1 {
                        self.warn();
                    }
                    self.check_bits(1);
                    s = if self.get_bits(1) != 0 { p1 } else { minus1 };
                } else if r != 15 {
                    eobrun = 1u32 << r;
                    if r != 0 {
                        self.check_bits(r);
                        eobrun = eobrun.wrapping_add(self.get_bits(r) as u32);
                    }
                    break;
                }
                // Over the already non-zero coefficients (each gets a correction bit) and r still-zero ones.
                loop {
                    let pos = natural(k);
                    if block[pos] != 0 {
                        correct(self, &mut block[pos]);
                    } else {
                        r -= 1;
                        if r < 0 {
                            break;
                        }
                    }
                    k += 1;
                    if k > se {
                        break;
                    }
                }
                if s != 0 {
                    block[natural(k)] = s as i16;
                }
                k += 1;
            }
        }
        if eobrun > 0 {
            while k <= se {
                let pos = natural(k);
                if block[pos] != 0 {
                    correct(self, &mut block[pos]);
                }
                k += 1;
            }
            eobrun -= 1;
        }
        self.entropy.eobrun = eobrun;
    }
}
