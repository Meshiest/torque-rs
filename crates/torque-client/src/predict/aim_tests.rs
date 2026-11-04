use super::*;

fn ctx(camera_pos: f32) -> Ctx {
    Ctx {
        camera_pos,
        prefs: GamePrefs::default(),
        drive: None,
        controls_player: false,
    }
}

fn turn(yaw: f32, pitch: f32, free_look: bool) -> sim::Move {
    sim::Move {
        yaw,
        pitch,
        free_look,
        ..sim::Move::default()
    }
}

#[test]
fn tge_turns_the_body_or_with_free_look_in_the_third_person_the_head() {
    let db = PlayerData::default();
    let mut s = PlayerState::new(collide::ObjectId(1), &db, [0.0; 3], 0.0);
    s.head = [0.0, 0.0, 0.2];
    let (yaw, head) = next(&s, &db, &turn(0.1, 0.05, false), &ctx(0.0));
    assert_eq!(
        (yaw, head[0], head[2]),
        (0.1, 0.05, 0.1),
        "the body turns, the head eases back"
    );
    let (yaw, head) = next(&s, &db, &turn(0.1, 0.0, true), &ctx(0.0));
    assert_eq!(
        (yaw, head[2]),
        (0.1, 0.1),
        "free look in the first person turns the body"
    );
    let (yaw, head) = next(&s, &db, &turn(0.1, 0.0, true), &ctx(1.0));
    assert_eq!(yaw, 0.0);
    assert!(
        (head[2] - 0.3).abs() < 1e-6,
        "free look in the third person turns the head"
    );
    s.control_object = Some(collide::ObjectId(9));
    let (yaw, head) = next(&s, &db, &turn(0.1, 0.05, false), &ctx(0.0));
    assert_eq!(
        (yaw, head[0]),
        (0.0, 0.0),
        "a control object takes the move"
    );
}

#[test]
fn tge_hands_the_control_move_without_free_look_only() {
    assert_eq!(mount_move(&turn(0.1, 0.2, false), 0.0), [0.1, 0.2]);
    assert_eq!(mount_move(&turn(0.1, 0.2, true), 0.0), [0.0, 0.0]);
}
