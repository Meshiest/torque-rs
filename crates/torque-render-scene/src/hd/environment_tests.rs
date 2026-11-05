use super::*;
use ghosts::F32;
use ghosts::Xf;
use ghosts::light::{ScatterSky, SpotLight, TimeOfDay as DayRecord};

struct NoFiles;

impl Assets for NoFiles {
    fn read(&self, _path: &str) -> Option<Vec<u8>> {
        None
    }
}

fn ghost(class: TgeClass, record: TgeRecord) -> ghost_world::world::Ghost<TgeClientSet> {
    ghost_world::world::Ghost {
        class: Some(class),
        serial: 1,
        always: true,
        record: Some(record),
        part: (),
        position: None,
        updates: 0,
    }
}

fn update() -> SpotUpdate {
    SpotUpdate {
        color: [1.0, 0.5, 0.0].map(F32::new),
        brightness: F32::new(2.0),
        cast_shadows: true,
        range: F32::new(20.0),
        inner_angle: F32::new(30.0),
        outer_angle: F32::new(60.0),
    }
}

/// A transform at `p` turned a quarter about z (its y axis toward -x).
fn turned(p: [f32; 3]) -> Xf {
    let s = std::f32::consts::FRAC_1_SQRT_2;
    Xf {
        position: p.map(F32::new),
        quat: [0.0, 0.0, s].map(F32::new),
        negative_w: false,
    }
}

#[test]
fn a_spot_light_ghost_points_along_its_y_axis_with_half_its_angles() {
    let mut w = ClientWorld::<TgeClientSet>::default();
    let on = SpotLight {
        transform: Some(turned([1.0, 2.0, 3.0])),
        update: Some(update()),
        enabled: Some(true),
        mount: None,
    };
    let off = SpotLight {
        enabled: Some(false),
        ..on.clone()
    };
    w.ghosts
        .insert(4, ghost(TgeClass::SpotLight, TgeRecord::SpotLight(on)));
    w.ghosts
        .insert(5, ghost(TgeClass::SpotLight, TgeRecord::SpotLight(off)));
    let spots = spot_lights(&w, &NoFiles, &mut Models::default(), &mut Vec::new());
    assert_eq!(spots.len(), 1, "the one switched off gives none");
    let s = spots[0];
    for k in 0..3 {
        assert!((s.light.position[k] - [1.0, 2.0, 3.0][k]).abs() < 1e-5);
    }
    let d = s.direction;
    assert!(
        d[0].abs() > 0.999 && d[1].abs() < 1e-5,
        "a quarter turn: {d:?}"
    );
    assert!((s.inner - 15f32.to_radians()).abs() < 1e-6);
    assert!((s.outer - 30f32.to_radians()).abs() < 1e-6);
    assert_eq!(s.light.radius, 20.0);
    assert!(s.shadows && s.light.carrier.is_none());
    // Brightness 2 at half the range: the red channel the falloff's inverse at 10 units, twice.
    assert!((s.light.color[0] - 2.0 * spot_gain(20.0)).abs() < 1e-3);
    assert_eq!(s.light.color[2], 0.0);
}

#[test]
fn the_client_runs_the_day_from_the_orbit_it_was_sent() {
    let mut w = ClientWorld::<TgeClientSet>::default();
    let orbit = TimeOrbit {
        start_time: F32::new(0.0),
        day_length: F32::new(100.0),
        time: F32::new(0.2),
        axis_tilt: F32::new(23.44),
        azimuth_override: F32::new(1.0),
        day_scale: F32::new(1.0),
        night_scale: F32::new(1.0),
        play: true,
    };
    let rec = DayRecord {
        orbit: Some(orbit),
        animate: None,
    };
    w.ghosts
        .insert(3, ghost(TgeClass::TimeOfDay, TgeRecord::TimeOfDay(rec)));
    let mut day = ClientDay::default();
    day.update(&w, 0.0);
    assert!((day.day.as_ref().unwrap().time - 0.2).abs() < 1e-6);
    // Ten seconds of a hundred-second day.
    for _ in 0..10 {
        day.update(&w, 1.0);
    }
    assert!((day.day.as_ref().unwrap().time - 0.3).abs() < 1e-4);
    // A new orbit starts it again from what was sent.
    let orbit = TimeOrbit {
        time: F32::new(0.6),
        play: false,
        ..orbit
    };
    w.ghosts.insert(
        3,
        ghost(
            TgeClass::TimeOfDay,
            TgeRecord::TimeOfDay(DayRecord {
                orbit: Some(orbit),
                animate: None,
            }),
        ),
    );
    day.update(&w, 5.0);
    assert!(
        (day.day.as_ref().unwrap().time - 0.6).abs() < 1e-6,
        "stopped"
    );
    // Its light lights a frame: past sunset the sky is made for the sun under the horizon.
    let mut scene = HdScene::new(());
    let look = ScatterLook::default();
    assert!(day.apply(&mut scene, &look));
    let sun = scene.atmosphere_sun.unwrap();
    assert!(sun[2] < 0.0);
    assert_eq!(scene.fog.color, scene.clear);
}

#[test]
fn a_scatter_sky_ghost_gives_its_look() {
    let mut w = ClientWorld::<TgeClientSet>::default();
    assert_eq!(scatter_look(&w), None);
    let rec = ScatterSky {
        update: Some(ScatterUpdate {
            brightness: F32::new(0.8),
            moon_enabled: true,
            moon_elevation: F32::new(30.0),
            moon_light_color: [1.0; 3].map(F32::new),
            ..ScatterUpdate::default()
        }),
    };
    w.ghosts
        .insert(6, ghost(TgeClass::ScatterSky, TgeRecord::ScatterSky(rec)));
    let look = scatter_look(&w).unwrap();
    assert_eq!(look.brightness, 0.8);
    assert_eq!(look.moon_elevation, 30.0);
    assert_eq!(look.moon_light_color, [1.0; 3]);
}
