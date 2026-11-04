use super::*;
use crate::source::{SkyDesc, SkyMainDesc};

#[test]
fn a_side_face_is_cut_at_the_horizon_keeping_its_first_two_corners() {
    let b = SkyBox::new(900.0, 512);
    let p = [b.points[0], b.points[1], b.points[6], b.points[5]];
    let c = clip_above(&p, 0.0);
    assert_eq!(c.len(), 4);
    assert_eq!(c[0], p[0]);
    assert_eq!(c[1], p[1]);
    assert!(c[2][2].abs() < 1e-3 && c[3][2].abs() < 1e-3);
    assert!((c[2][0] - p[1][0]).abs() < 1e-3 && (c[3][0] - p[0][0]).abs() < 1e-3);
    // The radius: 0.95 times the visible distance, the corners on that sphere.
    let r = (b.points[2].iter().map(|v| v * v).sum::<f32>()).sqrt();
    assert!((r - 855.0).abs() < 0.01, "{r}");
    assert!((b.tex[0][0] - 0.5 / 512.0).abs() < 1e-7);
}

#[test]
fn the_bans_ring_the_camera_at_the_horizon_and_offset_height_above() {
    let b = SkyBox::new(900.0, 512);
    let (rings, corners) = bans(&b, [0.0, BAN_OFFSET], &SkyProfile::TGE);
    let r0 = (rings[0][0][0].powi(2) + rings[0][0][1].powi(2)).sqrt();
    assert!((r0 - b.sky_box_pt[0]).abs() < 1e-3);
    assert_eq!(rings[1][4][2], BAN_OFFSET);
    assert!(
        rings[0][4][1] > 0.0,
        "the ring turns towards +y (TGE's euler matrix on (-m, 0, 0))"
    );
    assert_eq!(corners[0][2], -1.0);
}

struct NoFiles;
impl crate::Assets for NoFiles {
    fn read(&self, _: &str) -> Option<Vec<u8>> {
        None
    }
}

/// A world of one Sky, with or without a map.
struct World {
    sky: SkyDesc,
    map: bool,
}
impl SceneSource for World {
    fn eye(&self) -> Option<crate::source::Eye> {
        None
    }
    fn fov_deg(&self) -> f32 {
        90.0
    }
    fn max_view_distance(&self) -> Option<f32> {
        None
    }
    fn sky(&self) -> Option<SkyDesc> {
        Some(self.sky.clone())
    }
    fn sun(&self) -> Option<crate::source::SunDesc> {
        None
    }
    fn sun_flare(&self) -> Option<crate::source::FlareDesc> {
        None
    }
    fn has_map(&self) -> bool {
        self.map
    }
}

/// A Sky with no material list: the box's faces in the solid colour, untextured; with a profile that keeps that to maps, no box
/// where there is no map.
#[test]
fn a_sky_without_a_material_list_is_drawn_in_its_solid_colour() {
    let mut world = World {
        sky: SkyDesc {
            main: Some(SkyMainDesc {
                sky_color: [26.0 / 255.0, 26.0 / 255.0, 77.0 / 255.0],
                ..Default::default()
            }),
            ..Default::default()
        },
        map: false,
    };
    let mul = Multipliers {
        fog: [1.0; 3],
        sky: sky_color(&world),
    };
    let on_maps = SkyProfile {
        solid_only_on_map: true,
        ..SkyProfile::TGE
    };
    let draw = |world: &World| {
        let files = NoFiles;
        let mut textures = TextureSet::new(&files);
        draws(
            &on_maps,
            world,
            &files,
            [0.0; 3],
            500.0,
            mul,
            &mut textures,
            &mut Vec::new(),
        )
        .unwrap()
    };
    assert!(solid(&world, &SkyProfile::TGE));
    assert!(!solid(&world, &on_maps));
    assert!(draw(&world).boxes.is_empty());
    world.map = true;
    assert!(solid(&world, &on_maps));
    let d = draw(&world);
    assert_eq!(d.boxes.len(), 5, "four sides and the top");
    for b in &d.boxes {
        assert!(b.texture.is_none(), "{}", b.label);
        assert!(
            b.vertices.iter().all(|v| v.color == [26, 26, 77, 255]),
            "{}",
            b.label
        );
    }
    assert!(
        d.after_flare
            .iter()
            .all(|a| !a.label.starts_with("sky cloud"))
    );
}
