use super::*;
use crate::Filter;

fn decal() -> HdDecal {
    HdDecal {
        center: [1.0, 2.0, 3.0],
        half_x: [0.5, 0.0, 0.0],
        half_y: [0.0, 0.5, 0.0],
        half_z: [0.0, 0.0, 0.5],
        texture: (
            0,
            Sampler {
                repeat: false,
                mag: Filter::Linear,
                min: Filter::Linear,
                mip: Filter::Linear,
                mipmaps: true,
            },
        ),
        blend: Blend::Modulate2x,
        opacity: 1.0,
    }
}

/// A box with a value off the numbers, an axis of no length or one past the size cap is left out; the opacity is held.
#[test]
fn only_a_box_of_finite_capped_axes_is_drawn() {
    assert_eq!(decal().sanitized(), Some(decal()));
    let off = [
        HdDecal {
            center: [f32::NAN, 0.0, 0.0],
            ..decal()
        },
        HdDecal {
            half_x: [f32::INFINITY, 0.0, 0.0],
            ..decal()
        },
        HdDecal {
            half_y: [0.0; 3],
            ..decal()
        },
        HdDecal {
            half_z: [0.0, 0.0, 0.5 * MAX_DECAL_SIZE + 1.0],
            ..decal()
        },
        HdDecal {
            opacity: 0.0,
            ..decal()
        },
        HdDecal {
            opacity: f32::NAN,
            ..decal()
        },
    ];
    for d in off {
        assert_eq!(d.sanitized(), None, "{d:?}");
    }
    let held = HdDecal {
        opacity: 7.0,
        ..decal()
    };
    assert_eq!(held.sanitized().map(|d| d.opacity), Some(1.0));
}

/// The facing share is nothing at or past the CPU path's cut and whole a fade's width before it.
#[test]
fn the_facing_share_starts_at_the_cpu_paths_cut() {
    assert_eq!(HdDecal::facing_share(0.0), 0.0);
    assert_eq!(HdDecal::facing_share(CLIP_ANGLE_COS), 0.0);
    assert_eq!(HdDecal::facing_share(CLIP_ANGLE_COS + FACING_FADE), 1.0);
    assert_eq!(HdDecal::facing_share(1.0), 1.0);
    let mid = HdDecal::facing_share(CLIP_ANGLE_COS + 0.5 * FACING_FADE);
    assert!((mid - 0.5).abs() < 1e-6);
}
