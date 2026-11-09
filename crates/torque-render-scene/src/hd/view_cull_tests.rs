use super::*;

fn unit(v: [f32; 3]) -> [f32; 3] {
    Vec3::from(v).normalize().to_array()
}

/// A frame looking along +y from the origin (90 degrees across, 16:9), its sun straight overhead, with no lights or water.
fn scene() -> HdScene<()> {
    let mut s = HdScene::new(());
    s.size = (1600, 900);
    s.visible_distance = 500.0;
    s.shadow_distance = 100.0;
    s.sun.towards = [0.0, 0.0, 1.0];
    s
}

fn eye() -> (Eye, f32) {
    let e = Eye {
        position: [0.0, 0.0, 2.0],
        pitch: 0.0,
        yaw: 0.0,
        fov: None,
    };
    (e, 90.0)
}

fn cull(s: &HdScene<()>, capture: bool) -> ViewCull {
    ViewCull::new(s, &[eye()], capture, Some(s.shadow_distance)).expect("an eye")
}

#[test]
fn a_shape_in_view_is_kept_and_one_behind_the_eye_is_left_out() {
    let c = cull(&scene(), false);
    assert!(c.keeps([0.0, 20.0, 2.0], 1.0));
    assert!(c.keeps([18.0, 20.0, 2.0], 1.0), "near the edge of the view");
    assert!(!c.keeps([0.0, -20.0, 2.0], 1.0));
    assert!(
        !c.keeps([0.0, 600.0, 2.0], 1.0),
        "past the visible distance"
    );
}

#[test]
fn a_shape_out_of_view_whose_shadow_falls_in_view_is_kept() {
    let mut s = scene();
    // The sun low behind the eye: the shadow of a shape behind it reaches forward into the view.
    s.sun.towards = unit([0.0, -1.0, 0.3]);
    let c = cull(&s, false);
    assert!(c.keeps([0.0, -10.0, 4.0], 1.0));
    // With no sun shadows the same shape is left out.
    s.shadow_distance = 0.0;
    assert!(!cull(&s, false).keeps([0.0, -10.0, 4.0], 1.0));
}

#[test]
fn the_shadow_of_a_shape_past_the_shadows_reach_is_not_kept() {
    let mut s = scene();
    s.sun.towards = unit([0.0, -1.0, 0.3]);
    s.shadow_distance = 5.0;
    // Overhead of the eye's back, its shadow sweeping forward, but nothing past 5 units of the eye is shadowed: still kept
    // (the sweep passes within reach); far to the side behind the eye it is not.
    assert!(cull(&s, false).keeps([0.0, -3.0, 4.0], 0.5));
    assert!(!cull(&s, false).keeps([200.0, -50.0, 4.0], 0.5));
}

#[test]
fn the_shadows_reach_is_where_the_last_frames_range_ended_past_the_shadow_distance() {
    let mut s = scene();
    s.sun.towards = unit([0.0, -1.0, 0.3]);
    s.shadow_distance = 5.0;
    // Far behind the eye's back: its shadow sweeps forward, but only past 5 units of the eye.
    let behind = [0.0, -40.0, 4.0];
    assert!(!cull(&s, false).keeps(behind, 0.5));
    // A range fitted to far geometry ended at 64 (four times its start): the shadow falls within it.
    let pushed = ViewCull::new(&s, &[eye()], false, Some(64.0)).expect("an eye");
    assert!(pushed.keeps(behind, 0.5));
    // No range known yet: the shadows may reach the visible distance.
    let unknown = ViewCull::new(&s, &[eye()], false, None).expect("an eye");
    assert!(unknown.keeps(behind, 0.5));
}

#[test]
fn a_shape_seen_in_the_water_or_the_capture_or_in_a_lights_reach_is_kept() {
    let mut s = scene();
    s.shadow_distance = 0.0;
    // Looking 30 degrees down: a shape high in front is above the view, its reflection in the water below it within.
    let (mut down, fov) = eye();
    down.pitch = 30f32.to_radians();
    let above = [0.0, 60.0, 50.0];
    let culled = |s: &HdScene<()>| ViewCull::new(s, &[(down, fov)], false, None).expect("an eye");
    assert!(!culled(&s).keeps(above, 1.0), "above the view");
    let mut water = crate::hd::kit::box_object(0, [0.0, 30.0, 0.0], [50.0, 50.0, 0.0])
        .draws
        .remove(0);
    for v in &mut water.vertices {
        v.position[2] = 0.0;
    }
    s.water = vec![water];
    assert!(culled(&s).keeps(above, 1.0), "seen in the water");
    s.water.clear();
    let behind = [0.0, -30.0, 2.0];
    assert!(!cull(&s, false).keeps(behind, 1.0));
    assert!(
        cull(&s, true).keeps(behind, 1.0),
        "the capture sees every way"
    );
    s.lights.push(crate::hd::HdLight {
        position: [0.0, 5.0, 2.0],
        color: [1.0; 3],
        radius: 40.0,
        carrier: None,
    });
    assert!(
        cull(&s, false).keeps(behind, 1.0),
        "within a seen light's reach"
    );
}

#[test]
fn a_jump_or_a_sharp_turn_of_the_eye_is_a_cut_and_a_step_is_not() {
    let (last, _) = eye();
    let mut step = last;
    step.position[1] += 0.5;
    step.yaw += 0.05;
    assert!(!is_cut(&last, &step));
    let mut far = last;
    far.position[0] += 100.0;
    assert!(is_cut(&last, &far), "a teleport");
    let mut turned = last;
    turned.yaw += std::f32::consts::PI;
    assert!(is_cut(&last, &turned), "an orbit's jump to the other side");
    let mut bad = last;
    bad.position[2] = f32::NAN;
    assert!(is_cut(&last, &bad));
}
