use super::*;
use crate::synth::{DifSpec, dif_hulls};

const SPEC: DifSpec = DifSpec {
    version: 0,
    plain_surfaces: true,
    everything: false,
    border: 0,
};

#[test]
fn a_dif_s_hulls_keep_the_file_s_ranges_and_flips() {
    let bytes = dif_hulls(SPEC, 3, 6);
    let d = dif::parse(&bytes).unwrap();
    let i = &d.details[0];
    let c = IMPORTER.convexes(&bytes).unwrap();
    assert_eq!(c.hulls.len(), 3);
    assert_eq!(c.points, i.points);
    for (k, h) in c.hulls.iter().enumerate() {
        assert_eq!(h.point_start, i.hulls.hulls[k].hull_start);
        assert_eq!(h.plane_start, i.hulls.hulls[k].plane_start);
    }
    assert_eq!(
        c.hulls[2].plane_end as usize,
        i.hulls.hull_plane_indices.len()
    );
    assert_eq!(c.hulls[0].plane_end, c.hulls[1].plane_start);
    for (r, &raw) in c.plane_refs.iter().zip(&i.hulls.hull_plane_indices) {
        assert_eq!(r.flipped, raw & PLANE_FLIPPED != 0);
        assert_eq!(r.index, u32::from(raw & !PLANE_FLIPPED));
    }
}

#[test]
fn a_ter_s_layers_and_heights_are_its_own() {
    let bytes = crate::synth::ter(3, &["grass", "rock"]);
    let t = ter::parse(&bytes).unwrap();
    let f = IMPORTER.heightfield(&bytes).unwrap();
    assert_eq!(f.layers.len(), t.materials.len());
    for i in [0, 1, 300, ter::SAMPLES - 1] {
        assert_eq!(
            f.height(i % ter::SIZE, i / ter::SIZE),
            t.height(i % ter::SIZE, i / ter::SIZE)
        );
        for k in 0..t.materials.len() {
            assert_eq!(f.weight(k, i), t.alpha(k, i));
        }
    }
    assert_eq!(
        IMPORTER.named_files(FileKind::Heightfield, &bytes).unwrap(),
        ["grass", "rock"]
    );
}

#[test]
fn hostile_bytes_are_refused_with_the_reader_s_reason() {
    for kind in [FileKind::Heightfield, FileKind::Model] {
        assert!(IMPORTER.named_files(kind, b"\x01\x02").is_err());
    }
    assert!(IMPORTER.heightfield(b"").is_err());
    assert!(IMPORTER.surfaces(b"xx").is_err());
    assert!(IMPORTER.convexes(b"xx").is_err());
}

#[test]
fn a_placement_centres_the_block_at_height_0_and_names_its_holes() {
    let p = terrain_placement(8, true, &[2 | (1 << 8) | (3 << 16)]);
    assert_eq!(p.origin, [-1024.0, -1024.0, 0.0]);
    assert_eq!(p.square, 8.0);
    assert!(p.holes[2 + 256] && p.holes[4 + 256] && !p.holes[5 + 256]);
}

#[test]
fn surfaces_that_repeat_one_run_stop_at_the_budget() {
    let mut d = dif::parse(&crate::synth::dif(SPEC)).unwrap();
    let i = &mut d.details[0];
    let s = *i.surfaces.iter().find(|s| s.winding_count >= 3).unwrap();
    i.surfaces = vec![s; 10_000];
    let budget = i.windings.len() * SURFACE_POINTS_PER_WINDING;
    let out = surfaces(&d);
    let vertices: usize = out.by_material.values().map(Vec::len).sum();
    assert!(vertices > 0);
    // Each point of a strip gives at most 3 vertices.
    assert!(vertices <= 3 * budget, "{vertices} > 3 x {budget}");
}

#[test]
fn the_special_materials_are_tges_four_with_their_case_and_an_importer_may_name_its_own() {
    assert!(SPECIAL.iter().all(|&m| is_special(m)));
    for m in ["null", "EMITTER", "SKY", "wall"] {
        assert!(!is_special(m), "{m}");
    }
    // The synthetic interior's materials are "wall" and "NULL": only "wall" is a file it names.
    let bytes = crate::synth::dif(SPEC);
    assert_eq!(
        IMPORTER.named_files(FileKind::Model, &bytes).unwrap(),
        ["wall"]
    );
    let none = Legacy {
        special: |_| false,
        ..Legacy::TGE
    };
    assert_eq!(
        none.named_files(FileKind::Model, &bytes).unwrap(),
        ["wall", "NULL"]
    );
    // A surface of a special material is not drawn: with every material special, nothing is.
    let d = dif::parse(&bytes).unwrap();
    assert!(!surfaces(&d).by_material.is_empty());
    assert!(surfaces_with(&d, |_| true).by_material.is_empty());
    assert!(!surfaces(&d).by_material.contains_key("NULL"));
}

#[test]
fn the_border_word_layout_reads_only_through_an_importer_that_asks_for_it() {
    let padded = crate::synth::dif(DifSpec { border: 2, ..SPEC });
    // TGE's layout does not read the file to its end (it may still make something of the start).
    assert!(!dif::parse(&padded).is_ok_and(|d| d.trailing == 0));
    assert!(dif::parse_with_border_pad(&padded).unwrap().border_pad);
    let lenient = Legacy {
        border_pad: true,
        ..Legacy::TGE
    };
    assert!(lenient.convexes(&padded).is_ok());
}

#[test]
fn the_engine_s_surface_flags_mark_their_material_and_an_unmarked_file_marks_none() {
    let mut d = dif::parse(&crate::synth::dif(SPEC)).unwrap();
    assert!(surfaces(&d).material_flags.is_empty());
    for s in &mut d.details[0].surfaces {
        s.flags |= dif::surface_flags::TRANSLUCENT | dif::surface_flags::NEAREST;
    }
    let back = dif::parse(&dif::write(&d).unwrap()).unwrap();
    let out = surfaces(&back);
    assert_eq!(
        out.material_flags.get("wall"),
        Some(&MaterialFlags::TRANSLUCENT.with(MaterialFlags::NEAREST))
    );
}

/// The synthetic interior with its two-name material list replaced by `names`.
fn dif_with_materials(names: &[String]) -> Vec<u8> {
    let bytes = crate::synth::dif(SPEC);
    let mut old = vec![1u8];
    old.extend_from_slice(&2u32.to_le_bytes());
    for name in ["textures/wall", "NULL"] {
        old.push(name.len() as u8);
        old.extend_from_slice(name.as_bytes());
    }
    let at = bytes.windows(old.len()).position(|w| w == old).unwrap();
    let mut new = vec![1u8];
    new.extend_from_slice(&(names.len() as u32).to_le_bytes());
    for name in names {
        new.push(name.len() as u8);
        new.extend_from_slice(name.as_bytes());
    }
    let mut out = bytes[..at].to_vec();
    out.extend_from_slice(&new);
    out.extend_from_slice(&bytes[at + old.len()..]);
    out
}

#[test]
fn a_model_with_many_distinct_materials_is_named_in_linear_time() {
    // 200,000 names took minutes when each was compared with every name kept; the pass is a set lookup per name.
    let names: Vec<String> = (0..200_000).map(|i| format!("m{i:07}")).collect();
    let bytes = dif_with_materials(&names);
    let started = std::time::Instant::now();
    let named = IMPORTER.named_files(FileKind::Model, &bytes).unwrap();
    let took = started.elapsed();
    assert_eq!(named, names);
    let bound = if cfg!(debug_assertions) { 2_000 } else { 100 };
    assert!(took.as_millis() < bound, "{took:?}");
}

#[test]
fn material_names_that_differ_only_in_case_are_named_once_and_keep_their_first_spelling() {
    let names = ["Rock", "ROCK", "NULL", "Sand", "rock", "sand"].map(String::from);
    let bytes = dif_with_materials(&names);
    assert_eq!(
        IMPORTER.named_files(FileKind::Model, &bytes).unwrap(),
        ["Rock", "Sand"]
    );
}
