//! Synthetic physics files for the tests and the fuzz's seeds (no game file is used): solids built from point lists, the ledge
//! tree and ledges written as the format lays them out.

use crate::synth::Buf;

/// A point of the model's space (inches) as the file stores it (meters in the physics library's axes): the inverse of
/// [`crate::phy::to_model`].
pub fn stored(p: [f32; 3]) -> [f32; 3] {
    [p[0] * 0.0254, -p[2] * 0.0254, p[1] * 0.0254]
}

/// The eight corners of a box (model inches), as stored.
pub fn box_points(lo: [f32; 3], hi: [f32; 3]) -> Vec<[f32; 3]> {
    (0..8)
        .map(|i| {
            stored([
                if i & 1 == 0 { lo[0] } else { hi[0] },
                if i & 2 == 0 { lo[1] } else { hi[1] },
                if i & 4 == 0 { lo[2] } else { hi[2] },
            ])
        })
        .collect()
}

/// A node with the leaves of `lo..hi` under it, written in memory order (the left child next, the right child's offset
/// patched). Returns the node's place.
fn tree(buf: &mut Buf, leaves: &mut Vec<(usize, usize)>, lo: usize, hi: usize) -> usize {
    let at = buf.pad(28);
    if hi - lo == 1 {
        leaves.push((at, lo));
    } else {
        let mid = (lo + hi) / 2;
        tree(buf, leaves, lo, mid);
        let right = tree(buf, leaves, mid, hi);
        buf.put_i32(at, (right - at) as i32);
    }
    at
}

/// The compact surface of `ledges` (each a point list, as stored): 48 bytes, the tree, then the ledges.
pub fn surface(ledges: &[Vec<[f32; 3]>]) -> Vec<u8> {
    let mut b = Buf::default();
    b.pad(48);
    b.put_f32(24, 1.0);
    b.put_i32(32, 48);
    b.0[44..48].copy_from_slice(b"IVPS");
    if ledges.is_empty() {
        b.pad(28);
        return b.0;
    }
    let mut leaves = Vec::new();
    tree(&mut b, &mut leaves, 0, ledges.len());
    for (node, i) in leaves {
        let points = &ledges[i];
        let tris = points.len().div_ceil(3);
        let at = b.at();
        b.put_i32(node + 4, (at - node) as i32);
        b.i32(16 + 16 * tris as i32);
        b.i32(0);
        b.i32(0);
        b.i16(tris as i16);
        b.i16(0);
        for t in 0..tris {
            b.i32(0);
            for e in 0..3 {
                b.i32(((3 * t + e) % points.len()) as i32);
            }
        }
        for p in points {
            b.f32(p[0]);
            b.f32(p[1]);
            b.f32(p[2]);
            b.f32(0.0);
        }
    }
    b.0
}

/// A solid of type `kind` (0 convex pieces) in the modern layout.
pub fn modern(kind: u16, ledges: &[Vec<[f32; 3]>]) -> Vec<u8> {
    let surf = surface(ledges);
    let mut b = Buf::default();
    b.0.extend_from_slice(b"VPHY");
    b.i16(0x100);
    b.i16(kind as i16);
    b.i32(surf.len() as i32);
    for a in [0.5f32, 1.0, 1.5] {
        b.f32(a);
    }
    b.i32(0);
    b.0.extend_from_slice(&surf);
    b.0
}

/// A solid in the older layout (a bare compact surface).
pub fn legacy(ledges: &[Vec<[f32; 3]>]) -> Vec<u8> {
    surface(ledges)
}

/// A file of solids (each a size and data) and text.
pub fn file(solids: &[Vec<u8>], text: &str) -> Vec<u8> {
    let mut b = Buf::default();
    b.i32(16);
    b.i32(0);
    b.i32(solids.len() as i32);
    b.i32(crate::synth::CHECKSUM);
    for s in solids {
        b.i32(s.len() as i32);
        b.0.extend_from_slice(s);
    }
    b.0.extend_from_slice(text.as_bytes());
    b.0
}

/// The text of a one-solid file.
pub const TEXT: &str = "solid {\n\"index\" \"0\"\n\"name\" \"crate\"\n\"mass\" \"50.5\"\n\"surfaceprop\" \"wood\"\n\"damping\" \"0.1\"\n\"rotdamping\" \"0.2\"\n\"inertia\" \"1.5\"\n\"volume\" \"4096\"\n}\neditparams {\n\"rootname\" \"\"\n\"totalmass\" \"50.5\"\n\"concave\" \"0\"\n}\n";

/// A wedge: a box with one top edge cut, as stored.
pub fn wedge() -> Vec<[f32; 3]> {
    [
        [0.0, 0.0, 0.0],
        [20.0, 0.0, 0.0],
        [0.0, 10.0, 0.0],
        [20.0, 10.0, 0.0],
        [0.0, 0.0, 10.0],
        [20.0, 0.0, 10.0],
    ]
    .iter()
    .map(|&p| stored(p))
    .collect()
}

/// A two-piece file with text (the fuzz's seed and the tests').
pub fn two_pieces() -> Vec<u8> {
    let pieces = vec![box_points([-8.0, -8.0, 0.0], [8.0, 8.0, 16.0]), wedge()];
    file(&[modern(0, &pieces)], TEXT)
}

/// A ragdoll-like file: three solids named for bones and two joints.
pub fn ragdoll() -> Vec<u8> {
    let solids: Vec<Vec<u8>> = (0..3)
        .map(|i| {
            let o = i as f32 * 10.0;
            modern(0, &[box_points([o, 0.0, 0.0], [o + 4.0, 4.0, 4.0])])
        })
        .collect();
    let mut text = String::new();
    for (i, n) in ["pelvis", "spine", "head"].iter().enumerate() {
        text += &format!("solid {{ \"index\" \"{i}\" \"name\" \"{n}\" \"mass\" \"10\" }}\n");
    }
    text += "ragdollconstraint { \"parent\" \"0\" \"child\" \"1\" \"xmin\" \"-10\" \"xmax\" \"10\" \"xfriction\" \"1\" \"ymin\" \"-5\" \"ymax\" \"5\" \"zmin\" \"-1\" \"zmax\" \"1\" }\n";
    text +=
        "ragdollconstraint { \"parent\" \"1\" \"child\" \"2\" \"xmin\" \"-3\" \"xmax\" \"3\" }\n";
    text += "collisionrules { \"collisionpair\" \"0,2\" \"selfcollisions\" \"0\" }\n";
    file(&solids, &text)
}
