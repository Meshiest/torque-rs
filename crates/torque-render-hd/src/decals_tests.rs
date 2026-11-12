use super::*;
use torque_render_scene::hd::kit::look_at;

/// A sphere holding the eye covers the screen; one in front covers its projected disk, a quarter as much at twice the
/// distance; none covers more than the screen.
#[test]
fn a_spheres_screen_area_is_its_projected_disk_held_to_the_screen() {
    let camera = HdCamera::new(
        look_at([0.0; 3], [0.0, 1.0, 0.0]),
        90.0f32,
        500.0,
        1920,
        1080,
    );
    let screen = 1920.0 * 1080.0;
    assert_eq!(screen_area(&camera, [0.0, 0.5, 0.0], 1.0), screen);
    let near = screen_area(&camera, [0.0, 10.0, 0.0], 0.5);
    let far = screen_area(&camera, [0.0, 20.0, 0.0], 0.5);
    assert!(near > 100.0 && near < screen, "{near}");
    assert!((near / far - 4.0).abs() < 1e-3, "{near} {far}");
    assert_eq!(screen_area(&camera, [0.0, 2.0, 0.0], 1.9), screen);
}

/// A box's bytes are `DecalBox`'s members in order, the material word last.
#[test]
fn a_boxs_bytes_are_the_shaders_order() {
    let d = HdDecal {
        center: [1.0, 2.0, 3.0],
        half_x: [4.0, 5.0, 6.0],
        half_y: [7.0, 8.0, 9.0],
        half_z: [10.0, 11.0, 12.0],
        texture: (
            0,
            torque_render_scene::Sampler {
                repeat: false,
                mag: Filter::Linear,
                min: Filter::Linear,
                mip: Filter::Linear,
                mipmaps: true,
            },
        ),
        blend: Blend::Modulate2x,
        opacity: 0.5,
    };
    let b = box_bytes(&d);
    let f = |k: usize| f32::from_le_bytes(b[4 * k..4 * k + 4].try_into().expect("four bytes"));
    let words: Vec<f32> = (0..15).map(f).collect();
    assert_eq!(
        words,
        [
            1.0, 2.0, 3.0, 0.5, 4.0, 5.0, 6.0, 0.0, 7.0, 8.0, 9.0, 0.0, 10.0, 11.0, 12.0
        ]
    );
    assert_eq!(
        u32::from_le_bytes(b[60..64].try_into().expect("four bytes")),
        decal_material()
    );
}
