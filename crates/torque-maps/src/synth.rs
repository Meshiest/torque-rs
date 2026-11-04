//! Small synthetic files for the tests and the fuzz: a `.dif` holding one box-shaped interior in any version the reader takes
//! (with every optional part of a resource when asked), and a `.ter` of any legacy version.

use crate::png::{SIGNATURE, chunk};

/// What a synthetic `.dif` holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DifSpec {
    pub version: u32,
    /// A version-0 file in the plain form (no unused surface byte, no light direction maps).
    pub plain_surfaces: bool,
    /// A preview, a sub-object interior, a trigger, a path follower, a force field, an AI node, vehicle collision, game
    /// entities, a mirror and (from version 10) a static mesh.
    pub everything: bool,
    /// The lightmap-border block: 0 a zero flag, 1 a flag of one with a border, 2 a zero flag and one more word
    /// after it.
    pub border: u8,
}

/// The bytes written, and the hulls the box interior is written with (how many, the plane indices each names).
struct Out(Vec<u8>, (u32, u32));

impl Out {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32s(&mut self, v: &[f32]) {
        for &x in v {
            self.f32(x);
        }
    }
    fn s(&mut self, v: &str) {
        self.u8(v.len() as u8);
        self.0.extend_from_slice(v.as_bytes());
    }
    fn png(&mut self) {
        self.0.extend_from_slice(&tiny_png());
    }
}

/// A 1 by 1 PNG (its pixel data is not valid zlib: the readers do not decode).
pub fn tiny_png() -> Vec<u8> {
    let mut out = Vec::from(SIGNATURE);
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&1u32.to_be_bytes());
    ihdr.extend_from_slice(&1u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &[0x78, 0x9c, 0x63, 0, 0]);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// The eight corners of a unit box and its six faces as four-point strips (corner indices).
const CORNERS: [[f32; 3]; 8] = [
    [0.0, 0.0, 0.0],
    [1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0],
    [1.0, 1.0, 0.0],
    [0.0, 0.0, 1.0],
    [1.0, 0.0, 1.0],
    [0.0, 1.0, 1.0],
    [1.0, 1.0, 1.0],
];
const FACES: [[u32; 4]; 6] = [
    [0, 1, 2, 3],
    [4, 6, 5, 7],
    [0, 4, 1, 5],
    [2, 3, 6, 7],
    [0, 2, 4, 6],
    [1, 5, 3, 7],
];
const NORMALS: [[f32; 3]; 3] = [[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]];

fn interior(o: &mut Out, s: DifSpec) {
    let v = s.version;
    o.u32(v);
    o.u32(0); // detail level
    o.u32(250); // min pixels
    o.f32s(&[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]);
    o.f32s(&[0.5, 0.5, 0.5, 0.9]);
    o.u8(0); // alarm state
    o.u32(0); // light state entries
    // planes: three normals, six planes (z=0 flipped and z=1 etc.)
    o.u32(3);
    for n in NORMALS {
        o.f32s(&n);
    }
    o.u32(6);
    for (n, d) in [
        (0u16, 0.0f32),
        (0, -1.0),
        (1, 0.0),
        (1, -1.0),
        (2, 0.0),
        (2, -1.0),
    ] {
        o.u16(n);
        o.f32(d);
    }
    o.u32(8);
    for c in CORNERS {
        o.f32s(&c);
    }
    o.u32(8);
    o.0.extend_from_slice(&[1; 8]); // point visibility
    o.u32(1);
    o.f32s(&[1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0]); // texgen
    // BSP: one node on plane 0, front empty (zone 0), back the solid leaf 0
    o.u32(1);
    o.u16(0);
    if v >= 14 {
        o.u32(0x8_0000);
        o.u32(0xc_0000);
    } else {
        o.u16(0x8000);
        o.u16(0xc000);
    }
    o.u32(1);
    o.u32(0);
    o.u16(6);
    // material list
    o.u8(1);
    o.u32(2);
    o.s("textures/wall");
    o.s("NULL");
    // windings
    o.u32(24);
    for f in FACES {
        for c in f {
            o.u32(c);
        }
    }
    o.u32(6);
    for i in 0..6 {
        o.u32(i * 4);
        o.u32(4);
    }
    if v >= 12 {
        o.u32(1);
        for x in [0i32, 1, 0, 2] {
            o.u32(x as u32);
        }
    }
    // zones
    o.u32(1);
    o.u16(0);
    o.u16(0);
    o.u32(0);
    o.u16(6);
    if v >= 12 {
        o.u32(0);
        o.u32(0);
    }
    o.u16(0);
    o.u32(6);
    for i in 0..6u16 {
        o.u16(i);
    }
    if v >= 12 {
        o.u32(0);
    }
    o.u32(0); // zone portals
    o.u32(0); // portals
    // surfaces
    o.u32(6);
    for i in 0..6u32 {
        o.u32(i * 4);
        if v >= 13 {
            o.u32(4);
        } else {
            o.u8(4);
        }
        o.u16(i as u16 | if i == 0 { 0x8000 } else { 0 });
        o.u16(0);
        o.u32(0);
        o.u8(0);
        o.u32(0xf);
        o.u16((3 << 6) | 3); // stEnc 0, scales 1/8
        o.f32(0.0);
        o.f32(0.0);
        o.u16(0);
        o.u32(0);
        if v >= 13 {
            for x in [0, 0, 8, 8] {
                o.u32(x);
            }
        } else {
            o.0.extend_from_slice(&[0, 0, 8, 8]);
        }
        if !s.plain_surfaces && (v == 0 || v == 5 || v >= 12) {
            o.u8(0);
        }
        if v == 5 {
            o.u32(7);
        }
    }
    if v == 5 {
        o.u32(1);
        o.0.extend_from_slice(&[0; 24]);
        o.u32(1);
        o.f32s(&[0.0, 0.0, 1.0]);
        o.u32(1);
        o.u16(0);
    }
    for _ in 0..2 {
        o.u32(6);
        for _ in 0..6 {
            if v >= 13 {
                o.u32(0);
            } else {
                o.u8(0);
            }
        }
    }
    // null surfaces
    o.u32(1);
    o.u32(0);
    o.u16(1);
    o.u8(0);
    if v >= 13 {
        o.u32(4);
    } else {
        o.u8(4);
    }
    // lightmaps
    o.u32(1);
    o.png();
    if !s.plain_surfaces && (v == 0 || v == 5 || v >= 12) {
        o.png();
    }
    o.u8(1);
    // solid leaf surfaces: the six surfaces and the null one
    o.u32(6);
    for i in 0..5 {
        o.u32(i);
    }
    o.u32(0x8000_0000);
    if s.everything {
        o.u32(1);
        o.u32(0);
        o.u32(0);
        o.u16(1);
        o.u16(1);
        o.u32(500);
        o.u32(1);
        o.0.extend_from_slice(&[255, 128, 0]);
        o.u32(0);
        o.u32(0);
        o.u16(1);
        o.u32(1);
        o.u32(0);
        o.u32(0);
        o.u16(0);
    } else {
        o.u32(0);
        o.u32(0);
        o.u32(0);
    }
    o.u32(2); // state data buffer
    o.u32(0);
    o.u16(0x0101);
    o.u32(5);
    o.0.extend_from_slice(b"lamp\0");
    // sub-objects
    if s.everything {
        o.u32(1);
        o.u32(1);
        o.u32(0);
        o.u32(0);
        o.f32(0.5);
        o.u32(1);
        o.u32(0);
        o.f32s(&[0.5, 0.5, 0.5]);
    } else {
        o.u32(0);
    }
    hulls(o, v >= 12);
    for _ in 0..256 {
        o.u32(0);
        o.u32(1);
    }
    o.u32(1);
    o.u16(0);
    o.u32(0); // coord bin mode
    o.0.extend_from_slice(&[10, 20, 30, 255, 0, 0, 0, 255]);
    if v >= 10 {
        if s.everything {
            o.u32(1);
            static_mesh(o);
        } else {
            o.u32(0);
        }
    }
    if v >= 11 {
        o.u32(1);
        o.f32s(&[0.0, 0.0, 1.0]);
        o.u32(1);
        o.u32(0);
        o.u32(0);
        o.u32(0);
        o.u32(6);
        for _ in 0..6 {
            o.u32(0);
        }
    }
    if v < 10 {
        o.u32(0);
    }
    if v < 11 {
        o.u32(0);
        o.u32(0);
    }
    match s.border {
        1 => {
            o.u32(1);
            o.u32(2);
            o.u32(0);
        }
        2 => {
            o.u32(0);
            o.u32(0);
        }
        _ => o.u32(0),
    }
}

fn hulls(o: &mut Out, with_static: bool) {
    let (count, planes) = o.1;
    o.u32(count);
    for k in 0..count {
        o.u32(0);
        o.u16(8);
        o.f32s(&[0.0, 1.0, 0.0, 1.0, 0.0, 1.0]);
        o.u32(0);
        o.u16(6);
        o.u32(k * planes);
        o.u32(0);
        o.u32(0);
        o.u32(0);
        if with_static {
            o.u8(0);
        }
    }
    o.u32(4);
    o.0.extend_from_slice(&[1, 2, 3, 4]);
    o.u32(8);
    for i in 0..8 {
        o.u32(i);
    }
    o.u32(count * planes);
    for i in 0..count * planes {
        o.u16((i % 6) as u16);
    }
    o.u32(1);
    o.u32(0);
    o.u32(6);
    for i in 0..6 {
        o.u32(i);
    }
    o.u32(6);
    for i in 0..6 {
        o.u16(i);
    }
    o.u32(8);
    for i in 0..8 {
        o.u32(i);
    }
    o.u32(3);
    o.0.extend_from_slice(&[0, 1, 2]);
}

fn static_mesh(o: &mut Out) {
    o.u32(1);
    o.u8(0);
    o.u32(0);
    o.u32(0);
    o.u32(0);
    o.u32(0);
    o.u32(0);
    o.u32(3);
    o.f32s(&[0.0; 8]);
    o.0.extend_from_slice(&[0; 16]);
    o.u32(3);
    for i in 0..3 {
        o.u16(i);
    }
    for _ in 0..2 {
        o.u32(3);
        o.f32s(&[0.0; 9]);
    }
    for _ in 0..2 {
        o.u32(3);
        o.f32s(&[0.0; 6]);
    }
    o.u8(1);
    o.u8(1);
    o.u32(1);
    o.s("mesh_tex");
    for _ in 0..7 {
        o.u32(0);
    }
    o.u32(2);
    o.u8(1);
    o.png();
    o.u8(0);
    o.u8(1);
    o.u8(0);
    o.f32s(&[0.0; 6]);
    o.f32s(&[
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]);
    o.f32s(&[1.0; 3]);
}

/// A synthetic `.dif`.
pub fn dif(s: DifSpec) -> Vec<u8> {
    dif_hulls(s, 1, 6)
}

/// A synthetic `.dif` whose box interior (and vehicle collision) has `count` hulls, each the box's eight points and `planes`
/// plane indices (the box's six over and over).
pub fn dif_hulls(s: DifSpec, count: u32, planes: u32) -> Vec<u8> {
    let mut o = Out(Vec::new(), (count, planes));
    o.u32(crate::dif::RESOURCE_VERSION);
    if s.everything {
        o.u8(1);
        o.png();
    } else {
        o.u8(0);
    }
    o.u32(1);
    interior(&mut o, s);
    if s.everything {
        o.u32(1);
        interior(&mut o, s);
        // a trigger
        o.u32(1);
        o.s("trig");
        o.s("TriggerData");
        o.u32(1);
        o.s("key");
        o.s("value");
        o.u32(1);
        o.f32s(&[0.0; 3]);
        o.u32(1);
        o.f32s(&[0.0, 0.0, 1.0, 0.0]);
        o.u32(1);
        o.0.extend_from_slice(&[0; 16]);
        o.f32s(&[1.0, 2.0, 3.0]);
        // a path follower
        o.u32(1);
        o.s("door");
        o.s("PathedDefault");
        o.u32(0);
        o.f32s(&[0.0; 3]);
        o.u32(0);
        o.u32(1);
        o.u32(0);
        o.u32(1);
        o.f32s(&[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
        o.u32(1000);
        o.u32(0);
        o.u32(1000);
        // a force field
        o.u32(1);
        o.u32(0);
        o.s("field");
        o.u32(1);
        o.s("trig");
        o.f32s(&[0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.5, 0.5, 0.5, 1.0]);
        o.u32(1);
        o.f32s(&[0.0, 0.0, 1.0]);
        o.u32(1);
        o.u16(0);
        o.f32(0.0);
        o.u32(1);
        o.f32s(&[0.0; 3]);
        o.u32(1);
        o.0.extend_from_slice(&[0, 0, 0, 0x80, 0, 0xc0]);
        o.u32(1);
        o.u32(0);
        o.u16(1);
        o.u32(1);
        o.u32(0);
        o.u32(1);
        o.u32(0);
        o.u8(1);
        o.u16(0);
        o.u8(0);
        o.u32(0);
        o.u32(1);
        o.u32(0);
        o.0.extend_from_slice(&[255, 0, 0, 128]);
        // an AI node
        o.u32(1);
        o.s("node");
        o.f32s(&[1.0, 2.0, 3.0]);
        // vehicle collision
        o.u32(1);
        o.u32(s.version);
        hulls(&mut o, false);
        o.u32(1);
        o.u32(0);
        o.u16(0);
        o.u8(0);
        o.u32(4);
        o.u32(8);
        for c in CORNERS {
            o.f32s(&c);
        }
        o.u32(1);
        o.f32s(&[0.0, 0.0, 1.0, 0.0]);
        o.u32(4);
        for i in 0..4 {
            o.u32(i);
        }
        o.u32(1);
        o.u32(0);
        o.u32(4);
        // game entities
        o.u32(2);
        o.u32(1);
        o.s("StaticShapeData");
        o.s("Item");
        o.f32s(&[0.0; 3]);
        o.u32(0);
        o.u32(0);
    } else {
        for _ in 0..6 {
            o.u32(0);
        }
        o.u32(0);
    }
    o.0
}

/// A synthetic legacy `.ter` with the given slot names ("" for an unused slot; at most eight).
pub fn ter(version: u8, names: &[&str]) -> Vec<u8> {
    let mut o = Out(Vec::new(), (1, 6));
    o.u8(version);
    for i in 0..crate::ter::SAMPLES {
        o.u16((i % 256) as u16);
    }
    for i in 0..crate::ter::SAMPLES {
        o.u8((i % 3) as u8);
    }
    for i in 0..crate::ter::SLOTS {
        o.s(names.get(i).copied().unwrap_or(""));
    }
    if version >= 2 {
        for (k, n) in names.iter().enumerate() {
            if !n.is_empty() {
                for i in 0..crate::ter::SAMPLES {
                    o.u8(((i + k * 50) % 256) as u8);
                }
            }
        }
    }
    if version >= 3 {
        o.u32(3);
        o.0.extend_from_slice(b"abc");
        o.u32(0);
    }
    if version >= 5 {
        o.s("");
        o.s("edge");
    }
    o.0
}
