use super::*;
use ghost_world::tge::TgeClientSet;

/// The live client's clock of one slot: each frame's times from the clock it kept.
fn live(
    w: &ClientWorld<TgeClientSet>,
    kept: &mut Option<ImageClock>,
    image: &MountedImage,
) -> ImageClock {
    let next = advance_clock(w, image, kept.as_ref());
    *kept = Some(next);
    next
}

fn held(anim_time: f32, entered: u32, ambient: f32) -> MountedImage {
    MountedImage {
        datablock: 70,
        skin: ghosts::NetStr::Null,
        wet: false,
        ammo: false,
        loaded: true,
        target: false,
        trigger: false,
        fire_count: 0,
        state: 2,
        delay: 0.0,
        at_ms: Some(0),
        ambient_time: ambient,
        spin_scale: 0.5,
        spin_time: 0.0,
        anim: Some(ghost_world::images::ImageAnim {
            sequence: 0,
            forward: true,
            over: None,
            time: anim_time,
            still: false,
            entered,
        }),
        cap_hits: 0,
        events: Vec::new(),
    }
}

#[test]
fn the_live_client_plays_an_images_threads_by_the_frames_time() {
    // The machine's times move in ticks as the updates come; the drawn ones by each frame's time.
    let mut w = ClientWorld::<TgeClientSet> {
        animate: true,
        anim_ms: 1_000,
        ..Default::default()
    };
    let mut kept = None;
    let first = live(&w, &mut kept, &held(0.0, 1, 0.5));
    assert_eq!(first.now.anim, 0.0);
    let mut last = first;
    // Five 16 ms frames, the machine still at 0 (no datagram), then two ticks at once: no jump, 16 ms a frame.
    for (i, machine) in [0.0, 0.0, 0.0, 0.0, 0.064].into_iter().enumerate() {
        w.anim_ms += 16;
        let c = live(&w, &mut kept, &held(machine, 1, 0.5 + machine));
        assert!(
            (c.now.anim - last.now.anim - 0.016).abs() < 1e-6,
            "frame {i}: {c:?}"
        );
        assert!(
            (c.now.spin - last.now.spin - 0.008).abs() < 1e-6,
            "the spin at its speed"
        );
        assert_eq!(c.prev, last.now, "the previous frame's times are kept");
        // Twice in a frame: the same.
        assert_eq!(live(&w, &mut kept, &held(machine, 1, 0.5 + machine)), c);
        last = c;
    }
    // A state sets the thread again: from the machine's time since.
    w.anim_ms += 16;
    let c = live(&w, &mut kept, &held(0.032, 2, 0.6));
    assert_eq!(c.now.anim, 0.032);
    // A new mount (the ambient time gone back) starts from the machine's times.
    w.anim_ms += 16;
    let c = live(&w, &mut kept, &held(0.0, 0, 0.0));
    assert_eq!(
        (c.now, c.prev),
        (ThreadTimes::of(&held(0.0, 0, 0.0)), c.now)
    );
    // A world that does not animate (a session played back) draws the machine's times.
    w.animate = false;
    w.anim_ms += 16;
    assert_eq!(live(&w, &mut kept, &held(0.25, 3, 1.0)).now.anim, 0.25);
}

#[test]
fn an_identity_wire_transform_is_the_identity_and_a_turn_is_a_turn() {
    let id = [0, 0, 0, 0, 0, 0, 0].map(|v: u32| v);
    assert_eq!(xform_matrix(&id), Mat::IDENTITY);
    // 90 degrees about z: (0, 0, sin 45, cos 45), at (1, 2, 3); TGE's `setMatrix` turns x to -y.
    let s = std::f32::consts::FRAC_1_SQRT_2;
    let w = [1.0f32, 2.0, 3.0, 0.0, 0.0, s].map(f32::to_bits);
    let m = xform_matrix(&[w[0], w[1], w[2], w[3], w[4], w[5], 0]);
    let p = m.mul_p([1.0, 0.0, 0.0]);
    assert!(
        (p[0] - 1.0).abs() < 1e-6 && (p[1] - 1.0).abs() < 1e-6 && (p[2] - 3.0).abs() < 1e-6,
        "{p:?}"
    );
}

#[test]
fn scale_columns_scales_the_basis_not_the_translation_row() {
    let mut m = Mat::IDENTITY;
    m.set_column3([5.0, 6.0, 7.0]);
    let r = scale_columns(&m, [2.0, 3.0, 4.0]);
    assert_eq!((r.m[0], r.m[5], r.m[10]), (2.0, 3.0, 4.0));
    assert_eq!(r.column(3), [5.0, 6.0, 7.0]);
}
