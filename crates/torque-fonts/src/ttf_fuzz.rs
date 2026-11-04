//! The hostile-TrueType sweep of [`crate::ttf`]: synthetic faces made here (never a game or system font), the hostile shapes the
//! checks exist for (composite bombs and cycles, cmaps that enumerate everything, a looping kern table, glyphs larger than a sheet,
//! tables outside the file), and byte-level mutants of those and of any seed the caller adds (such as real faces). Each case is
//! parsed and, when accepted, made at three sizes, under `catch_unwind`; the sweep reports the cases, the panics, the accepted and
//! the slowest case. Deterministic for a seed.

use std::time::Instant;

use crate::ttf::{self, TtfFace};

/// One synthetic glyph: contours of points (on-curve), or components (glyph, dx, dy) when `components` is not empty.
#[derive(Debug, Clone, Default)]
pub struct SynthGlyph {
    pub contours: Vec<Vec<(i16, i16)>>,
    pub components: Vec<(u16, i16, i16)>,
    pub advance: u16,
    /// Every point off the curve (quadratic control points).
    pub off_curve: bool,
}

/// The parts of a synthetic face.
#[derive(Debug, Clone)]
pub struct Synth {
    pub units_per_em: u16,
    pub ascent: u16,
    pub descent: u16,
    pub glyphs: Vec<SynthGlyph>,
    /// `(code point, glyph)`, code points below 0xFFFF, ascending.
    pub cmap: Vec<(u16, u16)>,
    /// Extra raw tables (`tag`, bytes) added to the directory.
    pub extra: Vec<([u8; 4], Vec<u8>)>,
    /// Number of glyphs `maxp` claims (None: the glyph count).
    pub claimed_glyphs: Option<u16>,
}

fn box_glyph(x0: i16, y0: i16, x1: i16, y1: i16, advance: u16) -> SynthGlyph {
    SynthGlyph {
        contours: vec![vec![(x0, y0), (x0, y1), (x1, y1), (x1, y0)]],
        components: Vec::new(),
        advance,
        off_curve: false,
    }
}

impl Synth {
    /// A small well-formed face: .notdef, space, `A` (a triangle), `B` (a box), `C` (B moved), `é` (A with B as an accent).
    pub fn basic() -> Synth {
        let tri = SynthGlyph {
            contours: vec![vec![(50, 0), (300, 700), (550, 0)]],
            components: Vec::new(),
            advance: 600,
            off_curve: false,
        };
        Synth {
            units_per_em: 1000,
            ascent: 900,
            descent: 250,
            glyphs: vec![
                box_glyph(50, 0, 450, 700, 500),
                SynthGlyph {
                    advance: 250,
                    ..SynthGlyph::default()
                },
                tri,
                box_glyph(60, 0, 520, 700, 580),
                SynthGlyph {
                    components: vec![(3, 20, 0)],
                    advance: 600,
                    ..SynthGlyph::default()
                },
                SynthGlyph {
                    components: vec![(2, 0, 0), (3, 100, 800)],
                    advance: 600,
                    ..SynthGlyph::default()
                },
            ],
            cmap: vec![(32, 1), (65, 2), (66, 3), (67, 4), (0xe9, 5)],
            extra: Vec::new(),
            claimed_glyphs: None,
        }
    }

    /// The face's file.
    pub fn build(&self) -> Vec<u8> {
        let mut glyf = Vec::new();
        let mut loca: Vec<u32> = Vec::new();
        for g in &self.glyphs {
            loca.push(glyf.len() as u32);
            if !g.components.is_empty() {
                put16(&mut glyf, 0xffff);
                for _ in 0..4 {
                    put16(&mut glyf, 0);
                }
                for (i, (child, dx, dy)) in g.components.iter().enumerate() {
                    let more = if i + 1 < g.components.len() { 0x20 } else { 0 };
                    put16(&mut glyf, 0x0001 | 0x0002 | more);
                    put16(&mut glyf, *child);
                    put16(&mut glyf, *dx as u16);
                    put16(&mut glyf, *dy as u16);
                }
            } else if !g.contours.is_empty() {
                put16(&mut glyf, g.contours.len() as u16);
                let pts: Vec<(i16, i16)> = g.contours.iter().flatten().copied().collect();
                let (mut x0, mut y0, mut x1, mut y1) = (i16::MAX, i16::MAX, i16::MIN, i16::MIN);
                for &(x, y) in &pts {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
                for v in [x0, y0, x1, y1] {
                    put16(&mut glyf, v as u16);
                }
                let mut end = 0usize;
                for c in &g.contours {
                    end += c.len();
                    put16(&mut glyf, (end - 1) as u16);
                }
                put16(&mut glyf, 0);
                glyf.extend(std::iter::repeat_n(u8::from(!g.off_curve), pts.len()));
                let mut last = 0i16;
                for &(x, _) in &pts {
                    put16(&mut glyf, x.wrapping_sub(last) as u16);
                    last = x;
                }
                last = 0;
                for &(_, y) in &pts {
                    put16(&mut glyf, y.wrapping_sub(last) as u16);
                    last = y;
                }
            }
            while glyf.len() % 4 != 0 {
                glyf.push(0);
            }
        }
        loca.push(glyf.len() as u32);
        let n = self.claimed_glyphs.unwrap_or(self.glyphs.len() as u16);
        let mut head = vec![0u8; 54];
        set32(&mut head, 0, 0x0001_0000);
        set32(&mut head, 12, 0x5f0f_3cf5);
        set16(&mut head, 18, self.units_per_em);
        set16(&mut head, 50, 1);
        let mut hhea = vec![0u8; 36];
        set32(&mut hhea, 0, 0x0001_0000);
        set16(&mut hhea, 4, self.ascent);
        set16(&mut hhea, 6, (-(self.descent as i32)) as u16);
        set16(&mut hhea, 34, self.glyphs.len() as u16);
        let mut maxp = vec![0u8; 32];
        set32(&mut maxp, 0, 0x0001_0000);
        set16(&mut maxp, 4, n);
        let mut os2 = vec![0u8; 78];
        set16(&mut os2, 74, self.ascent);
        set16(&mut os2, 76, self.descent);
        let mut hmtx = Vec::new();
        for g in &self.glyphs {
            put16(&mut hmtx, g.advance);
            put16(&mut hmtx, 0);
        }
        let mut loca_b = Vec::new();
        for o in &loca {
            put32(&mut loca_b, *o);
        }
        let cmap = cmap4(&self.cmap);
        let mut tables: Vec<([u8; 4], Vec<u8>)> = vec![
            (*b"OS/2", os2),
            (*b"cmap", cmap),
            (*b"glyf", glyf),
            (*b"head", head),
            (*b"hhea", hhea),
            (*b"hmtx", hmtx),
            (*b"loca", loca_b),
            (*b"maxp", maxp),
        ];
        tables.extend(self.extra.iter().cloned());
        tables.sort_by_key(|a| a.0);
        directory(&tables)
    }
}

/// A file from its tables (in the order given).
pub fn directory(tables: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    put32(&mut out, 0x0001_0000);
    put16(&mut out, tables.len() as u16);
    put16(&mut out, 0);
    put16(&mut out, 0);
    put16(&mut out, 0);
    let mut offset = 12 + 16 * tables.len();
    for (tag, data) in tables {
        out.extend_from_slice(tag);
        put32(&mut out, 0);
        put32(&mut out, offset as u32);
        put32(&mut out, data.len() as u32);
        offset += data.len().div_ceil(4) * 4;
    }
    for (_, data) in tables {
        out.extend_from_slice(data);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    out
}

/// A format-4 cmap with one segment a mapping and the end marker.
fn cmap4(map: &[(u16, u16)]) -> Vec<u8> {
    let seg = map.len() + 1;
    let mut sub = Vec::new();
    put16(&mut sub, 4);
    put16(&mut sub, 0);
    put16(&mut sub, 0);
    put16(&mut sub, (seg * 2) as u16);
    put16(&mut sub, 0);
    put16(&mut sub, 0);
    put16(&mut sub, 0);
    for (c, _) in map {
        put16(&mut sub, *c);
    }
    put16(&mut sub, 0xffff);
    put16(&mut sub, 0);
    for (c, _) in map {
        put16(&mut sub, *c);
    }
    put16(&mut sub, 0xffff);
    for (c, g) in map {
        put16(&mut sub, g.wrapping_sub(*c));
    }
    put16(&mut sub, 1);
    for _ in 0..seg {
        put16(&mut sub, 0);
    }
    let len = sub.len() as u16;
    set16(&mut sub, 2, len);
    let mut out = Vec::new();
    put16(&mut out, 0);
    put16(&mut out, 1);
    put16(&mut out, 3);
    put16(&mut out, 1);
    put32(&mut out, 12);
    out.extend(sub);
    out
}

fn put16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_be_bytes());
}
fn put32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_be_bytes());
}
fn set16(v: &mut [u8], at: usize, x: u16) {
    v[at..at + 2].copy_from_slice(&x.to_be_bytes());
}
fn set32(v: &mut [u8], at: usize, x: u32) {
    v[at..at + 4].copy_from_slice(&x.to_be_bytes());
}

/// The hostile shapes, each a whole file.
pub fn hostile_shapes() -> Vec<(&'static str, Vec<u8>)> {
    let mut out = Vec::new();
    // Composite bomb: glyph k is two copies of glyph k-1, 40 levels (2^40 points through ttf-parser's walk).
    let mut s = Synth::basic();
    let base = s.glyphs.len() as u16;
    for k in 0..40u16 {
        let child = if k == 0 { 3 } else { base + k - 1 };
        s.glyphs.push(SynthGlyph {
            components: vec![(child, 0, 0), (child, 10, 0)],
            advance: 500,
            ..SynthGlyph::default()
        });
    }
    s.cmap.push((0x100, base + 39));
    out.push(("composite bomb", s.build()));
    // A composite that names itself (a cycle: ttf-parser stops at depth 32, after 2^32 visits with two components).
    let mut s = Synth::basic();
    let me = s.glyphs.len() as u16;
    s.glyphs.push(SynthGlyph {
        components: vec![(me, 0, 0), (me, 5, 5)],
        advance: 500,
        ..SynthGlyph::default()
    });
    s.cmap.push((0x100, me));
    out.push(("composite cycle", s.build()));
    // A composite of 4,000 empty components (space): bounded on its own, so accepted.
    let mut s = Synth::basic();
    let fan = s.glyphs.len() as u16;
    s.glyphs.push(SynthGlyph {
        components: vec![(1, 0, 0); 4000],
        advance: 500,
        ..SynthGlyph::default()
    });
    s.cmap.push((0x100, fan));
    out.push(("empty components", s.build()));
    // Fans of fans: each level 4,000 components of the one below, the bottom one 4,000 empty or self-referencing (too deep)
    // components: ttf-parser would visit 4,000^3 components though every outline is empty.
    let mut s = Synth::basic();
    let base = s.glyphs.len() as u16;
    let mut bottom = vec![(1u16, 0i16, 0i16); 2000];
    bottom.extend(std::iter::repeat_n((base, 0, 0), 2000));
    s.glyphs.push(SynthGlyph {
        components: bottom,
        advance: 500,
        ..SynthGlyph::default()
    });
    for k in 1..3u16 {
        s.glyphs.push(SynthGlyph {
            components: vec![(base + k - 1, 0, 0); 4000],
            advance: 500,
            ..SynthGlyph::default()
        });
    }
    s.cmap.push((0x100, base + 2));
    out.push(("composite fan of empty components", s.build()));
    // A cmap segment covering every code point (and a second subtable doing it again).
    let mut s = Synth::basic();
    s.cmap = vec![(0, 2), (0xfffe, 3)];
    let mut file = s.build();
    if let Some(at) = find_tag(&file, b"cmap") {
        // Segment 0's end code (at 14 into the subtable, which sits at 12 in the table) to 0xFFFD.
        let sub = at + 12;
        set16(&mut file, sub + 14, 0xfffd);
    }
    out.push(("cmap full range", file));
    // Format 12 with 1,000 groups each covering the whole range.
    let s = Synth::basic();
    let mut c12 = Vec::new();
    put16(&mut c12, 0);
    put16(&mut c12, 1);
    put16(&mut c12, 3);
    put16(&mut c12, 10);
    put32(&mut c12, 12);
    put16(&mut c12, 12);
    put16(&mut c12, 0);
    put32(&mut c12, 16 + 12 * 1000);
    put32(&mut c12, 0);
    put32(&mut c12, 1000);
    for _ in 0..1000 {
        put32(&mut c12, 0);
        put32(&mut c12, 0x10_ffff);
        put32(&mut c12, 1);
    }
    let mut file = s.build();
    replace_table(&mut file, b"cmap", &c12);
    out.push(("cmap format 12 x1000 full ranges", file));
    // Apple kern with four billion subtables of length 0.
    let mut s = Synth::basic();
    let mut kern = Vec::new();
    put16(&mut kern, 1);
    put16(&mut kern, 0);
    put32(&mut kern, u32::MAX);
    put32(&mut kern, 0);
    put16(&mut kern, 0x0002);
    put16(&mut kern, 0);
    s.extra.push((*b"kern", kern));
    out.push(("kern loop", s.build()));
    // A glyph far larger than a sheet at a tiny em.
    let mut s = Synth::basic();
    s.units_per_em = 16;
    s.ascent = 12;
    s.descent = 4;
    s.glyphs[2] = box_glyph(-32000, -32000, 32000, 32000, 60000);
    out.push(("huge glyph", s.build()));
    // maxp claims 65,535 glyphs.
    let mut s = Synth::basic();
    s.claimed_glyphs = Some(u16::MAX);
    out.push(("maxp 65535", s.build()));
    // A table that runs past the end, an unsorted directory, a CFF table, a collection header.
    let mut file = Synth::basic().build();
    let n = file.len() as u32;
    if let Some(rec) = record_of(&file, b"glyf") {
        set32(&mut file, rec + 12, n);
    }
    out.push(("glyf past the end", file));
    let mut file = Synth::basic().build();
    let (a, b) = (12, 28);
    let ra: Vec<u8> = file[a..a + 16].to_vec();
    let rb: Vec<u8> = file[b..b + 16].to_vec();
    file[a..a + 16].copy_from_slice(&rb);
    file[b..b + 16].copy_from_slice(&ra);
    out.push(("unsorted directory", file));
    let mut s = Synth::basic();
    s.extra.push((*b"CFF ", vec![1, 0, 4, 1]));
    out.push(("cff", s.build()));
    let mut file = Synth::basic().build();
    file[0..4].copy_from_slice(b"ttcf");
    out.push(("collection", file));
    // Curves that fontdue would cut into millions of lines: a tiny em, glyphs of 4,000 control points on a huge square.
    let mut s = Synth::basic();
    s.units_per_em = 16;
    s.ascent = 12;
    s.descent = 4;
    let corners = [
        (-32000i16, -32000i16),
        (-32000, 32000),
        (32000, 32000),
        (32000, -32000),
    ];
    let pts: Vec<(i16, i16)> = (0..4000).map(|i| corners[i % 4]).collect();
    let first = s.glyphs.len() as u16;
    for k in 0..30u16 {
        s.glyphs.push(SynthGlyph {
            contours: vec![pts.clone()],
            components: Vec::new(),
            advance: 10,
            off_curve: true,
        });
        s.cmap.push((0x100 + k, first + k));
    }
    out.push(("curve bomb", s.build()));
    // kern format 3 with 65,535 glyphs (fontdue loops over their square).
    let mut s = Synth::basic();
    let mut kern = Vec::new();
    put16(&mut kern, 0);
    put16(&mut kern, 1);
    put16(&mut kern, 0);
    put16(&mut kern, 0);
    kern.push(3);
    kern.push(1);
    put16(&mut kern, 0xffff);
    kern.extend_from_slice(&[1, 1, 1, 0]);
    put16(&mut kern, 0);
    kern.extend(std::iter::repeat_n(0u8, 2 * 65535 + 1));
    s.extra.push((*b"kern", kern));
    out.push(("kern format 3", s.build()));
    // A cmap that maps past the glyphs.
    let mut s = Synth::basic();
    s.cmap.push((0x100, 900));
    out.push(("cmap past the glyphs", s.build()));
    // Empty and short files.
    out.push(("empty", Vec::new()));
    out.push(("12 bytes", vec![0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
    out
}

fn record_of(file: &[u8], tag: &[u8; 4]) -> Option<usize> {
    let n = u16::from_be_bytes([*file.get(4)?, *file.get(5)?]) as usize;
    (0..n)
        .map(|i| 12 + 16 * i)
        .find(|&r| file.get(r..r + 4) == Some(&tag[..]))
}

fn find_tag(file: &[u8], tag: &[u8; 4]) -> Option<usize> {
    let r = record_of(file, tag)?;
    Some(u32::from_be_bytes(file[r + 8..r + 12].try_into().ok()?) as usize)
}

/// Appends `data` as the table `tag`'s new body (its record pointed at the end of the file).
fn replace_table(file: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    if let Some(r) = record_of(file, tag) {
        let at = file.len() as u32;
        file.extend_from_slice(data);
        set32(file, r + 8, at);
        set32(file, r + 12, data.len() as u32);
    }
}

/// A deterministic generator (xorshift64*).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }
}

/// One mutant of `seed`.
pub fn mutate(seed: &[u8], rng: &mut Rng) -> Vec<u8> {
    let mut v = seed.to_vec();
    let edits = 1 + rng.below(8);
    for _ in 0..edits {
        if v.is_empty() {
            v.push(rng.next_u64() as u8);
            continue;
        }
        match rng.below(7) {
            0 => {
                let at = rng.below(v.len());
                v[at] ^= 1 << rng.below(8);
            }
            1 => {
                let at = rng.below(v.len());
                v[at] = rng.next_u64() as u8;
            }
            2 => {
                let at = rng.below(v.len().saturating_sub(1));
                let x: u16 =
                    [0, 1, 0x7fff, 0x8000, 0xffff, 0xfffe, rng.next_u64() as u16][rng.below(7)];
                if at + 2 <= v.len() {
                    v[at..at + 2].copy_from_slice(&x.to_be_bytes());
                }
            }
            3 => {
                let at = rng.below(v.len().saturating_sub(3));
                let x: u32 = [
                    0,
                    1,
                    0x7fff_ffff,
                    0x8000_0000,
                    u32::MAX,
                    rng.next_u64() as u32,
                ][rng.below(6)];
                if at + 4 <= v.len() {
                    v[at..at + 4].copy_from_slice(&x.to_be_bytes());
                }
            }
            4 => {
                let to = rng.below(v.len() + 1);
                v.truncate(to);
            }
            5 => {
                // A table record's offset or length (the directory's fields).
                let n = u16::from_be_bytes([
                    v.get(4).copied().unwrap_or(0),
                    v.get(5).copied().unwrap_or(0),
                ])
                .min(64) as usize;
                if n > 0 {
                    let r = 12 + 16 * rng.below(n) + if rng.below(2) == 0 { 8 } else { 12 };
                    let x = [
                        0u32,
                        1,
                        rng.next_u64() as u32 % (v.len() as u32 + 64),
                        u32::MAX,
                    ][rng.below(4)];
                    if r + 4 <= v.len() {
                        v[r..r + 4].copy_from_slice(&x.to_be_bytes());
                    }
                }
            }
            _ => {
                // A chunk copied over another place.
                let len = 1 + rng.below(64.min(v.len()));
                let from = rng.below(v.len() - len + 1);
                let to = rng.below(v.len() - len + 1);
                let chunk = v[from..from + len].to_vec();
                v[to..to + len].copy_from_slice(&chunk);
            }
        }
    }
    v
}

/// What a sweep did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sweep {
    pub cases: u64,
    pub panics: u64,
    pub accepted: u64,
    /// The slowest case (parse and make), milliseconds, and its name.
    pub slowest_ms: f64,
    pub slowest: String,
    /// The hostile shapes accepted (each should be refused or bounded: listed for the report).
    pub hostile_accepted: Vec<String>,
}

fn one(bytes: &[u8]) -> Result<bool, ()> {
    std::panic::catch_unwind(|| match TtfFace::parse(bytes) {
        Ok(f) => {
            for size in [ttf::MIN_SIZE, 13, ttf::MAX_SIZE] {
                if let Some(font) = f.make(size) {
                    // The layout over every byte (the walk's use of a font).
                    let all: Vec<u8> = (0u8..=255).collect();
                    let mut colors = crate::TextColors::new([255; 4], [0; 4]);
                    let _ = font.layout((0, 0), &all, &mut colors, None, 9);
                    let _ = font.str_width(&all);
                }
            }
            true
        }
        Err(_) => false,
    })
    .map_err(|_| ())
}

/// `cases` cases: the hostile shapes first, then mutants of them, of [`Synth::basic`] and of each of `seeds`; `heavy` of the
/// mutants (at most) come from `seeds` (a large face costs fontdue tens of milliseconds a parse).
pub fn sweep(cases: u64, seed: u64, seeds: &[Vec<u8>], heavy: u64) -> Sweep {
    let mut s = Sweep::default();
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let run = |name: String, bytes: &[u8], s: &mut Sweep| -> bool {
        let t = Instant::now();
        let r = one(bytes);
        let ms = t.elapsed().as_secs_f64() * 1e3;
        s.cases += 1;
        if ms > s.slowest_ms {
            s.slowest_ms = ms;
            s.slowest = name;
        }
        match r {
            Ok(true) => {
                s.accepted += 1;
                true
            }
            Ok(false) => false,
            Err(()) => {
                s.panics += 1;
                false
            }
        }
    };
    let shapes = hostile_shapes();
    for (name, bytes) in &shapes {
        if s.cases >= cases {
            break;
        }
        if run(format!("hostile {name}"), bytes, &mut s) {
            s.hostile_accepted.push((*name).to_string());
        }
    }
    let basic = Synth::basic().build();
    let mut light: Vec<&[u8]> = vec![&basic];
    light.extend(shapes.iter().map(|(_, b)| b.as_slice()));
    let mut rng = Rng::new(seed);
    let mut heavy_left = if seeds.is_empty() { 0 } else { heavy };
    let mut i = 0u64;
    while s.cases < cases {
        let (name, m) = if heavy_left > 0 && i % 10 == 9 {
            heavy_left -= 1;
            let k = rng.below(seeds.len());
            (format!("seed {k} mutant {i}"), mutate(&seeds[k], &mut rng))
        } else {
            let k = rng.below(light.len());
            (
                format!("synthetic {k} mutant {i}"),
                mutate(light[k], &mut rng),
            )
        };
        run(name, &m, &mut s);
        i += 1;
    }
    std::panic::set_hook(prev);
    s
}
