use super::*;
use crate::{Blend, Cull};

fn draw(label: &str) -> HdDraw {
    HdDraw {
        label: label.to_string(),
        vertices: Vec::new(),
        indices: Vec::new(),
        texture: None,
        blend: Blend::Opaque,
        cull: Cull::None,
        depth_test: false,
        depth_write: false,
    }
}

/// `auto` draws the physical sky in the box's place only when the caller says the box is the built-in default; `atmosphere` and `server` whatever the box.
#[test]
fn auto_picks_the_atmosphere_only_when_the_caller_says_the_box_is_the_default() {
    assert!(SkyChoice::Auto.atmosphere(true));
    assert!(!SkyChoice::Auto.atmosphere(false));
    assert!(SkyChoice::Atmosphere.atmosphere(false));
    assert!(!SkyChoice::Server.atmosphere(true));
    for c in SkyChoice::ALL {
        assert_eq!(SkyChoice::named(c.name()), Some(c));
    }
    assert_eq!(
        SkyChoice::named(" ATMOSPHERE "),
        Some(SkyChoice::Atmosphere)
    );
    assert_eq!(SkyChoice::named("box"), None);
    assert_eq!(SkyChoice::default(), SkyChoice::Auto);
}

#[test]
fn the_shading_follows_the_box_and_the_disk_goes_before_the_clouds() {
    let sky = [
        draw("sky face 0"),
        draw("sky face 4"),
        draw("remote flare"),
        draw("sky cloud layer 0"),
        draw("sky side bans"),
    ];
    let at = |shade, disk, clouds| SkyOrder {
        shade,
        disk,
        clouds,
    };
    assert_eq!(order(&sky), at(2, 3, 4));
    assert_eq!(order(&sky[..4]), at(2, 3, 4));
    assert_eq!(order(&sky[..2]), at(2, 2, 2));
    assert_eq!(order(&[]), at(0, 0, 0));
    assert_eq!(SkyPart::of("sky skirt"), SkyPart::Bans);
}

fn towards(az: f32, el: f32) -> [f32; 3] {
    let (az, el) = (az.to_radians(), el.to_radians());
    [az.sin() * el.cos(), az.cos() * el.cos(), el.sin()]
}

/// A box of blue with a white band at the horizon and white suns (direction, angular radius in degrees).
fn sky_of(suns: Vec<([f32; 3], f32)>) -> impl Fn([f32; 3], u32) -> [f32; 3] {
    move |d: [f32; 3], _| {
        let white = d[2] < 0.05
            || suns
                .iter()
                .any(|(s, r)| dot3(d, *s).clamp(-1.0, 1.0).acos() <= r.to_radians());
        if white { [1.0; 3] } else { [0.4, 0.6, 0.9] }
    }
}

#[test]
fn a_sun_painted_off_the_suns_direction_is_found_and_drawn_there() {
    let sun = towards(250.0, 35.0);
    let painted = towards(250.0, 22.3);
    let p = painted_sun(&sky_of(vec![(painted, 3.0)]), sun, PAINTED_SUN).expect("the painted sun");
    assert!((p.offset.to_degrees() - 12.7).abs() < 0.3, "{p:?}");
    assert!((p.radius.to_degrees() - 3.0).abs() < 0.5, "{p:?}");
    let d = p.drawn().expect("drawn on the painted sun");
    assert!(
        dot3(d, painted).clamp(-1.0, 1.0).acos().to_degrees() < 0.2,
        "{p:?}"
    );
}

#[test]
fn a_painted_sun_over_the_suns_direction_keeps_it() {
    let sun = towards(238.0, 21.0);
    let p = painted_sun(&sky_of(vec![(towards(238.0, 20.0), 2.0)]), sun, PAINTED_SUN)
        .expect("the painted sun");
    assert_eq!(p.drawn(), None, "{p:?}");
}

#[test]
fn no_sun_in_a_plain_or_white_box_nor_in_a_speck() {
    let sun = towards(250.0, 35.0);
    assert_eq!(painted_sun(&sky_of(Vec::new()), sun, PAINTED_SUN), None);
    let white = |_: [f32; 3], _: u32| [1.0f32; 3];
    assert_eq!(painted_sun(&white, sun, PAINTED_SUN), None);
    let speck = sky_of(vec![(towards(250.0, 25.0), 0.4)]);
    assert_eq!(painted_sun(&speck, sun, PAINTED_SUN), None);
    // A low sun's painted sun merged with the horizon's white band is not one.
    let low = towards(200.0, 5.0);
    assert_eq!(
        painted_sun(&sky_of(vec![(towards(200.0, 3.0), 3.0)]), low, PAINTED_SUN),
        None
    );
}
