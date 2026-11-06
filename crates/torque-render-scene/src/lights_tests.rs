use super::*;
use crate::sight::Open;

struct Files;
impl crate::Assets for Files {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        (path == "corona.png").then(|| crate::textures::tests::png(8, 8, 200))
    }
}

/// An eye at the origin looking along +y.
pub(crate) fn camera() -> Camera {
    let eye = crate::source::Eye {
        position: [0.0; 3],
        pitch: 0.0,
        yaw: 0.0,
        fov: Some(90),
    };
    Camera::from_eye(eye, 90.0_f32, 500.0, 800, 600)
}

/// A light at `p` with a flare of `FlareBitmap` `corona.png`, near size 1 within 10 units, far size 3 past 30.
pub(crate) fn light(p: [f32; 3]) -> Light {
    Light {
        position: p,
        color: [0.5, 0.25, 0.0],
        radius: 10.0,
        flare: Some(Flare {
            bitmap: "corona.png".into(),
            color: [1.0, 1.0, 1.0],
            constant_size: None,
            near_size: 1.0,
            far_size: 3.0,
            near_distance: 10.0,
            far_distance: 30.0,
            blend_mode: 0,
            link_flare: false,
            link_flare_size: false,
            scale: 1.0,
        }),
        through: None,
        attached: None,
    }
}

/// Limits as a world may set them.
struct Near;
impl CoronaRules for Near {
    fn reach(&self) -> f32 {
        50.0
    }
    fn max_tests(&self) -> usize {
        1
    }
}

/// Blocks the corona lines of sight that do not pass through object 7.
struct Wall;
impl LineOfSight for Wall {
    fn blocked(
        &mut self,
        _: [f32; 3],
        _: [f32; 3],
        s: Sight,
        through: Option<u32>,
    ) -> Option<bool> {
        Some(s == Sight::Corona && through != Some(7))
    }
}

#[test]
fn a_coronas_size_runs_from_its_near_to_its_far_size_over_the_distances() {
    let f = light([0.0; 3]).flare.unwrap();
    assert_eq!(corona_size(&f, 5.0), 1.0);
    assert_eq!(corona_size(&f, 20.0), 2.0);
    assert_eq!(corona_size(&f, 40.0), 3.0);
    let constant = Flare {
        constant_size: Some(4.0),
        ..f
    };
    assert_eq!(corona_size(&constant, 40.0), 4.0);
}

#[test]
fn tge_draws_every_corona_in_sight_and_a_world_may_limit_them() {
    let mut textures = TextureSet::new(&Files);
    let lights = [light([0.0, 20.0, 0.0]), light([0.0, 200.0, 0.0])];
    let drawn = coronas(&lights, &camera(), &mut Open, &Tge, &mut textures);
    assert_eq!(drawn.len(), 2);
    let (dist, d) = &drawn[0];
    assert_eq!(*dist, 20.0);
    assert_eq!(d.blend, Blend::Additive);
    assert!(!d.depth_test && !d.depth_write && !d.lighting);
    // Half a side of 2 at 20 units: the quad spans 4 across.
    let xs: Vec<f32> = d.vertices.iter().map(|v| v.position[0]).collect();
    let span =
        xs.iter().copied().fold(f32::MIN, f32::max) - xs.iter().copied().fold(f32::MAX, f32::min);
    assert!((span - 4.0).abs() < 1e-4, "{span}");
    // The world's reach leaves out the far one, its one test the second near one.
    let three = [
        light([0.0, 20.0, 0.0]),
        light([0.0, 200.0, 0.0]),
        light([0.0, 25.0, 0.0]),
    ];
    let near = coronas(&three, &camera(), &mut Open, &Near, &mut textures);
    assert_eq!(near.len(), 1);
    assert_eq!(near[0].0, 20.0);
    assert_eq!(tested_coronas(&three, [0.0; 3], &Near), vec![0]);
    // A blocked line of sight hides it, unless it runs through the object the light sits in.
    assert!(coronas(&lights[..1], &camera(), &mut Wall, &Tge, &mut textures).is_empty());
    let inside = Light {
        through: Some(7),
        ..light([0.0, 20.0, 0.0])
    };
    assert_eq!(
        coronas(&[inside], &camera(), &mut Wall, &Tge, &mut textures).len(),
        1
    );
}

#[test]
fn a_linked_corona_takes_the_lights_colour_and_its_luminance_as_size() {
    let mut textures = TextureSet::new(&Files);
    let mut l = light([0.0, 5.0, 0.0]);
    if let Some(f) = &mut l.flare {
        f.link_flare = true;
        f.link_flare_size = true;
    }
    let d = &coronas(&[l], &camera(), &mut Open, &Tge, &mut textures)[0].1;
    assert_eq!(&d.vertices[0].color[..3], &[128, 64, 0]);
    let luma = 0.5 * 0.212_671 + 0.25 * 0.715_16;
    let xs: Vec<f32> = d.vertices.iter().map(|v| v.position[0]).collect();
    let span =
        xs.iter().copied().fold(f32::MIN, f32::max) - xs.iter().copied().fold(f32::MAX, f32::min);
    assert!((span - 2.0 * luma).abs() < 1e-5, "{span}");
}
