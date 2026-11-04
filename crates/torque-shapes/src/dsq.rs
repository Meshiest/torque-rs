//! The reader of a `.dsq` (version 24): [`parse`] reads a sequence file the way `TSShape::importSequences` does
//! (`TSShapeConstructor::preload` reads these from the `sequence<n>` fields) into a [`Dsq`],
//! [`crate::write::write_dsq`] gives the bytes back, and [`Shape::import_sequences`] applies one to a shape.
//!
//! ```text
//! u32  version (the whole word; 24)
//! i32  node count, then per node a name: i32 length, length bytes
//! i32  ignored count, i32 object count of the shape the file was made from
//! i32 n, n x (4 x i16)                       node rotations
//! i32 n, n x (3 x f32)                       node translations
//! i32 n, n x f32                             uniform scales
//! i32 n, n x (3 x f32)                       aligned scales
//! i32 n, n x (4 x i16), n x (3 x f32)        arbitrary scale rotations, then the factors (one count for both)
//! i32 n, n x (3 x f32), n x (4 x i16)        ground translations, then ground rotations (one count for both)
//! i32 n                                      object states (the count only; no state is read)
//! i32 n, n x ( name, sequence )              sequences (a name as above, then 14 words and the eight sets)
//! i32 n, n x ( u32 state, f32 position )     triggers
//! ```
//!
//! A name longer than 255 bytes is read for its first 255 and the rest skipped; the model keeps the whole name (up to
//! [`MAX_DSQ_NAME_BYTES`]) so that the writer gives the file back, and [`stored_name`] is the part Torque keeps. The file must
//! end with the triggers; Torque does not check, the model keeps whatever follows.
//!
//! **Importing** (`importSequences` over a loaded shape) is atomic here: [`Shape::import_sequences`] checks every cap and every
//! node before it changes anything, so a refused import leaves the shape as it was. A kept shape is shared by every
//! `TSShapeConstructor` that names it, so the caps count the shape's total (`MAX_SEQUENCES`, `MAX_KEYFRAMES`, `MAX_SHAPE_NAMES`)
//! and each import is charged per sequence, per node name and per keyframe.

use std::collections::HashMap;

use crate::dts::{MAX_KEYFRAMES, Reader, f32x3, quats, read_sequence, rows};
use crate::shape::{
    IntSet, Quat16, SEQ_ALIGNED_SCALE, SEQ_ARBITRARY_SCALE, SEQ_UNIFORM_SCALE, SET_BITS, Sequence,
    Shape, Trigger, check_sequence,
};
use crate::{
    Charge, MAX_DSQ_BYTES, MAX_SEQUENCES, MAX_SHAPE_NAMES, MAX_SHAPE_NODES, ShapeError, pay,
    reserved, within,
};

/// The longest name this crate reads from a `.dsq` (Torque reads 255 of its bytes and skips the rest).
pub const MAX_DSQ_NAME_BYTES: usize = 1 << 16;

/// A sequence file: what `importSequences` reads, as data.
#[derive(Debug, Clone, PartialEq)]
pub struct Dsq {
    /// The whole version word (compared as signed).
    pub version: u32,
    pub node_names: Vec<Vec<u8>>,
    /// The two words after the node names: a count Torque ignores and the object count of the shape the file was
    /// made from.
    pub spare: [i32; 2],
    pub node_rotations: Vec<Quat16>,
    pub node_translations: Vec<[f32; 3]>,
    pub uniform_scales: Vec<f32>,
    pub aligned_scales: Vec<[f32; 3]>,
    pub arbitrary_scale_rots: Vec<Quat16>,
    pub arbitrary_scale_factors: Vec<[f32; 3]>,
    pub ground_translations: Vec<[f32; 3]>,
    pub ground_rotations: Vec<Quat16>,
    /// The object state count (no state follows it).
    pub object_states: u32,
    pub sequences: Vec<DsqSequence>,
    pub triggers: Vec<Trigger>,
    /// Bytes after the triggers (none in a well-formed file).
    pub trailing: Vec<u8>,
}

/// A sequence of a `.dsq`: its name as the file writes it (the name index is not in the file) and the sequence.
#[derive(Debug, Clone, PartialEq)]
pub struct DsqSequence {
    pub name: Vec<u8>,
    pub sequence: Sequence,
}

/// The name Torque keeps of a name in a sequence file (`TSShape::readName`): the first 255 bytes, cut at the first NUL.
/// `None` for a name of length 0 (no name at all, index -1).
pub fn stored_name(name: &[u8]) -> Option<&[u8]> {
    if name.is_empty() {
        return None;
    }
    let cut = &name[..name.len().min(255)];
    Some(cut.split(|&b| b == 0).next().unwrap_or(cut))
}

/// What [`scan`] found.
#[derive(Debug, Clone, PartialEq)]
pub struct DsqScan {
    pub version: u32,
    pub node_names: Vec<Vec<u8>>,
    pub node_rotations: usize,
    pub node_translations: usize,
    pub uniform_scales: usize,
    pub aligned_scales: usize,
    pub arbitrary_scales: usize,
    pub ground_frames: usize,
    pub object_states: usize,
    pub sequences: Vec<(Vec<u8>, Sequence)>,
    pub triggers: usize,
    /// Bytes after the triggers (0 for a well-formed file).
    pub trailing: usize,
}

fn name(r: &mut Reader) -> Result<Vec<u8>, ShapeError> {
    let len = within("name length", r.u32("a name")?.into(), MAX_DSQ_NAME_BYTES)?;
    Ok(r.take(len as u64, 1, "a name")?.to_vec())
}

fn counted<'a>(
    r: &'a mut Reader,
    what: &'static str,
    element: u64,
) -> Result<(usize, &'a [u8]), ShapeError> {
    let n = within(what, r.u32(what)?.into(), MAX_KEYFRAMES)?;
    let bytes = r.take(n as u64, element, what)?;
    Ok((n, bytes))
}

/// Reads a version-24 `.dsq`.
pub fn parse(bytes: &[u8], charge: Charge) -> Result<Dsq, ShapeError> {
    if bytes.len() > MAX_DSQ_BYTES {
        return Err(ShapeError::TooLarge {
            bytes: bytes.len(),
            cap: MAX_DSQ_BYTES,
        });
    }
    pay(charge, bytes.len() as u64)?;
    let mut r = Reader {
        bytes,
        at: 0,
        buffer: "sequence file",
    };
    let version = r.u32("the version")?;
    if version > 24 {
        return Err(ShapeError::NewerVersion(version.min(255) as u8));
    }
    if version != 24 {
        return Err(ShapeError::UnsupportedVersion(version as u8));
    }
    let nodes = within(
        "node count",
        r.u32("the node count")?.into(),
        MAX_SHAPE_NODES,
    )?;
    let mut node_names = reserved(nodes, bytes.len() - r.at, 4);
    for _ in 0..nodes {
        node_names.push(name(&mut r)?);
    }
    let spare = [r.i32("the ignored count")?, r.i32("the object count")?];
    let node_rotations = quats(counted(&mut r, "node rotations", 8)?.1);
    let node_translations = f32x3(counted(&mut r, "node translations", 12)?.1);
    let uniform_scales = rows(counted(&mut r, "uniform scales", 4)?.1, 4, |c| {
        f32::from_le_bytes([c[0], c[1], c[2], c[3]])
    });
    let aligned_scales = f32x3(counted(&mut r, "aligned scales", 12)?.1);
    // One count for the rotations and the factors: the bytes of both, rotations first.
    let (n, both) = counted(&mut r, "arbitrary scales", 8 + 12)?;
    let (rot_bytes, factor_bytes) = both.split_at(n * 8);
    let arbitrary_scale_rots = quats(rot_bytes);
    let arbitrary_scale_factors = f32x3(factor_bytes);
    // Ground translations first, then the rotations.
    let (n, both) = counted(&mut r, "ground frames", 12 + 8)?;
    let (translation_bytes, rotation_bytes) = both.split_at(n * 12);
    let ground_translations = f32x3(translation_bytes);
    let ground_rotations = quats(rotation_bytes);
    let object_states = r.u32("object states")?;
    within("object states", u64::from(object_states), MAX_KEYFRAMES)?;
    let count = within(
        "sequence count",
        r.u32("the sequence count")?.into(),
        MAX_SEQUENCES,
    )?;
    pay(charge, 8 + 4 * count as u64)?;
    let mut sequences = reserved(
        count,
        bytes.len() - r.at,
        crate::dts::SEQUENCE_MIN_BYTES - 4,
    );
    for _ in 0..count {
        let n = name(&mut r)?;
        sequences.push(DsqSequence {
            name: n,
            sequence: read_sequence(&mut r, false)?,
        });
    }
    let triggers = rows(counted(&mut r, "triggers", 8)?.1, 8, |c| Trigger {
        state: u32::from_le_bytes([c[0], c[1], c[2], c[3]]),
        pos: f32::from_le_bytes([c[4], c[5], c[6], c[7]]),
    });
    Ok(Dsq {
        version,
        node_names,
        spare,
        node_rotations,
        node_translations,
        uniform_scales,
        aligned_scales,
        arbitrary_scale_rots,
        arbitrary_scale_factors,
        ground_translations,
        ground_rotations,
        object_states,
        sequences,
        triggers,
        trailing: bytes[r.at..].to_vec(),
    })
}

/// Walks a version-24 `.dsq` and reports what it saw: [`parse`] reduced to counts.
pub fn scan(bytes: &[u8], charge: Charge) -> Result<DsqScan, ShapeError> {
    let d = parse(bytes, charge)?;
    Ok(DsqScan {
        version: d.version,
        node_names: d
            .node_names
            .iter()
            .map(|n| n[..n.len().min(255)].to_vec())
            .collect(),
        node_rotations: d.node_rotations.len(),
        node_translations: d.node_translations.len(),
        uniform_scales: d.uniform_scales.len(),
        aligned_scales: d.aligned_scales.len(),
        arbitrary_scales: d.arbitrary_scale_factors.len(),
        ground_frames: d.ground_translations.len(),
        object_states: d.object_states as usize,
        sequences: d
            .sequences
            .into_iter()
            .map(|q| {
                let n = q.name[..q.name.len().min(255)].to_vec();
                (n, q.sequence)
            })
            .collect(),
        triggers: d.triggers.len(),
        trailing: d.trailing.len(),
    })
}

/// A total of one array of the shape after an import, refused past [`MAX_KEYFRAMES`].
fn grown(have: usize, add: usize, what: &'static str) -> Result<(), ShapeError> {
    let total = have.saturating_add(add);
    within(what, total as u64, MAX_KEYFRAMES).map(|_| ())
}

impl Shape {
    /// `TSShape::importSequences` over this shape (version 24): the file's nodes are matched to the shape's by name, its
    /// keyframes are appended to the shape's arrays, its sequences added with their base indices moved to the new keyframes and
    /// their rotation, translation and scale sets remapped to the shape's nodes, and its triggers appended.
    ///
    /// All or nothing: every cap and every node is checked first, so an `Err` leaves the shape unchanged. The caps are on the
    /// shape's totals (it may have been imported into before). Torque's error lines (`Sequence import failed: ...`) are the
    /// [`ShapeError::Bad`] texts of this crate.
    pub fn import_sequences(&mut self, dsq: &Dsq, charge: Charge) -> Result<(), ShapeError> {
        if dsq.version != 24 {
            return Err(ShapeError::UnsupportedVersion(dsq.version as u8));
        }
        let add_keys = dsq.node_rotations.len()
            + dsq.node_translations.len()
            + dsq.uniform_scales.len()
            + dsq.aligned_scales.len()
            + dsq.arbitrary_scale_factors.len()
            + dsq.ground_translations.len()
            + dsq.triggers.len();
        // The names the file asks for are folded to lower case once and looked up in a table; the shape's names are folded once
        // each to fill it, and its nodes are listed once by name. Charged before the work, by the bytes folded: the file's names
        // (the 255 bytes Torque keeps of each), the shape's names (when the file asks for any) and the nodes listed.
        let asked = |n: &[u8]| stored_name(n).map_or(0, |e| e.len() as u64 + 1);
        let asked_bytes: u64 = dsq.node_names.iter().map(|n| asked(n)).sum::<u64>()
            + dsq.sequences.iter().map(|q| asked(&q.name)).sum::<u64>();
        let shape_bytes: u64 = if asked_bytes > 0 {
            self.names.iter().map(|n| n.len() as u64 + 1).sum()
        } else {
            0
        };
        let nodes_listed = if dsq.node_names.is_empty() {
            0
        } else {
            self.nodes.len() as u64
        };
        pay(
            charge,
            8 + dsq.sequences.len() as u64 * 64
                + add_keys as u64
                + asked_bytes
                + shape_bytes
                + nodes_listed,
        )?;
        within(
            "sequences (with the imports)",
            (self.sequences.len() + dsq.sequences.len()) as u64,
            MAX_SEQUENCES,
        )?;
        grown(
            self.node_rotations.len(),
            dsq.node_rotations.len(),
            "node rotations (with the imports)",
        )?;
        grown(
            self.node_translations.len(),
            dsq.node_translations.len(),
            "node translations (with the imports)",
        )?;
        grown(
            self.node_uniform_scales.len(),
            dsq.uniform_scales.len(),
            "uniform scales (with the imports)",
        )?;
        grown(
            self.node_aligned_scales.len(),
            dsq.aligned_scales.len(),
            "aligned scales (with the imports)",
        )?;
        grown(
            self.node_arbitrary_scale_factors.len(),
            dsq.arbitrary_scale_factors.len(),
            "arbitrary scales (with the imports)",
        )?;
        grown(
            self.ground_translations.len(),
            dsq.ground_translations.len(),
            "ground frames (with the imports)",
        )?;
        grown(
            self.triggers.len(),
            dsq.triggers.len(),
            "triggers (with the imports)",
        )?;

        // The folded names the file asks for, each with the index of the first name of the shape that equals it.
        let fold = |n: &[u8]| n.to_ascii_lowercase();
        let mut known: HashMap<Vec<u8>, Option<i32>> = HashMap::new();
        for n in dsq
            .node_names
            .iter()
            .chain(dsq.sequences.iter().map(|q| &q.name))
        {
            if let Some(e) = stored_name(n) {
                known.entry(fold(e)).or_insert(None);
            }
        }
        let mut folded: Vec<u8> = Vec::new();
        if !known.is_empty() {
            for (i, n) in self.names.iter().enumerate() {
                folded.clear();
                folded.extend(n.iter().map(u8::to_ascii_lowercase));
                if let Some(slot) = known.get_mut(&folded) {
                    slot.get_or_insert(i as i32);
                }
            }
        }

        // The file's nodes to the shape's: by name, the n-th appearance of a name to the n-th node of that name.
        let mut by_name: HashMap<i32, Vec<usize>> = HashMap::new();
        if !dsq.node_names.is_empty() {
            for (j, n) in self.nodes.iter().enumerate() {
                by_name.entry(n.name).or_default().push(j);
            }
        }
        let not_in_shape =
            ShapeError::Bad("sequence import (a node of the file is not in the shape)");
        let mut node_map: Vec<usize> = reserved(dsq.node_names.len(), usize::MAX, 1);
        let mut seen: HashMap<i32, usize> = HashMap::new();
        for file_name in &dsq.node_names {
            let index = match stored_name(file_name) {
                Some(n) => known
                    .get(&fold(n))
                    .copied()
                    .flatten()
                    .ok_or(not_in_shape.clone())?,
                None => -1,
            };
            let nth = if index >= 0 {
                let c = seen.entry(index).or_insert(0);
                *c += 1;
                *c - 1
            } else {
                0
            };
            match by_name.get(&index).and_then(|v| v.get(nth)) {
                Some(&j) => node_map.push(j),
                None if nth > 0 => {
                    return Err(ShapeError::Bad(
                        "sequence import (a node duplicated more in the file than in the shape)",
                    ));
                }
                None => return Err(not_in_shape),
            }
        }

        // Names: a sequence's name is looked up, else added (after the shape's).
        let mut added: Vec<Vec<u8>> = Vec::new();
        let mut added_at: HashMap<Vec<u8>, i32> = HashMap::new();
        let mut sequences: Vec<Sequence> = reserved(dsq.sequences.len(), usize::MAX, 1);
        let base = (
            self.node_rotations.len(),
            self.node_translations.len(),
            self.node_uniform_scales.len(),
            self.node_aligned_scales.len(),
            self.node_arbitrary_scale_factors.len(),
            self.triggers.len(),
            self.ground_translations.len(),
        );
        let overflow = ShapeError::Bad("sequence import (a keyframe index overflows)");
        let moved = |at: i32, by: usize| -> Result<i32, ShapeError> {
            i32::try_from(by)
                .ok()
                .and_then(|by| at.checked_add(by))
                .ok_or(overflow.clone())
        };
        for q in &dsq.sequences {
            let mut s = q.sequence.clone();
            s.name = match stored_name(&q.name) {
                None => -1,
                Some(n) => {
                    let key = fold(n);
                    match known.get(&key).copied().flatten() {
                        Some(i) => i,
                        None => match added_at.get(&key) {
                            Some(&i) => i,
                            None => {
                                let i = (self.names.len() + added.len()) as i32;
                                added_at.insert(key, i);
                                added.push(n.to_vec());
                                i
                            }
                        },
                    }
                }
            };
            s.base_rotation = moved(s.base_rotation, base.0)?;
            s.base_translation = moved(s.base_translation, base.1)?;
            if s.flags & SEQ_UNIFORM_SCALE != 0 {
                s.base_scale = moved(s.base_scale, base.2)?;
            } else if s.flags & SEQ_ALIGNED_SCALE != 0 {
                s.base_scale = moved(s.base_scale, base.3)?;
            } else if s.flags & SEQ_ARBITRARY_SCALE != 0 {
                s.base_scale = moved(s.base_scale, base.4)?;
            }
            // The sets move from the file's nodes to the shape's (a set holds 192 bits: the nodes past that are in no set).
            let mut translation = IntSet::default();
            let mut rotation = IntSet::default();
            let mut scale = IntSet::default();
            for (j, &to) in node_map.iter().enumerate().take(SET_BITS) {
                for (from, into) in [
                    (&s.translation_matters, &mut translation),
                    (&s.rotation_matters, &mut rotation),
                    (&s.scale_matters, &mut scale),
                ] {
                    if from.test(j) && !into.set(to) {
                        return Err(ShapeError::Bad(
                            "sequence import (a node past the 192 a sequence can move)",
                        ));
                    }
                }
            }
            s.translation_matters = translation;
            s.rotation_matters = rotation;
            s.scale_matters = scale;
            s.first_trigger = moved(s.first_trigger, base.5)?;
            s.first_ground_frame = moved(s.first_ground_frame, base.6)?;
            sequences.push(s);
        }
        // Every keyframe the new sequences reach must be in the arrays as they will be (their object and decal states are the
        // shape's own: a file brings none).
        let mut lens = self.seq_lens();
        lens.rotations += dsq.node_rotations.len();
        lens.translations += dsq.node_translations.len();
        lens.uniform += dsq.uniform_scales.len();
        lens.aligned += dsq.aligned_scales.len();
        lens.arbitrary += dsq.arbitrary_scale_factors.len();
        lens.ground += dsq.ground_translations.len();
        lens.triggers += dsq.triggers.len();
        for q in &sequences {
            check_sequence(q, &lens, false)?;
        }
        within(
            "names (with the imports)",
            (self.names.len() + added.len()) as u64,
            MAX_SHAPE_NAMES,
        )?;

        // Nothing can refuse now.
        self.names.extend(added);
        self.node_rotations.extend_from_slice(&dsq.node_rotations);
        self.node_translations
            .extend_from_slice(&dsq.node_translations);
        self.node_uniform_scales
            .extend_from_slice(&dsq.uniform_scales);
        self.node_aligned_scales
            .extend_from_slice(&dsq.aligned_scales);
        self.node_arbitrary_scale_rots
            .extend_from_slice(&dsq.arbitrary_scale_rots);
        self.node_arbitrary_scale_factors
            .extend_from_slice(&dsq.arbitrary_scale_factors);
        self.ground_translations
            .extend_from_slice(&dsq.ground_translations);
        self.ground_rotations
            .extend_from_slice(&dsq.ground_rotations);
        self.triggers.extend_from_slice(&dsq.triggers);
        self.sequences.extend(sequences);
        Ok(())
    }
}

/// The bytes of a small `.dsq` for other modules' tests.
#[cfg(test)]
pub(crate) mod tests_support {
    pub(crate) fn one_sequence() -> Vec<u8> {
        super::tests::sample()
    }
}

#[cfg(test)]
#[path = "dsq_tests.rs"]
mod tests;
