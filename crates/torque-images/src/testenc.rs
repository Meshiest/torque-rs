//! A small JPEG encoder for the tests: it writes chosen quantised coefficients (no forward DCT) as baseline (single- or multi-scan,
//! with restart intervals) or progressive files (any scan script: spectral selection, successive approximation, EOB runs), by the
//! rules of libjpeg's encoder. Test-only.

use crate::Bitmap;
use crate::jpeg::{Info, JpegError, Limits, NATURAL_ORDER, decode_with};

// A test encoder

pub(crate) struct BitSink {
    pub(crate) out: Vec<u8>,
    pub(crate) acc: u32,
    pub(crate) n: u32,
}

impl BitSink {
    pub(crate) fn new() -> BitSink {
        BitSink {
            out: Vec::new(),
            acc: 0,
            n: 0,
        }
    }

    pub(crate) fn bits(&mut self, v: u32, n: u32) {
        for i in (0..n).rev() {
            self.acc = (self.acc << 1) | ((v >> i) & 1);
            self.n += 1;
            if self.n == 8 {
                let b = self.acc as u8;
                self.out.push(b);
                if b == 0xff {
                    self.out.push(0);
                }
                self.acc = 0;
                self.n = 0;
            }
        }
    }

    /// Pads the last byte with ones.
    pub(crate) fn flush(&mut self) {
        if self.n > 0 {
            let pad = 8 - self.n;
            self.bits((1 << pad) - 1, pad);
        }
    }
}

/// A Huffman code: (code, length) per symbol.
#[derive(Clone)]
pub(crate) struct Code {
    pub(crate) bits: [u8; 17],
    pub(crate) vals: Vec<u8>,
    pub(crate) map: Vec<Option<(u32, u32)>>,
}

impl Code {
    /// Every symbol of `symbols` at length `len`.
    pub(crate) fn flat(symbols: &[u8], len: usize) -> Code {
        let mut bits = [0u8; 17];
        bits[len] = symbols.len() as u8;
        let mut map = vec![None; 256];
        for (i, &s) in symbols.iter().enumerate() {
            map[usize::from(s)] = Some((i as u32, len as u32));
        }
        Code {
            bits,
            vals: symbols.to_vec(),
            map,
        }
    }

    pub(crate) fn dc() -> Code {
        Code::flat(&(0..=11).collect::<Vec<u8>>(), 4)
    }

    pub(crate) fn ac() -> Code {
        let mut s = Vec::new();
        for r in 0..16u8 {
            s.push(r << 4);
            for size in 1..=10u8 {
                s.push((r << 4) | size);
            }
        }
        Code::flat(&s, 8)
    }

    pub(crate) fn put(&self, sink: &mut BitSink, sym: u8) {
        let (c, l) = self.map[usize::from(sym)].expect("symbol in table");
        sink.bits(c, l);
    }
}

pub(crate) fn nbits(v: u32) -> u32 {
    32 - v.leading_zeros()
}

/// One component's coefficients on its padded block grid.
#[derive(Clone)]
pub(crate) struct EncComp {
    pub(crate) id: u8,
    pub(crate) h: u8,
    pub(crate) v: u8,
    pub(crate) tq: u8,
    pub(crate) bw: usize,
    pub(crate) wib: usize,
    pub(crate) hib: usize,
    pub(crate) blocks: Vec<[i16; 64]>,
}

#[derive(Clone)]
pub(crate) struct Enc {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) comps: Vec<EncComp>,
    pub(crate) quant: Vec<[u16; 64]>,
    pub(crate) restart: u16,
    pub(crate) jfif: bool,
}

/// A progressive scan: components (indices), Ss, Se, Ah, Al.
pub(crate) type ScanSpec = (Vec<usize>, u8, u8, u8, u8);

pub(crate) struct Rng(pub(crate) u64);

impl Rng {
    pub(crate) fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub(crate) fn range(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

impl Enc {
    /// An image with random coefficients (larger at low frequencies), components `(h, v)`.
    pub(crate) fn random(width: usize, height: usize, samp: &[(u8, u8)], seed: u64) -> Enc {
        let mut rng = Rng(seed | 1);
        let max_h = samp.iter().map(|s| s.0).max().unwrap_or(1) as usize;
        let max_v = samp.iter().map(|s| s.1).max().unwrap_or(1) as usize;
        let mut comps = Vec::new();
        for (i, &(h, v)) in samp.iter().enumerate() {
            let wib = (width * h as usize).div_ceil(max_h * 8);
            let hib = (height * v as usize).div_ceil(max_v * 8);
            let bw = wib.div_ceil(h as usize) * h as usize;
            let bh = hib.div_ceil(v as usize) * v as usize;
            let mut blocks = vec![[0i16; 64]; bw * bh];
            for b in &mut blocks {
                b[0] = rng.range(120) as i16 - 60;
                for (k, &zz) in NATURAL_ORDER.iter().enumerate().take(64).skip(1) {
                    let spread = if k < 6 {
                        24
                    } else if k < 20 {
                        8
                    } else {
                        3
                    };
                    if rng.range(3) == 0 {
                        b[zz] = rng.range(spread * 2 + 1) as i16 - spread as i16;
                    }
                }
            }
            comps.push(EncComp {
                id: i as u8 + 1,
                h,
                v,
                tq: (i.min(1)) as u8,
                bw,
                wib,
                hib,
                blocks,
            });
        }
        let mut q0 = [0u16; 64];
        let mut q1 = [0u16; 64];
        for i in 0..64 {
            q0[i] = 2 + (i as u16 % 7) * 3;
            q1[i] = 3 + (i as u16 % 5) * 4;
        }
        Enc {
            width,
            height,
            comps,
            quant: vec![q0, q1],
            restart: 0,
            jfif: true,
        }
    }

    pub(crate) fn header(&self, out: &mut Vec<u8>, sof: u8) {
        out.extend_from_slice(&[0xff, 0xd8]);
        if self.jfif {
            out.extend_from_slice(&[0xff, 0xe0, 0, 16]);
            out.extend_from_slice(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
        }
        for (i, q) in self.quant.iter().enumerate() {
            let sixteen = q.iter().any(|&v| v > 255);
            out.extend_from_slice(&[0xff, 0xdb]);
            let len = 2 + 1 + if sixteen { 128 } else { 64 };
            out.extend_from_slice(&(len as u16).to_be_bytes());
            out.push(((sixteen as u8) << 4) | i as u8);
            for k in 0..64 {
                let v = q[NATURAL_ORDER[k]];
                if sixteen {
                    out.extend_from_slice(&v.to_be_bytes());
                } else {
                    out.push(v as u8);
                }
            }
        }
        out.extend_from_slice(&[0xff, sof]);
        let n = self.comps.len();
        out.extend_from_slice(&((8 + n * 3) as u16).to_be_bytes());
        out.push(8);
        out.extend_from_slice(&(self.height as u16).to_be_bytes());
        out.extend_from_slice(&(self.width as u16).to_be_bytes());
        out.push(n as u8);
        for c in &self.comps {
            out.extend_from_slice(&[c.id, (c.h << 4) | c.v, c.tq]);
        }
        for (class, code) in [(0u8, Code::dc()), (0x10, Code::ac())] {
            out.extend_from_slice(&[0xff, 0xc4]);
            out.extend_from_slice(&((2 + 1 + 16 + code.vals.len()) as u16).to_be_bytes());
            out.push(class);
            out.extend_from_slice(&code.bits[1..]);
            out.extend_from_slice(&code.vals);
        }
        if self.restart != 0 {
            out.extend_from_slice(&[0xff, 0xdd, 0, 4]);
            out.extend_from_slice(&self.restart.to_be_bytes());
        }
    }

    pub(crate) fn sos(&self, out: &mut Vec<u8>, comps: &[usize], ss: u8, se: u8, ah: u8, al: u8) {
        out.extend_from_slice(&[0xff, 0xda]);
        out.extend_from_slice(&((6 + comps.len() * 2) as u16).to_be_bytes());
        out.push(comps.len() as u8);
        for &c in comps {
            out.extend_from_slice(&[self.comps[c].id, 0x00]);
        }
        out.extend_from_slice(&[ss, se, (ah << 4) | al]);
    }

    /// The blocks of each MCU of a scan over `comps`, as (component, block index).
    pub(crate) fn mcus(&self, comps: &[usize]) -> Vec<Vec<(usize, usize)>> {
        let mut out = Vec::new();
        if comps.len() == 1 {
            let c = &self.comps[comps[0]];
            for r in 0..c.hib {
                for col in 0..c.wib {
                    out.push(vec![(comps[0], r * c.bw + col)]);
                }
            }
        } else {
            let max_h = self.comps.iter().map(|c| c.h).max().unwrap_or(1) as usize;
            let max_v = self.comps.iter().map(|c| c.v).max().unwrap_or(1) as usize;
            for mr in 0..self.height.div_ceil(max_v * 8) {
                for mc in 0..self.width.div_ceil(max_h * 8) {
                    let mut m = Vec::new();
                    for &ci in comps {
                        let c = &self.comps[ci];
                        for yi in 0..c.v as usize {
                            for xi in 0..c.h as usize {
                                m.push((
                                    ci,
                                    (mr * c.v as usize + yi) * c.bw + mc * c.h as usize + xi,
                                ));
                            }
                        }
                    }
                    out.push(m);
                }
            }
        }
        out
    }

    pub(crate) fn restart_marker(&self, sink: &mut BitSink, out: &mut Vec<u8>, n: &mut u8) {
        sink.flush();
        out.append(&mut sink.out);
        out.extend_from_slice(&[0xff, 0xd0 + *n]);
        *n = (*n + 1) & 7;
    }

    /// A sequential file with one scan per group of components.
    pub(crate) fn baseline(&self, groups: &[Vec<usize>]) -> Vec<u8> {
        let mut out = Vec::new();
        self.header(&mut out, 0xc0);
        let (dc, ac) = (Code::dc(), Code::ac());
        for g in groups {
            self.sos(&mut out, g, 0, 63, 0, 0);
            let mut sink = BitSink::new();
            let mut pred = [0i32; 4];
            let mut rst = 0u8;
            for (i, mcu) in self.mcus(g).iter().enumerate() {
                if self.restart != 0 && i > 0 && i % self.restart as usize == 0 {
                    self.restart_marker(&mut sink, &mut out, &mut rst);
                    pred = [0; 4];
                }
                for &(ci, b) in mcu {
                    let k = g.iter().position(|&x| x == ci).unwrap_or(0);
                    let block = &self.comps[ci].blocks[b];
                    let diff = i32::from(block[0]) - pred[k];
                    pred[k] = i32::from(block[0]);
                    put_value(&mut sink, &dc, 0, diff);
                    let mut run = 0u8;
                    for zz in 1..64 {
                        let v = i32::from(block[NATURAL_ORDER[zz]]);
                        if v == 0 {
                            run += 1;
                            continue;
                        }
                        while run > 15 {
                            ac.put(&mut sink, 0xf0);
                            run -= 16;
                        }
                        put_value(&mut sink, &ac, run << 4, v);
                        run = 0;
                    }
                    if run > 0 {
                        ac.put(&mut sink, 0x00);
                    }
                }
            }
            sink.flush();
            out.append(&mut sink.out);
        }
        out.extend_from_slice(&[0xff, 0xd9]);
        out
    }

    /// A progressive file with the given scans (libjpeg's encoder rules for each kind).
    pub(crate) fn progressive(&self, script: &[ScanSpec]) -> Vec<u8> {
        let mut out = Vec::new();
        self.header(&mut out, 0xc2);
        let (dc, ac) = (Code::dc(), Code::ac());
        for (comps, ss, se, ah, al) in script {
            let (ss, se, ah, al) = (*ss, *se, *ah, *al);
            self.sos(&mut out, comps, ss, se, ah, al);
            let mut st = Prog {
                sink: BitSink::new(),
                eobrun: 0,
                be: Vec::new(),
            };
            let mut pred = [0i32; 4];
            let mut rst = 0u8;
            for (i, mcu) in self.mcus(comps).iter().enumerate() {
                if self.restart != 0 && i > 0 && i % self.restart as usize == 0 {
                    st.emit_eobrun(&ac);
                    self.restart_marker(&mut st.sink, &mut out, &mut rst);
                    pred = [0; 4];
                }
                for &(ci, b) in mcu {
                    let k = comps.iter().position(|&x| x == ci).unwrap_or(0);
                    let block = &self.comps[ci].blocks[b];
                    if ss == 0 {
                        if ah == 0 {
                            let v = i32::from(block[0]) >> al;
                            let diff = v - pred[k];
                            pred[k] = v;
                            put_value(&mut st.sink, &dc, 0, diff);
                        } else {
                            st.sink.bits(((i32::from(block[0]) >> al) & 1) as u32, 1);
                        }
                    } else if ah == 0 {
                        st.ac_first(&ac, block, ss, se, al);
                    } else {
                        st.ac_refine(&ac, block, ss, se, al);
                    }
                }
            }
            st.emit_eobrun(&ac);
            st.sink.flush();
            out.append(&mut st.sink.out);
        }
        out.extend_from_slice(&[0xff, 0xd9]);
        out
    }
}

/// A Huffman symbol `run | size` and the value's bits (negative values as one's complement).
pub(crate) fn put_value(sink: &mut BitSink, code: &Code, run: u8, v: i32) {
    let mag = v.unsigned_abs();
    let size = nbits(mag);
    code.put(sink, run | size as u8);
    if size > 0 {
        let bits = if v < 0 { (v - 1) as u32 } else { v as u32 };
        sink.bits(bits & ((1 << size) - 1), size);
    }
}

pub(crate) struct Prog {
    pub(crate) sink: BitSink,
    pub(crate) eobrun: u32,
    /// Correction bits buffered behind the pending EOB run.
    pub(crate) be: Vec<u32>,
}

impl Prog {
    pub(crate) fn emit_eobrun(&mut self, ac: &Code) {
        if self.eobrun > 0 {
            let r = nbits(self.eobrun) - 1;
            ac.put(&mut self.sink, (r as u8) << 4);
            if r > 0 {
                self.sink.bits(self.eobrun - (1 << r), r);
            }
            self.eobrun = 0;
            for b in std::mem::take(&mut self.be) {
                self.sink.bits(b, 1);
            }
        }
    }

    pub(crate) fn ac_first(&mut self, ac: &Code, block: &[i16; 64], ss: u8, se: u8, al: u8) {
        let mut r = 0u8;
        for k in ss..=se {
            let v = i32::from(block[NATURAL_ORDER[usize::from(k)]]);
            let mag = v.unsigned_abs() >> al;
            if mag == 0 {
                r += 1;
                continue;
            }
            self.emit_eobrun(ac);
            while r > 15 {
                ac.put(&mut self.sink, 0xf0);
                r -= 16;
            }
            let size = nbits(mag);
            ac.put(&mut self.sink, (r << 4) | size as u8);
            let bits = if v < 0 { !mag } else { mag };
            self.sink.bits(bits & ((1 << size) - 1), size);
            r = 0;
        }
        if r > 0 {
            self.eobrun += 1;
            if self.eobrun == 0x7fff {
                self.emit_eobrun(ac);
            }
        }
    }

    pub(crate) fn ac_refine(&mut self, ac: &Code, block: &[i16; 64], ss: u8, se: u8, al: u8) {
        let abs: Vec<u32> = (0..64)
            .map(|k| {
                if (ss as usize..=se as usize).contains(&k) {
                    u32::from(block[NATURAL_ORDER[k]].unsigned_abs()) >> al
                } else {
                    0
                }
            })
            .collect();
        let eob = (ss as usize..=se as usize).filter(|&k| abs[k] == 1).max();
        let mut r = 0u32;
        let mut br: Vec<u32> = Vec::new();
        for k in ss as usize..=se as usize {
            let t = abs[k];
            if t == 0 {
                r += 1;
                continue;
            }
            while r > 15 && eob.is_some_and(|e| k <= e) {
                self.emit_eobrun(ac);
                ac.put(&mut self.sink, 0xf0);
                r -= 16;
                for b in br.drain(..) {
                    self.sink.bits(b, 1);
                }
            }
            if t > 1 {
                br.push(t & 1);
                continue;
            }
            self.emit_eobrun(ac);
            ac.put(&mut self.sink, ((r as u8) << 4) | 1);
            self.sink.bits(u32::from(block[NATURAL_ORDER[k]] >= 0), 1);
            for b in br.drain(..) {
                self.sink.bits(b, 1);
            }
            r = 0;
        }
        if r > 0 || !br.is_empty() {
            self.eobrun += 1;
            self.be.append(&mut br);
            if self.eobrun == 0x7fff || self.be.len() > 937 {
                self.emit_eobrun(ac);
            }
        }
    }
}

/// A full progressive script: DC first at Al 1, then refined; luma/chroma AC bands 1-5 and 6-63 first at Al 2, refined twice.
pub(crate) fn full_script(n: usize) -> Vec<ScanSpec> {
    let all: Vec<usize> = (0..n).collect();
    let mut s = vec![(all.clone(), 0, 0, 0, 1)];
    for c in 0..n {
        s.push((vec![c], 1, 5, 0, 2));
        s.push((vec![c], 6, 63, 0, 2));
    }
    s.push((all, 0, 0, 1, 0));
    for c in 0..n {
        for (ah, al) in [(2, 1), (1, 0)] {
            s.push((vec![c], 1, 5, ah, al));
            s.push((vec![c], 6, 63, ah, al));
        }
    }
    s
}

/// The same bands, never refined below bit 1: the decoder smooths (the first five ACs are inexact).
pub(crate) fn coarse_script(n: usize) -> Vec<ScanSpec> {
    let all: Vec<usize> = (0..n).collect();
    let mut s = vec![(all, 0, 0, 0, 0)];
    for c in 0..n {
        s.push((vec![c], 1, 63, 0, 1));
    }
    s
}

/// The coefficients after the point transforms of a coarse script (what a decoder can recover), for comparing against a
/// baseline file: AC magnitudes truncated to multiples of 2.
pub(crate) fn truncated_to(enc: &Enc, al: u32) -> Enc {
    let mut e = enc.clone();
    for c in &mut e.comps {
        for b in &mut c.blocks {
            for c in b.iter_mut().skip(1) {
                let v = i32::from(*c);
                let m = (v.unsigned_abs() >> al) << al;
                *c = if v < 0 { -(m as i32) } else { m as i32 } as i16;
            }
        }
    }
    e
}

pub(crate) fn fnv(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

pub(crate) fn dec(bytes: &[u8]) -> Result<(Bitmap, Info), JpegError> {
    decode_with(bytes, &Limits::default())
}
