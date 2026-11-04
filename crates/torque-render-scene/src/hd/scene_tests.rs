use super::*;
use crate::hd::cells::StaticCells;

#[test]
fn a_new_frame_over_plain_cells_draws_nothing_and_waits_for_nothing() {
    let mut scene = HdScene::new(StaticCells::<()>::default());
    assert!(scene.cells.cells().is_empty() && scene.objects.is_empty() && scene.lights.is_empty());
    assert_eq!(scene.fov, DEFAULT_FOV);
    assert!(!scene.pending());
    scene.textures_pending = true;
    assert!(scene.pending());
}

#[test]
fn the_sky_sun_is_the_suns_direction_without_a_painted_one() {
    let mut scene = HdScene::new(StaticCells::<()>::default());
    scene.sun.towards = [0.0, 0.6, 0.8];
    assert_eq!(scene.sky_sun(), [0.0, 0.6, 0.8]);
}
