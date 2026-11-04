use super::*;
use ghosts::F32;
use ghosts::sky::{Sky, SkyInit, Sun, SunUpdate};

/// A world of TGE's set with a Sky (its textures off: a solid sky) at `distances` and a Sun towards `azimuth`, `elevation`.
pub(crate) fn tge_world(
    distances: [f32; 2],
    azimuth: f32,
    elevation: f32,
) -> ClientWorld<TgeClientSet> {
    let mut w = ClientWorld::<TgeClientSet>::default();
    let ghost = |class, record| ghost_world::world::Ghost {
        class: Some(class),
        serial: 1,
        always: true,
        record: Some(record),
        part: (),
        position: None,
        updates: 0,
    };
    let sky = Sky {
        init: Some(SkyInit {
            fog_color: [0.5, 0.6, 0.7].map(F32::new),
            solid_fill_color: [0.2, 0.4, 0.8].map(F32::new),
            ..SkyInit::default()
        }),
        visibility: Some(distances.map(F32::new)),
        ..Sky::default()
    };
    w.ghosts
        .insert(1, ghost(TgeClass::Sky, TgeRecord::Sky(sky)));
    let (az, el) = (azimuth.to_radians(), elevation.to_radians());
    let towards = [az.sin() * el.cos(), az.cos() * el.cos(), el.sin()];
    let sun = Sun {
        update: Some(SunUpdate {
            direction: towards.map(|v| F32::new(-v)),
            color: [0.6, 0.6, 0.6, 1.0].map(F32::new),
            ambient: [0.3, 0.3, 0.3, 1.0].map(F32::new),
        }),
    };
    w.ghosts
        .insert(2, ghost(TgeClass::Sun, TgeRecord::Sun(sun)));
    w
}

#[test]
fn a_tge_sun_gives_back_the_angles_its_direction_was_made_from() {
    let w = tge_world([800.0, 400.0], 135.0, 30.0);
    let sun = SceneSource::sun(&w).unwrap();
    assert!((sun.azimuth - 135.0).abs() < 1e-3, "{}", sun.azimuth);
    assert!((sun.elevation - 30.0).abs() < 1e-3, "{}", sun.elevation);
    assert_eq!(sun.direct, [0.6; 3]);
    assert_eq!(sun.ambient, [0.3; 3]);
    assert_eq!(sun.shadow, [0.0; 3]);
    assert_eq!(angles_of([0.0, 1.0, 0.0]), (0.0, 0.0));
    let (az, el) = angles_of([-1.0, 0.0, 0.0]);
    assert!((az - 270.0).abs() < 1e-3 && el.abs() < 1e-6, "{az} {el}");
}

#[test]
fn a_tge_sky_with_its_textures_off_is_a_solid_sky_at_its_distances() {
    let w = tge_world([800.0, 400.0], 0.0, 45.0);
    let sky = SceneSource::sky(&w).unwrap();
    assert_eq!(sky.distances, Some([800.0, 400.0]));
    let main = sky.main.unwrap();
    assert!(main.material.is_empty());
    assert_eq!(main.sky_color, [0.2, 0.4, 0.8]);
    assert_eq!(main.fog_color, [0.5, 0.6, 0.7]);
    assert_eq!(main.fog_bytes, [127, 153, 178]);
    assert!(crate::sky::solid(&w, &crate::sky::SkyProfile::TGE));
    assert!(SceneSource::sun_flare(&w).is_none() && !w.has_map());
    let empty = ClientWorld::<TgeClientSet>::default();
    assert!(SceneSource::sky(&empty).is_none() && SceneSource::sun(&empty).is_none());
}
