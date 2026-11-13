//! A `PropData` edited while the server runs (a field written, or the datablock declared again) refreshes the props built from
//! it: each body is made again with the new numbers at the exact pose and velocities it had, through the server's drain of the
//! changed datablocks, with a real server over the memory transport.

use crate::rig;

use rig::{Rig, RigProps, files};
use torque_game::scene::refresh;
use torque_physics_rapier::{BodyId, BodyState};

/// The body of prop `id` in the server's physics world, with its mass.
fn body(rig: &Rig, id: u32) -> (BodyState, f32) {
    let props = rig.props();
    let s = props.0.borrow();
    let physics = s.world.as_ref().expect("a world").physics();
    (
        physics.body(BodyId(id)).expect("a body"),
        physics.mass_of(BodyId(id)).expect("a mass"),
    )
}

/// Spawns 64 crates in the air on a grid (after the server's first ticks, which catch up on its clock) and lets them fall a
/// few ticks, so every body moves.
fn falling_crates(rig: &mut Rig) -> Vec<u32> {
    rig.run(3);
    let ids: Vec<u32> = (0..64)
        .map(|k| {
            let (x, y, z) = (
                (k % 8) as f32 * 3.0 - 10.0,
                (k / 8) as f32 * 3.0 - 10.0,
                10.0 + k as f32 * 0.25,
            );
            rig.eval(&format!("return propSpawn(Crate, \"{x} {y} {z}\");"))
                .parse()
                .expect("a prop")
        })
        .collect();
    rig.run(5);
    ids
}

#[test]
fn a_redeclare_doubling_the_mass_refits_all_64_props_at_their_exact_pose_and_velocities() {
    let files = files();
    let mut rig = Rig::new(&files, &[]);
    let ids = falling_crates(&mut rig);
    let crate_db: u32 = rig.eval("return Crate.getId();").parse().unwrap();
    assert_eq!(
        refresh::users(rig.server.vm(), crate_db),
        64,
        "every prop counts"
    );
    let before: Vec<(BodyState, f32)> = ids.iter().map(|&id| body(&rig, id)).collect();
    assert!(
        before
            .iter()
            .all(|(s, m)| s.linvel[2] < 0.0 && (m - 20.0).abs() < 1e-3),
        "falling, 20 units of mass each: {:?}",
        before[0]
    );
    let stats = refresh::stats(rig.server.vm());
    // The redeclare, then the drain a tick runs before its objects step (run here by hand, so the refit is seen on its own).
    rig.eval(
        r#"datablock PropData(Crate) { shapeFile = "kit/cube.dts"; size = "1 1 1"; mass = 40; };"#,
    );
    let errors = refresh::drain(rig.server.vm_mut());
    assert!(errors.is_empty(), "{errors:?}");
    let now = refresh::stats(rig.server.vm());
    assert_eq!((now.marks - stats.marks, now.heavy - stats.heavy), (1, 64));
    let mut worst = 0.0f32;
    for (&id, (s, _)) in ids.iter().zip(&before) {
        let (after, mass) = body(&rig, id);
        assert!((mass - 40.0).abs() < 1e-3, "prop {id}: mass {mass}");
        for k in 0..3 {
            worst = worst.max((after.position[k] - s.position[k]).abs());
        }
        assert_eq!(
            after.linvel.map(f32::to_bits),
            s.linvel.map(f32::to_bits),
            "prop {id}: the velocity is kept bit for bit"
        );
        assert_eq!(after.angvel.map(f32::to_bits), s.angvel.map(f32::to_bits));
        assert_eq!(
            after.rotation.map(f32::to_bits),
            s.rotation.map(f32::to_bits)
        );
    }
    assert!(worst <= 1e-4, "position error {worst}");
    // A write is the light refresh of the same work: the mass follows at the next tick.
    rig.eval("Crate.mass = 10;");
    rig.run(1);
    let now = refresh::stats(rig.server.vm());
    assert_eq!(now.light - stats.light, 64);
    assert!(ids.iter().all(|&id| (body(&rig, id).1 - 10.0).abs() < 1e-3));
    assert_eq!(rig.server_props().len(), 64, "every prop is still there");
    // A deleted prop is counted off its datablock; a datablock without users is not refreshed.
    rig.eval(&format!("propDelete({});", ids[0]));
    rig.run(1);
    assert_eq!(refresh::users(rig.server.vm(), crate_db), 63);
    let rest: Vec<String> = ids[1..]
        .iter()
        .map(|id| format!("propDelete({id});"))
        .collect();
    rig.eval(&rest.concat());
    rig.run(2);
    assert_eq!(rig.server_props().len(), 0);
    assert_eq!(refresh::users(rig.server.vm(), crate_db), 0);
    let stats = refresh::stats(rig.server.vm());
    rig.eval("Crate.mass = 20;");
    rig.run(1);
    assert_eq!(
        refresh::stats(rig.server.vm()).marks,
        stats.marks,
        "no users, no mark"
    );
}
