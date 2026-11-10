//! Small synthetic files of the model formats, written here for the tests and the fuzz's seeds (no game file is used).

/// A growing file with little-endian writes and patches.
#[derive(Default)]
pub struct Buf(pub Vec<u8>);

impl Buf {
    pub fn at(&self) -> usize {
        self.0.len()
    }
    pub fn pad(&mut self, n: usize) -> usize {
        let at = self.at();
        self.0.resize(at + n, 0);
        at
    }
    pub fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn i16(&mut self, v: i16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn str(&mut self, s: &str) -> usize {
        let at = self.at();
        self.0.extend_from_slice(s.as_bytes());
        self.0.push(0);
        at
    }
    pub fn put_i32(&mut self, at: usize, v: i32) {
        self.0[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    pub fn put_i16(&mut self, at: usize, v: i16) {
        self.0[at..at + 2].copy_from_slice(&v.to_le_bytes());
    }
    pub fn put_f32(&mut self, at: usize, v: f32) {
        self.0[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
}

pub const CHECKSUM: i32 = 0x1234_5678;

/// A model header: one bone, one texture in one folder, one body part with one model of one mesh of 3 vertices, and one
/// looping animation of `frames` frames whose bone turns about x and moves along x (run-length streams), as one sequence.
pub fn mdl(frames: i32) -> Vec<u8> {
    let mut b = Buf::default();
    b.pad(408);
    b.0[..4].copy_from_slice(b"IDST");
    b.put_i32(4, 48);
    b.put_i32(8, CHECKSUM);
    b.0[12..16].copy_from_slice(b"tri\0");
    // The bone.
    let bone = b.pad(216);
    b.put_i32(156, 1);
    b.put_i32(160, bone as i32);
    b.put_i32(bone + 4, -1);
    b.put_f32(bone + 56, 1.0);
    for k in 0..3 {
        b.put_f32(bone + 72 + k * 4, 1.0 / 32.0);
        b.put_f32(bone + 84 + k * 4, 1.0 / 1024.0);
    }
    for k in [0, 5, 10] {
        b.put_f32(bone + 96 + k * 4, 1.0);
    }
    // The texture, its folder and the skin table.
    let tex = b.pad(64);
    b.put_i32(204, 1);
    b.put_i32(208, tex as i32);
    let cd = b.pad(4);
    b.put_i32(212, 1);
    b.put_i32(216, cd as i32);
    let skin = b.pad(4);
    b.put_i32(220, 1);
    b.put_i32(224, 1);
    b.put_i32(228, skin as i32);
    // The body part, its model and mesh.
    let part = b.pad(16);
    b.put_i32(232, 1);
    b.put_i32(236, part as i32);
    let model = b.pad(148);
    b.put_i32(part + 4, 1);
    b.put_i32(part + 8, 1);
    b.put_i32(part + 12, (model - part) as i32);
    b.0[model..model + 4].copy_from_slice(b"tri\0");
    let mesh = b.pad(116);
    b.put_i32(model + 72, 1);
    b.put_i32(model + 76, (mesh - model) as i32);
    b.put_i32(model + 80, 3);
    b.put_i32(mesh + 8, 3);
    // The animation and its data.
    let anim = b.pad(100);
    b.put_i32(180, 1);
    b.put_i32(184, anim as i32);
    b.put_f32(anim + 8, 30.0);
    b.put_i32(anim + 12, 1);
    b.put_i32(anim + 16, frames);
    let data = b.at();
    b.put_i32(anim + 56, (data - anim) as i32);
    b.0.extend_from_slice(&[0, 0x08 | 0x04]);
    b.i16(0);
    // Rotation offsets (x only), then position offsets (x only), each relative to its own three.
    let rot = b.at();
    b.i16(12);
    b.i16(0);
    b.i16(0);
    let pos = b.at();
    b.i16(0);
    b.i16(0);
    b.i16(0);
    let stream = |b: &mut Buf| {
        let n = frames.clamp(1, 255);
        b.0.extend_from_slice(&[n as u8, n as u8]);
        for f in 0..n {
            b.i16((f * 50) as i16);
        }
    };
    debug_assert_eq!(b.at(), rot + 12);
    stream(&mut b);
    let pos_stream = b.at();
    b.put_i16(pos, (pos_stream - pos) as i16);
    stream(&mut b);
    // The terminating record.
    b.0.extend_from_slice(&[255, 0, 0, 0]);
    // The sequence and its one cell.
    let seq = b.pad(212);
    b.put_i32(188, 1);
    b.put_i32(192, seq as i32);
    b.put_i32(seq + 12, 1);
    b.put_i32(seq + 68, 1);
    b.put_i32(seq + 72, 1);
    let cell = b.pad(2);
    b.put_i32(seq + 60, (cell - seq) as i32);
    // The strings.
    let s = b.str("root");
    b.put_i32(bone, (s - bone) as i32);
    let s = b.str("skin");
    b.put_i32(tex, (s - tex) as i32);
    let s = b.str("models\\synth\\");
    b.put_i32(cd, s as i32);
    let s = b.str("body");
    b.put_i32(part, (s - part) as i32);
    let s = b.str("wave");
    b.put_i32(anim + 4, (s - anim) as i32);
    b.put_i32(seq + 4, (s - seq) as i32);
    let s = b.str("ACT_IDLE");
    b.put_i32(seq + 8, (s - seq) as i32);
    let len = b.at() as i32;
    b.put_i32(76, len);
    b.0
}

/// A vertex file of 3 vertices on the one bone.
pub fn vvd() -> Vec<u8> {
    let mut b = Buf::default();
    b.0.extend_from_slice(b"IDSV");
    b.i32(4);
    b.i32(CHECKSUM);
    b.i32(1);
    b.i32(3);
    b.pad(28);
    b.i32(0);
    b.i32(64);
    b.i32(64);
    b.i32(0);
    for (x, y) in [(0.0, 0.0), (10.0, 0.0), (0.0, 10.0)] {
        b.f32(1.0);
        b.f32(0.0);
        b.f32(0.0);
        b.0.extend_from_slice(&[0, 0, 0, 1]);
        b.f32(x);
        b.f32(y);
        b.f32(0.0);
        b.f32(0.0);
        b.f32(0.0);
        b.f32(1.0);
        b.f32(x / 10.0);
        b.f32(y / 10.0);
    }
    b.0
}

/// A mesh file of one body part, model, level and mesh, with `groups` strip groups; each reads the same 3 vertices and
/// `indices` indices (when `share`, every group's offsets point at the first group's lists: overlapping offsets).
pub fn vtx(groups: usize, indices: usize, share: bool) -> Vec<u8> {
    let mut b = Buf::default();
    b.i32(7);
    b.i32(24);
    b.pad(4);
    b.i32(3);
    b.i32(CHECKSUM);
    b.i32(1);
    b.i32(0);
    b.i32(1);
    b.i32(36);
    // Body part at 36, model at 44, level at 52, mesh at 64.
    b.i32(1);
    b.i32(8);
    b.i32(1);
    b.i32(8);
    b.i32(1);
    b.i32(12);
    b.f32(0.0);
    b.i32(groups as i32);
    b.i32(9);
    b.0.push(0);
    let first_group = b.at();
    b.pad(25 * groups);
    let mut lists = Vec::new();
    for g in 0..groups {
        if share && g > 0 {
            lists.push(lists[0]);
            continue;
        }
        let verts = b.at();
        for v in 0..3u16 {
            b.0.extend_from_slice(&[0, 0, 0, 1]);
            b.0.extend_from_slice(&v.to_le_bytes());
            b.0.extend_from_slice(&[0, 0, 0]);
        }
        let idx = b.at();
        for i in 0..indices {
            b.0.extend_from_slice(&((i % 3) as u16).to_le_bytes());
        }
        lists.push((verts, idx));
    }
    for (g, (verts, idx)) in lists.into_iter().enumerate() {
        let at = first_group + g * 25;
        b.put_i32(at, 3);
        b.put_i32(at + 4, verts as i32 - at as i32);
        b.put_i32(at + 8, indices as i32);
        b.put_i32(at + 12, idx as i32 - at as i32);
    }
    b.0
}
