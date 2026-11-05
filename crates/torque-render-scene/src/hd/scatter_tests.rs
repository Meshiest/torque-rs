use super::*;

fn at(elevation: f32, look: &ScatterLook) -> SkyLight {
    let e = elevation.to_radians();
    ScatterTable::earth().light([0.0, e.cos(), e.sin()], look)
}

fn lum(c: [f32; 3]) -> f32 {
    luminance(c.map(srgb_to_linear))
}

#[test]
fn noon_is_the_default_daylight_and_the_sun_reddens_as_it_sets() {
    let look = ScatterLook::default();
    let noon = at(90.0, &look);
    let lin = |l: &SkyLight| {
        [0, 1, 2]
            .map(|k| srgb_to_linear(l.ambient[k] + l.diffuse[k]) - srgb_to_linear(l.ambient[k]))
    };
    let n = lin(&noon);
    assert!((n[1] - NOON_SUN[1] * HIGH_SUN_SHARE).abs() < 0.02, "{n:?}");
    assert!((luminance(noon.ambient.map(srgb_to_linear)) - NOON_AMBIENT).abs() < 0.01);
    // The ambient is the sky's blue by day.
    assert!(noon.ambient[2] > noon.ambient[0], "{noon:?}");
    let low = at(4.0, &look);
    let l = lin(&low);
    assert!(l[0] > 1.5 * l[2], "a low sun is red: {l:?}");
    // Against the noon sun before the high sun's cut.
    assert!(luminance(l) < 0.6 * luminance(n) / HIGH_SUN_SHARE);
    // Under the horizon the sun's light is gone and the fog darkens.
    let dusk = at(-4.0, &look);
    assert_eq!(dusk.night, smoothstep(SUN_GONE, MOON_FULL, -4.0));
    assert!(lum(dusk.fog) < 0.3 * lum(noon.fog));
    // The fog at noon has NOON_FOG's brightness.
    assert!((lum(noon.fog) - luminance(NOON_FOG.map(srgb_to_linear))).abs() < 0.01);
}

#[test]
fn night_takes_the_moon_and_keeps_the_night_color() {
    let look = ScatterLook {
        night_color: [0.1, 0.12, 0.16],
        night_fog_color: [0.05, 0.06, 0.08],
        ..ScatterLook::default()
    };
    let night = at(-30.0, &look);
    assert_eq!(night.night, 1.0);
    // The light comes from the moon (azimuth 0, elevation 45).
    let s = std::f32::consts::FRAC_1_SQRT_2;
    assert!((night.towards[1] - s).abs() < 1e-4 && (night.towards[2] - s).abs() < 1e-4);
    assert!(night.diffuse.iter().all(|v| *v > 0.0));
    for k in 0..3 {
        assert!(night.ambient[k] >= look.night_color[k] - 1e-4);
        assert!(night.fog[k] >= look.night_fog_color[k] - 1e-4);
    }
    // No moon: no light, the sun's direction kept.
    let dark = at(
        -30.0,
        &ScatterLook {
            moon_enabled: false,
            ..look
        },
    );
    assert_eq!(dark.night, 0.0);
    assert_eq!(dark.diffuse, [0.0; 3]);
    assert!(dark.towards[2] < 0.0);
}

#[test]
fn the_light_changes_smoothly_through_the_day() {
    let look = ScatterLook::default();
    let mut last = at(-20.0, &look);
    let mut e = -20.0;
    while e < 90.0 {
        e += 0.25;
        let l = at(e, &look);
        for k in 0..3 {
            assert!(
                (l.ambient[k] - last.ambient[k]).abs() < 0.03,
                "{e}: {l:?} {last:?}"
            );
            assert!(
                (l.diffuse[k] - last.diffuse[k]).abs() < 0.08,
                "{e}: {l:?} {last:?}"
            );
            assert!((l.fog[k] - last.fog[k]).abs() < 0.06, "{e}: {l:?} {last:?}");
        }
        last = l;
    }
}
