//! The engine's own classes ([`crate::light`]): each record packed from a view, written and read back, and its bits.

use super::*;
use crate::light::{
    SCATTER_UPDATE_MASK, SPOT_ENABLED_MASK, SPOT_MOUNT_MASK, SPOT_UPDATE_MASK, ScatterSky,
    SpotLight, SpotMount, TIME_ANIMATE_MASK, TIME_ORBIT_MASK, TimeOfDay, pack_scatter_sky,
    pack_spot_light, pack_time_of_day,
};

#[test]
fn a_spot_light_packs_its_cone_its_switch_and_its_mount() {
    let t = table();
    let v = MapView::default()
        .with("color", 0, Value::F32(1.0))
        .with("color", 1, Value::F32(0.8))
        .with("color", 2, Value::F32(0.6))
        .with("brightness", 0, Value::F32(2.0))
        .with("castShadows", 0, Value::Bool(true))
        .with("range", 0, Value::F32(30.0))
        .with("innerAngle", 0, Value::F32(40.0))
        .with("outerAngle", 0, Value::F32(60.0))
        .with("isEnabled", 0, Value::Bool(false))
        .with("mount_node", 0, Value::Int(2));
    let v = super::tge_shapes::with_link(v, "mount_object", Link::Ghost(7));
    let rec = pack_spot_light(
        &v,
        SPOT_UPDATE_MASK | SPOT_ENABLED_MASK | SPOT_MOUNT_MASK,
        &pack_ctx(),
    );
    assert_eq!(rec.transform, None);
    let u = rec.update.expect("the update");
    assert_eq!(u.brightness.get(), 2.0);
    assert!(u.cast_shadows);
    assert_eq!(
        [u.range, u.inner_angle, u.outer_angle].map(|f| f.get()),
        [30.0, 40.0, 60.0]
    );
    assert_eq!(rec.enabled, Some(false));
    assert_eq!(rec.mount, Some(Some(SpotMount { object: 7, node: 2 })));
    assert_eq!(round_trip(&rec, &t), rec);
    // Flags, the color, brightness, the flag, three floats, the switch, the mount's ghost and node.
    let ghost = usize::from(widths().ghost_bits) + 1;
    assert_eq!(
        bit_total(&rec, &t),
        4 + 96 + 32 + 1 + 96 + 1 + 1 + ghost + 8
    );
    // Unmounted: the block's flag and a cleared one.
    let rec = pack_spot_light(&MapView::default(), SPOT_MOUNT_MASK, &pack_ctx());
    assert_eq!(rec.mount, Some(None));
    assert_eq!(round_trip(&rec, &t), rec);
    assert_eq!(bit_total(&SpotLight::default(), &t), 4);
}

#[test]
fn a_time_of_day_packs_its_orbit_and_its_animation() {
    let t = table();
    let v = MapView::default()
        .with("dayLength", 0, Value::F32(600.0))
        .with("time", 0, Value::F32(0.3))
        .with("play", 0, Value::Bool(true))
        .with("animate_time", 0, Value::F32(180.0))
        .with("animate_speed", 0, Value::F32(10.0));
    let rec = pack_time_of_day(&v, TIME_ORBIT_MASK | TIME_ANIMATE_MASK);
    let o = rec.orbit.expect("the orbit");
    assert_eq!(
        (o.day_length.get(), o.time.get(), o.play),
        (600.0, 0.3, true)
    );
    assert_eq!(rec.animate.map(|a| a.map(|f| f.get())), Some([180.0, 10.0]));
    assert_eq!(round_trip(&rec, &t), rec);
    assert_eq!(bit_total(&rec, &t), 2 + 7 * 32 + 1 + 64);
    assert_eq!(bit_total(&TimeOfDay::default(), &t), 2);
}

#[test]
fn a_scatter_sky_packs_its_look() {
    let t = table();
    let v = MapView::default()
        .with("moonEnabled", 0, Value::Bool(true))
        .with("moonElevation", 0, Value::F32(45.0))
        .with("nightColor", 2, Value::F32(0.2));
    let rec = pack_scatter_sky(&v, SCATTER_UPDATE_MASK);
    let u = rec.update.expect("the update");
    assert!(u.moon_enabled);
    assert_eq!(u.moon_elevation.get(), 45.0);
    assert_eq!(u.night_color[2].get(), 0.2);
    assert_eq!(round_trip(&rec, &t), rec);
    assert_eq!(bit_total(&ScatterSky::default(), &t), 1);
}
