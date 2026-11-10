//! Studio models from synthetic files: assembly, animation, and hostile files (overlapping tables, long animations).

use crate::model::{Choice, assemble};
use crate::{mdl::Mdl, synth, vtx::Vtx, vvd::Vvd};

#[test]
fn a_synthetic_model_assembles_and_animates() {
    let m = Mdl::parse(&synth::mdl(8)).unwrap();
    assert_eq!(m.bones.len(), 1);
    assert_eq!(
        m.material_paths(0),
        vec!["materials/models/synth/skin.vmt".to_string()]
    );
    let v = Vvd::parse(&synth::vvd()).unwrap();
    let t = Vtx::parse(&synth::vtx(1, 3, false)).unwrap();
    let mut sm = assemble(&m, &v, &t, &Choice::default()).unwrap();
    assert_eq!(sm.model.meshes.len(), 1);
    assert_eq!(sm.model.meshes[0].triangles(), 1);
    sm.add_sequences(&m, &m, None, &mut |_| true).unwrap();
    let a = &sm.model.animations[0];
    assert_eq!((a.name.as_str(), a.keys, a.cyclic), ("wave", 7, true));
    // Frame 3's position along x: value 150 times the scale 1/32.
    let p = a.tracks[0].translations[3];
    assert!((p[0] - 150.0 / 32.0).abs() < 1e-5, "{p:?}");
}

#[test]
fn frames_decode_in_one_pass_and_agree_with_single_frames() {
    let m = Mdl::parse(&synth::mdl(200)).unwrap();
    let all = m.frames(0, None, 200).unwrap();
    assert_eq!(all.len(), 200);
    for f in [0u32, 1, 57, 199] {
        assert_eq!(m.frame(0, f, None).unwrap(), all[f as usize]);
    }
}

#[test]
fn overlapping_strip_groups_are_refused() {
    // 400 strip groups that all read one list of 600 indices: far more indices than the file holds.
    let b = synth::vtx(400, 600, true);
    assert!(Vtx::parse(&b).is_err());
    // The same file without the overlap reads.
    assert!(Vtx::parse(&synth::vtx(4, 600, false)).is_ok());
}

#[test]
fn overlapping_model_tables_are_refused() {
    // One body part table entry copied over many places: every body part points at one table of many models.
    let mut b = synth::mdl(8);
    let part = i32::from_le_bytes(b[236..240].try_into().unwrap()) as usize;
    let models_at = b.len();
    let n_models = 300usize;
    b.resize(models_at + n_models * 148, 0);
    let n_parts = 2000usize;
    let parts_at = b.len();
    b.resize(parts_at + n_parts * 16, 0);
    for p in 0..n_parts {
        let at = parts_at + p * 16;
        b[at..at + 4].copy_from_slice(&(part as i32 - at as i32).to_le_bytes());
        b[at + 4..at + 8].copy_from_slice(&(n_models as i32).to_le_bytes());
        b[at + 12..at + 16].copy_from_slice(&((models_at as i32) - at as i32).to_le_bytes());
    }
    b[232..236].copy_from_slice(&(n_parts as i32).to_le_bytes());
    b[236..240].copy_from_slice(&(parts_at as i32).to_le_bytes());
    let e = Mdl::parse(&b).unwrap_err();
    assert!(e.to_string().contains("over the cap"), "{e}");
}

#[test]
fn sections_are_capped_to_what_fits() {
    // An animation that claims a section per frame for a billion frames, its section list at the file's end.
    let mut b = synth::mdl(8);
    let anim = i32::from_le_bytes(b[184..188].try_into().unwrap()) as usize;
    b[anim + 16..anim + 20].copy_from_slice(&1_000_000_000i32.to_le_bytes());
    b[anim + 84..anim + 88].copy_from_slice(&1i32.to_le_bytes());
    let end = b.len() as i32 - 16;
    b[anim + 80..anim + 84].copy_from_slice(&(end - anim as i32).to_le_bytes());
    if let Ok(m) = Mdl::parse(&b) {
        assert!(m.frames(0, None, u32::MAX).is_err());
    }
}

/// The synthetic model with a table of `n` entries of `stride` bytes, each `entry`, appended, and the header's count and offset
/// at `at` pointing at it.
fn with_table(
    n: usize,
    stride: usize,
    at: usize,
    entry: impl Fn(usize, usize) -> Vec<u8>,
) -> Vec<u8> {
    let mut b = synth::mdl(8);
    let table = b.len();
    for k in 0..n {
        let e = entry(table + k * stride, table);
        assert_eq!(e.len(), stride);
        b.extend_from_slice(&e);
    }
    b[at..at + 4].copy_from_slice(&(n as i32).to_le_bytes());
    b[at + 4..at + 8].copy_from_slice(&(table as i32).to_le_bytes());
    b
}

#[test]
fn texture_folders_includes_and_skins_are_capped() {
    // 65 texture folders (each naming the model's own name string at 12): over the cap of 64.
    let b = with_table(65, 4, 212, |_, _| 12i32.to_le_bytes().to_vec());
    assert!(
        Mdl::parse(&b)
            .unwrap_err()
            .to_string()
            .contains("texture folders")
    );
    // 33 include models, each naming itself.
    let b = with_table(33, 8, 336, |at, _| {
        let mut e = vec![0u8; 8];
        e[4..8].copy_from_slice(&(12 - at as i32).to_le_bytes());
        e
    });
    assert!(
        Mdl::parse(&b)
            .unwrap_err()
            .to_string()
            .contains("include models")
    );
    // 2,000 skin families of one reference.
    let mut b = synth::mdl(8);
    let at = b.len();
    b.resize(at + 2000 * 2, 0);
    b[220..224].copy_from_slice(&1i32.to_le_bytes());
    b[224..228].copy_from_slice(&2000i32.to_le_bytes());
    b[228..232].copy_from_slice(&(at as i32).to_le_bytes());
    assert!(
        Mdl::parse(&b)
            .unwrap_err()
            .to_string()
            .contains("skin families")
    );
}

#[test]
fn a_boneless_animation_is_still_capped() {
    let mut b = synth::mdl(8);
    b[156..160].copy_from_slice(&0i32.to_le_bytes());
    let anim = i32::from_le_bytes(b[184..188].try_into().unwrap()) as usize;
    b[anim + 16..anim + 20].copy_from_slice(&i32::MAX.to_le_bytes());
    let m = Mdl::parse(&b).unwrap();
    assert!(m.frames(0, None, u32::MAX).is_err());
}
