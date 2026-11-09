use super::*;
use ghost_world::ClientSet;
use ghosts::shapebase::{Damage, Mount, ShapeBody};
use ghosts::shapes::Camera;

fn words() -> PlayerWords {
    PlayerWords {
        energy: 60.0,
        flag: false,
        free: Some(FreeMotion {
            position: [1.0, 2.0, 3.0],
            velocity: [0.5, -0.25, 0.0],
            contact: 7,
        }),
        head_x: 0.125,
        head_z: -0.5,
        rot_z: 1.5,
        translation: [1.0, 2.0, 3.0],
        frame: TailFrame::Absent,
    }
}

#[test]
fn a_players_frame_reads_back_with_its_words() {
    for frame in [
        TailFrame::World,
        TailFrame::Parent {
            ghost: 41,
            node: Some(3),
        },
    ] {
        let words = PlayerWords { frame, ..words() };
        let data = TgeClientSet::player_data(&words, None);
        let read = TgeClientSet::read_player(data.as_ref()).expect("a Player's");
        assert_eq!(read.words, words);
    }
}

#[test]
fn a_players_packet_data_reads_back_as_its_words() {
    let data = TgeClientSet::player_data(&words(), None);
    let read = TgeClientSet::read_player(data.as_ref()).expect("a Player's");
    assert_eq!(read.words, words());
    assert!(read.controlled.is_none());
    // The tail of a mismatching control section reads the same words.
    let tail = TgeClientSet::tail(data.as_ref());
    assert_eq!(tail.player(), Some(words()));
    // A plain ShapeBase's is not a Player's.
    let shape = TgePacketData::new(TailData::Shape(ShapeTail::default()));
    assert!(TgeClientSet::read_player(&shape).is_none());
    assert!(TgeClientSet::tail(&shape).player().is_none());
}

#[test]
fn a_players_control_object_travels_with_its_data() {
    let vehicle = VehicleWords {
        energy: 0.0,
        steering: [0.1, -0.2],
        position: [4.0, 5.0, 6.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        lin_mom: [1.0, 0.0, 0.0],
        ang_mom: [0.0, 0.0, 0.5],
        at_rest: false,
        contacts_zero: true,
        disable_move: false,
        kind: KindWords::Wheeled {
            brake: true,
            wheels: vec![
                WheelWords {
                    avel: 2.0,
                    dy: 0.1,
                    dx: 0.2,
                    slipping: true,
                };
                4
            ],
        },
    };
    let nested = TgeClientSet::vehicle_data(&vehicle, [4.0, 5.0, 6.0]);
    assert_eq!(
        TgeClientSet::read_vehicle(nested.as_ref()),
        Some(vehicle.clone())
    );
    let data = TgeClientSet::player_data(&words(), Some((12, nested)));
    let read = TgeClientSet::read_player(data.as_ref()).expect("a Player's");
    let (index, inner) = read.controlled.expect("its control object");
    assert_eq!(index, 12);
    assert_eq!(TgeClientSet::read_vehicle(inner), Some(vehicle));
    assert_eq!(inner.position(), [4.0, 5.0, 6.0]);
}

#[test]
fn a_flying_vehicles_data_has_no_wheels() {
    let w = VehicleWords {
        energy: 10.0,
        steering: [0.0; 2],
        position: [0.0; 3],
        rotation: [0.0, 0.0, 0.0, 1.0],
        lin_mom: [0.0; 3],
        ang_mom: [0.0; 3],
        at_rest: true,
        contacts_zero: false,
        disable_move: true,
        kind: KindWords::Flying,
    };
    let data = TgeClientSet::vehicle_data(&w, [0.0; 3]);
    assert_eq!(TgeClientSet::read_vehicle(data.as_ref()), Some(w));
    assert!(TgeClientSet::read_player(data.as_ref()).is_none());
}

#[test]
fn the_classes_sort_as_the_prediction_sorts_them() {
    assert_eq!(
        TgeClientSet::class_kind(TgeClass::Player),
        ClassKind::Player
    );
    assert_eq!(
        TgeClientSet::class_kind(TgeClass::AiPlayer),
        ClassKind::AiPlayer
    );
    assert_eq!(
        TgeClientSet::class_kind(TgeClass::AiWheeledVehicle),
        ClassKind::WheeledVehicle
    );
    assert_eq!(
        TgeClientSet::class_kind(TgeClass::FlyingVehicle),
        ClassKind::FlyingVehicle
    );
    assert_eq!(
        TgeClientSet::class_kind(TgeClass::HoverVehicle),
        ClassKind::Other
    );
    assert_eq!(TgeClientSet::class_kind(TgeClass::Item), ClassKind::Other);
    assert_eq!(
        TgeClientSet::class_kind(TgeClass::StaticShape),
        ClassKind::StaticShape
    );
}

#[test]
fn a_records_shapebase_state_is_read() {
    let shape = TgeShapeBase {
        body: Some(ShapeBody {
            damage: Some(Damage {
                state: 1,
                ..Damage::default()
            }),
            mount: Some(Some(Mount { object: 9, node: 2 })),
            ..ShapeBody::default()
        }),
        ..TgeShapeBase::default()
    };
    let player = TgeRecord::Player(ghosts::player::Player {
        shape: shape.clone(),
        ..Default::default()
    });
    let want = ShapeState {
        damage: Some(1),
        mount: Some((9, 2)),
        controlled: false,
    };
    assert_eq!(player.shape_state(), Some(want));
    assert_eq!(player.player_state(), Some(want));
    assert!(player.vehicle().is_none());
    let camera = TgeRecord::Camera(Camera {
        shape,
        ..Default::default()
    });
    assert_eq!(camera.shape_state(), Some(want));
    assert!(camera.player_state().is_none());
    assert!(TgeRecord::Sky(Default::default()).shape_state().is_none());
}

#[test]
fn a_datablocks_fields_are_its_values() {
    let db = TgeDatablock {
        class: "PlayerData".to_string(),
        fields: vec![
            ("maxDamage".to_string(), 0, Value::F32(100.0)),
            ("shapeFile".to_string(), 0, Value::str("player.dts")),
        ],
    };
    assert_eq!(db.class_name(), "PlayerData");
    assert_eq!(db.value("maxDamage", 0), Some(&Value::F32(100.0)));
    assert_eq!(db.value("maxDamage", 1), None);
    assert_eq!(db.value("shapeFile", 0), Some(&Value::str("player.dts")));
}
