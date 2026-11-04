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
        recoil: None,
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

/// A shape with `sightRear` and `sightFront` nodes at `rear` and `front`.
fn sighted(rear: [f32; 3], front: [f32; 3]) -> Shape {
    let b = [-1.0, -1.0, -1.0, 1.0, 1.0, 1.0];
    let mut model =
        shapes::synth::hull_shape(b, &[("detail32", b)], &["sightRear", "sightFront"], &[]);
    let s = &mut model.shape;
    let rear_node = node_named(s, b"sightRear").unwrap();
    let front_node = node_named(s, b"sightFront").unwrap();
    s.default_translations[rear_node] = rear;
    s.default_translations[front_node] = front;
    model.shape
}

#[test]
fn the_sights_are_held_on_the_eyes_line() {
    let shape = sighted([0.02, 0.1, 0.05], [0.02, 0.3, 0.15]);
    let (place, pitch) = sight_offset(&shape, 0.36).expect("both sights");
    let mut m = Mat::euler_x(pitch);
    m.set_column3(place);
    let rear = m.mul_p([0.02, 0.1, 0.05]);
    let front = m.mul_p([0.02, 0.3, 0.15]);
    for (got, want) in rear.iter().zip([0.0, 0.36, 0.0]) {
        assert!((got - want).abs() < 1e-5, "the rear sight at {rear:?}");
    }
    assert!(
        front[0].abs() < 1e-5 && front[2].abs() < 1e-5 && front[1] > rear[1],
        "{front:?}"
    );
    let plain = shapes::synth::hull_shape([-1.0; 6], &[], &[], &[]).shape;
    assert_eq!(sight_offset(&plain, 0.36), None, "no sights");
}

#[test]
fn a_hosts_motion_moves_an_image_before_the_eye_and_in_the_hand() {
    let shape = sighted([0.0; 3], [0.0, 1.0, 0.0]);
    let instance = ShapeInstance::new(&shape, &mut |_| true).unwrap();
    let mut eye_offset = Mat::IDENTITY;
    eye_offset.set_column3([0.3, 0.5, -0.2]);
    let db = ImageDb {
        shape_file: "pistol.dts".to_string(),
        mount_point: 0,
        offset: Mat::IDENTITY,
        eye_offset: Some(eye_offset),
    };
    let mut camera = Mat::IDENTITY;
    camera.set_column3([0.0, 0.0, 1.8]);
    let mut xform = Mat::IDENTITY;
    xform.set_column3([5.0, 0.0, 0.0]);
    let owner = Owner {
        kind: ShapeKind::Player,
        shape: &shape,
        instance: &instance,
        xform,
        scale: [1.0; 3],
        first_person: true,
        camera,
        scaled_eye_offset: false,
        scaled_bare_mount: false,
        motion: HeldMotion::default(),
    };
    let near = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5);
    // TGE's: at the eye times `eyeOffset`, and in the hand at the mount.
    assert!(near(
        owner.image_transform(&db, &shape).column(3),
        [0.3, 0.5, 1.6]
    ));
    assert_eq!(owner.held_transform(&db, &shape).column(3), [5.0, 0.0, 0.0]);
    let mut eye = Mat::IDENTITY;
    eye.set_column3([0.0, 0.4, 0.0]);
    let moved = Owner {
        motion: HeldMotion {
            eye: Some(eye),
            hand: Some(Mat::euler_x(0.3)),
        },
        ..owner
    };
    assert!(near(
        moved.image_transform(&db, &shape).column(3),
        [0.0, 0.4, 1.8]
    ));
    let hand = moved.held_transform(&db, &shape);
    assert_eq!(hand.column(3), [5.0, 0.0, 0.0]);
    assert_eq!(hand.m, Mat::mul(&xform, &Mat::euler_x(0.3)).m);
    // An image with no eye offset is held in the hand in the first person too.
    let bare = ImageDb {
        eye_offset: None,
        ..db
    };
    assert_eq!(moved.image_transform(&bare, &shape).m, hand.m);
}
