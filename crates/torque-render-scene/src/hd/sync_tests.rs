use super::*;
use crate::hd::cells::StaticCells;
use crate::lights::Tge;
use crate::lights::tests::{camera, light};
use crate::sight::Open;
use crate::tge::tests::tge_world;

struct Files;
impl Assets for Files {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        (path == "corona.png").then(|| crate::textures::tests::png(8, 8, 200))
    }
}

/// The eye draws of a TGE world (a solid Sky, a Sun, one light at 20 units) for the test camera, and a frame given them.
fn synced(look: SkyLook) -> (EyeDraws, HdScene<StaticCells>) {
    let w = tge_world([800.0, 400.0], 0.0, 30.0);
    let mut textures = TextureSet::new(&Files);
    let mut notes = Vec::new();
    let draws = eye_draws(
        &w,
        &Files,
        &camera(),
        800,
        800.0,
        800.0,
        &[light([0.0, 20.0, 0.0])],
        &mut Open,
        &Tge,
        &mut textures,
        &mut notes,
    );
    let mut scene = HdScene::new(StaticCells::<()>::default());
    let local: Vec<usize> = textures
        .into_textures()
        .into_iter()
        .map(|t| scene.textures.intern(t))
        .collect();
    scene.set_eye_draws(&w, &draws, &local, look, &mut |_, _| 0);
    (draws, scene)
}

#[test]
fn a_tge_worlds_sky_and_coronas_become_the_enhanced_frames() {
    let (draws, scene) = synced(SkyLook::default());
    // The solid Sky's box and bans, no flare (TGE's set ghosts no fxSunLight), no planes, the light's corona.
    assert!(!draws.sky.is_empty());
    // The clear: the fog colour's bytes (127, 153, 178).
    assert_eq!(
        draws.clear,
        Some([127.0, 153.0, 178.0].map(|b: f32| b / 255.0))
    );
    assert!(draws.local_flare.is_none() && draws.planes.is_empty());
    assert_eq!(draws.coronas.len(), 1);
    assert_eq!(scene.sky.len(), draws.sky.len());
    assert!(scene.sky.iter().all(|d| {
        d.vertices
            .iter()
            .all(|v| v.material & material::SKY_EYE != 0)
    }));
    assert_eq!(scene.sky_eye, [0.0; 3]);
    assert!(scene.clouds.is_some() && !scene.atmosphere && scene.flare.is_none());
    // The corona: a billboard anchored at the light, its half size the quad's.
    let (dist, c) = &scene.coronas[0];
    assert_eq!(*dist, 20.0);
    for v in &c.vertices {
        assert_eq!(v.position, [0.0, 20.0, 0.0]);
        assert_eq!(
            v.material & (material::BILLBOARD | material::ANCHORED | material::EMITTER),
            material::BILLBOARD | material::ANCHORED | material::EMITTER
        );
        assert!((f32::from_bits(v.extra) - 2.0).abs() < 1e-4);
    }
}

#[test]
fn the_engines_own_sky_takes_the_boxs_place_where_the_look_and_the_choice_allow() {
    let look = SkyLook {
        own_sky: true,
        default_sky: true,
        sun_moves: false,
    };
    let (_, scene) = synced(look);
    assert!(scene.atmosphere);
    assert!(
        scene.sky.is_empty(),
        "the solid Sky has no cloud layers to keep"
    );
    let (_, server) = synced(SkyLook {
        default_sky: false,
        ..look
    });
    assert!(!server.atmosphere && !server.sky.is_empty());
}

#[test]
fn a_light_is_a_linear_point_light_of_the_same_brightness_and_a_corona_an_anchored_quad() {
    let mut l = light([1.0, 2.0, 3.0]);
    l.color = [2.0, 1.0, 0.0];
    l.attached = Some(4);
    let h = HdLight::of_light(&l);
    assert_eq!(h.color[0], 2.0);
    assert!((h.color[1] - 2.0 * srgb_to_linear(0.5)).abs() < 1e-6);
    assert_eq!((h.radius, h.carrier), (10.0, Some(4)));
    let d = corona_draw([1.0, 2.0, 3.0], 0.5, [255, 128, 0]);
    assert_eq!(d.vertices.len(), 4);
    assert!(
        d.vertices
            .iter()
            .all(|v| v.position == [1.0, 2.0, 3.0] && (f32::from_bits(v.extra) - 0.5).abs() < 1e-6)
    );
}
