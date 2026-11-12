use super::*;
use torque_render_scene::hd::distortion::{DistortionLook, MAX_DISTORTERS, MAX_VERTICES, sphere};

fn valid(src: &str) {
    let module =
        naga::front::wgsl::parse_str(src).unwrap_or_else(|e| panic!("{}", e.emit_to_string(src)));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .unwrap_or_else(|e| panic!("{e:?}"));
}

#[test]
fn the_shader_and_its_composite_module_are_valid() {
    valid(SOURCE);
    valid(&format!(
        "{}\n{SOURCE}",
        include_str!("../shaders/fullscreen.wgsl")
    ));
}

#[test]
fn a_looks_bytes_are_the_shaders_order() {
    let look = DistortionLook {
        strength: 0.5,
        edge: 2.0,
        map: None,
        map_scale: 3.0,
        scroll: [0.25, -0.5],
        tint: [0.1, 0.2, 0.3, 0.4],
        chroma: 0.6,
    };
    let b = look_bytes(&look, true);
    let f: Vec<f32> = b
        .chunks(4)
        .map(|c| f32::from_le_bytes(c.try_into().expect("four bytes")))
        .collect();
    assert_eq!(
        f,
        [0.5, 2.0, 3.0, 0.6, 0.25, -0.5, 0.0, 1.0, 0.1, 0.2, 0.3, 0.4]
    );
}

#[test]
fn only_the_distorters_in_sight_that_change_the_picture_are_drawn() {
    let look = DistortionLook {
        strength: 0.2,
        ..DistortionLook::default()
    };
    let near = HdDistorter::new(sphere([0.0, 0.0, 0.0], 1.0, 4, 6, 255), look);
    let far = HdDistorter::new(sphere([100.0, 0.0, 0.0], 1.0, 4, 6, 255), look);
    let plain = HdDistorter::new(
        sphere([0.0, 0.0, 0.0], 1.0, 4, 6, 255),
        DistortionLook::default(),
    );
    // The half space x <= 10.
    let planes = [[-1.0, 0.0, 0.0, 10.0]];
    assert_eq!(seen(&[near, far, plain], &planes, [0.0; 3]), vec![0]);
}

/// A server decides how many shapes bend: past the caps the nearest are drawn and the frame's bytes stay bounded.
#[test]
fn more_distorters_than_the_cap_draw_the_nearest_within_the_caps() {
    let look = DistortionLook {
        strength: 0.2,
        ..DistortionLook::default()
    };
    let ball = |x: f32| HdDistorter::new(sphere([x, 0.0, 0.0], 0.5, 4, 6, 255), look);
    // Shuffled distances, so the order kept is the distance's and not the list's.
    let many: Vec<HdDistorter> = (0..MAX_DISTORTERS + 100)
        .map(|i| ball(((i * 37) % (MAX_DISTORTERS + 100)) as f32 + 2.0))
        .collect();
    let all = [[0.0, 0.0, 0.0, 1.0]];
    let kept = seen(&many, &all, [0.0; 3]);
    assert_eq!(kept.len(), MAX_DISTORTERS);
    let farthest_kept = kept
        .iter()
        .map(|&i| many[i].centre[0])
        .fold(0.0f32, f32::max);
    let nearest_left = (0..many.len())
        .filter(|i| !kept.contains(i))
        .map(|i| many[i].centre[0])
        .fold(f32::INFINITY, f32::min);
    assert!(
        farthest_kept < nearest_left,
        "{farthest_kept} {nearest_left}"
    );
    // One distorter of more vertices than the frame may hold is left out; the smaller ones beyond it are kept.
    let rings = 1100;
    let huge = HdDistorter::new(sphere([1.0, 0.0, 0.0], 0.5, rings, 1000, 255), look);
    assert!(huge.draw.vertices.len() > MAX_VERTICES);
    let kept = seen(&[huge, ball(5.0), ball(9.0)], &all, [0.0; 3]);
    assert_eq!(kept, vec![1, 2]);
}
