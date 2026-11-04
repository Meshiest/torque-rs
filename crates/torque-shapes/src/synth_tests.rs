use super::*;
use crate::dts;
use crate::write::write_dts;

fn free(_: u64) -> bool {
    true
}

#[test]
fn a_synthetic_model_writes_and_parses_back_to_itself() {
    for seed in 0..300 {
        let mut rng = Rng(seed);
        let model = model(&mut rng, true, 1 + (seed % 3) as usize);
        let bytes = write_dts(&model);
        let (back, info) =
            dts::parse_with_info(&bytes, &mut free).unwrap_or_else(|e| panic!("seed {seed}: {e}"));
        assert_eq!(
            back.shape.meshes.len(),
            model.shape.meshes.len(),
            "seed {seed}"
        );
        assert_eq!(back.shape, model.shape, "seed {seed}");
        assert_eq!(back.render, model.render, "seed {seed}");
        // The padding of the 16- and 8-bit sections comes back as what it is: zeros.
        assert_eq!(back.file.materials, model.file.materials, "seed {seed}");
        assert_eq!(back.file.trailing, model.file.trailing, "seed {seed}");
        assert!(
            back.file.left.iter().flatten().all(|&b| b == 0),
            "seed {seed}"
        );
        assert!(back.file.left.iter().all(|l| l.len() < 4), "seed {seed}");
        assert_eq!(info.guards.wrong, 0);
        assert_eq!(info.guards.read, expected_guards(&model), "seed {seed}");
        assert_eq!(write_dts(&back), bytes, "seed {seed}");
    }
}

#[test]
fn a_consistent_synthetic_model_passes_the_bounds_check() {
    for seed in 0..200 {
        let mut rng = Rng(seed);
        let model = model(&mut rng, true, 2);
        let report = bounds::check(&model, &mut free).unwrap();
        assert!(report.all_equal(), "seed {seed}: {report:?}");
    }
}

#[test]
fn an_inconsistent_synthetic_model_fails_it_somewhere() {
    let failures = (0..100)
        .filter(|&seed| {
            let mut rng = Rng(seed);
            let model = model(&mut rng, false, 2);
            !bounds::check(&model, &mut free).unwrap().all_equal()
        })
        .count();
    assert!(failures > 90, "{failures}");
}

#[test]
fn the_generator_makes_every_mesh_kind_and_shared_arrays() {
    let (mut skin, mut sorted, mut child, mut null) = (0, 0, 0, 0);
    for seed in 0..200 {
        let mut rng = Rng(seed);
        let model = model(&mut rng, true, 3);
        for (m, r) in model.shape.meshes.iter().zip(&model.render) {
            match (m, r) {
                (Some(m), Some(r)) => {
                    skin += usize::from(matches!(r.extra, MeshExtra::Skin(_)));
                    sorted += usize::from(matches!(r.extra, MeshExtra::Sorted(_)));
                    child += usize::from(m.parent >= 0);
                }
                _ => null += 1,
            }
        }
    }
    assert!(
        skin > 20 && sorted > 20 && child > 20 && null > 20,
        "{skin} {sorted} {child} {null}"
    );
}

#[test]
fn a_solid_shape_writes_and_parses_back_with_its_triangles() {
    for strips in [false, true] {
        let model = solid_shape(
            [-10.0, -10.0, -10.0, 10.0, 10.0, 10.0],
            &[("Collision-1", [-1.0, -1.0, -1.0, 1.0, 1.0, 1.0])],
            [0, 0, 0, 32767],
            [0.0; 3],
            strips,
        );
        let bytes = write_dts(&model);
        let back = dts::parse(&bytes, &mut free).unwrap_or_else(|e| panic!("{strips}: {e}"));
        assert_eq!(back.shape.meshes, model.shape.meshes);
        let mesh = back.shape.meshes[0].as_ref().unwrap();
        assert_eq!(crate::collide::mesh_triangles(mesh), 12, "{strips}");
    }
}

#[test]
fn mutation_changes_the_bytes_and_is_repeatable() {
    let mut rng = Rng(5);
    let bytes = write_dts(&model(&mut rng, true, 1));
    let a = mutate(&mut Rng(9), &bytes);
    let b = mutate(&mut Rng(9), &bytes);
    assert_eq!(a, b);
    assert!((0..50).any(|i| mutate(&mut Rng(i), &bytes) != bytes));
}

/// The guard words a version-24 file of this shape holds: seventeen for the groups of the header, and two around every mesh
/// body (three for a skin or a sorted mesh, which close with a guard of their own).
fn expected_guards(model: &crate::shape::Model) -> u32 {
    let mut n = 17;
    for (m, r) in model.shape.meshes.iter().zip(&model.render) {
        if let (Some(_), Some(r)) = (m, r) {
            n += 2 + u32::from(!matches!(r.extra, crate::shape::MeshExtra::None));
        }
    }
    n
}
