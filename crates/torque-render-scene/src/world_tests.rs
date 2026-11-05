use super::*;
use ghost_world::tge::TgeDatablock;
use ghosts::F32;
use ghosts::vehicle::{VehicleState, WheelSetup as SentWheel, WheeledVehicle};

/// A ghost of TGE's set with `record`.
pub(crate) fn ghost(class: TgeClass, record: TgeRecord) -> Ghost<TgeClientSet> {
    Ghost {
        class: Some(class),
        serial: 1,
        always: false,
        record: Some(record),
        part: (),
        position: None,
        updates: 0,
    }
}

/// A datablock of `class` with `fields` (each element 0).
pub(crate) fn datablock(class: &str, fields: &[(&str, Value)]) -> TgeDatablock {
    TgeDatablock {
        class: class.to_string(),
        fields: fields
            .iter()
            .map(|(n, v)| (n.to_string(), 0, v.clone()))
            .collect(),
    }
}

#[test]
fn tges_classes_draw_as_their_kinds_in_the_frames_order() {
    let w = ClientWorld::<TgeClientSet>::default();
    assert_eq!(w.kind(TgeClass::AiPlayer), ShapeKind::Player);
    assert_eq!(w.kind(TgeClass::HoverVehicle), ShapeKind::Vehicle);
    assert_eq!(w.kind(TgeClass::Sky), ShapeKind::None);
    let classes = w.shape_classes();
    assert_eq!(classes.first(), Some(&TgeClass::StaticShape));
    assert_eq!(classes.last(), Some(&TgeClass::Projectile));
    assert!(classes.iter().all(|c| w.kind(*c) != ShapeKind::None));
}

#[test]
fn a_datablocks_fields_and_class_are_read_through_the_world() {
    let mut w = ClientWorld::<TgeClientSet>::default();
    w.datablocks.insert(
        5,
        datablock(
            "ShapeBaseImageData",
            &[
                ("shapeFile", Value::str("demo/data/shapes/crossbow.dts")),
                ("mountPoint", Value::Int(0)),
            ],
        ),
    );
    w.datablocks
        .insert(6, datablock("ItemData", &[("elasticity", Value::F32(0.5))]));
    assert!(w.is_image(5) && !w.is_image(6) && !w.is_image(7));
    assert_eq!(
        w.text(5, "shapeFile"),
        Some(&b"demo/data/shapes/crossbow.dts"[..])
    );
    assert_eq!(w.float(6, "elasticity", 0), Some(0.5));
    assert_eq!(w.float(5, "shapeFile", 0), None);
}

#[test]
fn a_wheeled_vehicles_steering_is_unpacked_against_its_datablocks_angle() {
    let mut w = ClientWorld::<TgeClientSet>::default();
    w.datablocks.insert(
        9,
        datablock(
            "WheeledVehicleData",
            &[("maxSteeringAngle", Value::F32(0.5))],
        ),
    );
    let mut r = WheeledVehicle::default();
    r.vehicle.shape.game.datablock = Some(9);
    r.vehicle.state = Some(VehicleState {
        steering: [511, 0],
        ..Default::default()
    });
    r.setup = Some(vec![
        Some(SentWheel {
            tire: 20,
            spring: 21,
            powered: true,
            steering: F32::new(1.0),
        }),
        None,
    ]);
    let g = ghost(TgeClass::WheeledVehicle, TgeRecord::WheeledVehicle(r));
    assert_eq!(w.kind_of(&g), ShapeKind::WheeledVehicle);
    let wheels = w.wheels(&g).unwrap();
    assert!((wheels.steering - 0.5).abs() < 1e-6, "{}", wheels.steering);
    assert_eq!(
        wheels.wheels,
        vec![
            Some(WheelSetup {
                tire: 20,
                spring: 21,
                steering: 1.0
            }),
            None
        ]
    );
    // TGE draws no tint and hides nothing; the mount scale is the object's own.
    assert!(w.mesh_colors(&g).is_none() && w.hidden_nodes(&g).is_none());
    assert_eq!(w.mount_scale(&g, [2.0; 3]), [2.0; 3]);
    // The steering thread: full right (0.5 rad of 0.5) at 0.5 - (0.5 * 0.5 / 0.5) / 2; the client's own
    // steering for a vehicle it runs.
    w.ghosts.insert(4, g);
    let place = crate::world::steering_place(&w, 4).unwrap();
    assert!((place - 0.25).abs() < 1e-5, "{place}");
    w.vehicle_look.insert(
        4,
        ghost_world::world::VehicleLook {
            steering: -0.25,
            velocity: [0.0; 3],
            wheels: Vec::new(),
        },
    );
    let place = crate::world::steering_place(&w, 4).unwrap();
    assert!((place - (0.5 + 0.0625 / 0.5 * 0.5)).abs() < 1e-5, "{place}");
    assert_eq!(crate::world::steering_place(&w, 5), None);
}

/// TGE's Player at rest: the arm (`look`), `head` and `headside` threads held at the head pitch, then the action thread on `root`.
#[test]
fn a_player_at_rest_holds_its_look_threads_at_its_pitch_over_the_root() {
    let b = [-1.0, -1.0, -1.0, 1.0, 1.0, 1.0];
    let model = shapes::synth::hull_shape(
        b,
        &[("detail32", b)],
        &[],
        &["look", "head", "headside", "root"],
    );
    let shape = &model.shape;
    let mut charge = |_: u64| true;
    let mut instance = ShapeInstance::new(shape, &mut charge).unwrap();
    crate::player::rest_threads(shape, &mut instance, 0.0, 0.0);
    assert_eq!(instance.thread_count(), 4);
    let at = |k: usize| {
        let t = instance.thread(shapes::instance::ThreadId(k)).unwrap();
        (t.sequence, t.get_pos(), t.time_scale)
    };
    assert_eq!(at(0), (Some(0), 0.5, 0.0));
    assert_eq!(at(1), (Some(1), 0.5, 0.0));
    assert_eq!(at(2), (Some(2), 0.5, 0.0));
    // (The synthetic shape names its root node `root` too: only the action thread's start is checked.)
    assert_eq!(at(3).1, 0.0);
    // Looking all the way up: the look threads at their end.
    let mut up = ShapeInstance::new(shape, &mut charge).unwrap();
    crate::player::rest_threads(shape, &mut up, 1.5, 0.0);
    assert!(up.thread(shapes::instance::ThreadId(0)).unwrap().get_pos() > 0.999);
    // Half the side angle to the left: `headside` at a quarter.
    let mut side = ShapeInstance::new(shape, &mut charge).unwrap();
    crate::player::rest_threads(shape, &mut side, 0.0, -0.5);
    assert_eq!(
        side.thread(shapes::instance::ThreadId(2))
            .unwrap()
            .get_pos(),
        0.25
    );
}

/// TGE's world poses a Player its record places by the record's head (its fractions of `maxLookAngle`, 1.4 by default).
#[test]
fn tges_player_at_rest_looks_by_its_records_head() {
    let b = [-1.0, -1.0, -1.0, 1.0, 1.0, 1.0];
    let model = shapes::synth::hull_shape(
        b,
        &[("detail32", b)],
        &[],
        &["look", "head", "headside", "root"],
    );
    let shape = &model.shape;
    let mut w = ClientWorld::<TgeClientSet>::default();
    // Head x all the way up (63 of 6 bits: +1), head z all the way to one side (0: -1).
    let record = TgeRecord::Player(ghosts::player::Player {
        motion: Some(ghosts::player::PlayerMove {
            head: [63, 0],
            ..Default::default()
        }),
        ..Default::default()
    });
    let mut g = ghost(TgeClass::Player, record);
    g.position = Some([0.0; 3]);
    let pose = g.pose();
    assert!(matches!(pose, Some(Pose::Euler { pitch, .. }) if pitch == 0.0));
    w.ghosts.insert(3, g);
    let mut charge = |_: u64| true;
    let mut instance = ShapeInstance::new(shape, &mut charge).unwrap();
    w.pose_at_rest(3, pose, shape, &mut instance, 0, &mut Vec::new());
    let pos = |k: usize| {
        instance
            .thread(shapes::instance::ThreadId(k))
            .unwrap()
            .get_pos()
    };
    // 1.4 rad over the -80..+80 degree range: (1.4 + 1.396) / 2.793.
    assert!(pos(0) > 0.999 && pos(1) > 0.999, "{} {}", pos(0), pos(1));
    assert_eq!(pos(2), 0.0);
}

/// TGE scales neither a bare mount point's transform nor a vehicle's tyres.
#[test]
fn tges_world_scales_no_bare_mount_and_no_tyres() {
    let w = ClientWorld::<TgeClientSet>::default();
    assert!(!w.scaled_bare_mount() && !w.scaled_tires() && !w.scaled_eye_offset());
}

/// TGE's image emitters always emit, first-person view or not, and only node -1 is the muzzle.
#[test]
fn tges_image_emitters_emit_in_every_view_and_only_minus_one_is_the_muzzle() {
    let w = ClientWorld::<TgeClientSet>::default();
    assert!(w.image_emits(5, true) && w.image_emits(5, false));
    let b = [-1.0, -1.0, -1.0, 1.0, 1.0, 1.0];
    let model = shapes::synth::hull_shape(b, &[("detail32", b)], &[], &[]);
    assert_eq!(w.image_emitter_node(&model.shape, -1, 4), 4);
    for node in [0, 1, 3] {
        assert_eq!(w.image_emitter_node(&model.shape, node, 4), node);
    }
}
