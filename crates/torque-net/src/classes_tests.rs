use super::*;

fn rep(name: &str, kind: Kind, direction: Direction) -> NetRep {
    NetRep {
        name: name.to_string(),
        kind,
        direction,
    }
}

#[test]
fn ids_are_positions_in_the_byte_wise_unsigned_name_order() {
    let reps = [
        rep("fxSunLight", Kind::Object, Direction::Any),
        rep("WheeledVehicle", Kind::Object, Direction::Any),
        rep("Camera", Kind::Object, Direction::Any),
        rep("Zeta", Kind::DataBlock, Direction::Any),
        rep("Alpha", Kind::DataBlock, Direction::Any),
        rep("Beta", Kind::Event, Direction::ServerToClient),
    ];
    let c = NetClasses::derive(&reps).unwrap();
    // Upper case sorts before lower case: fxSunLight is after WheeledVehicle.
    assert_eq!(c.id_of(Kind::Object, "Camera"), Some(0));
    assert_eq!(c.id_of(Kind::Object, "WheeledVehicle"), Some(1));
    assert_eq!(c.id_of(Kind::Object, "fxSunLight"), Some(2));
    assert_eq!(c.id_of(Kind::DataBlock, "Alpha"), Some(0));
    assert_eq!(c.id_of(Kind::DataBlock, "Zeta"), Some(1));
    // Ids restart for each kind.
    assert_eq!(c.id_of(Kind::Event, "Beta"), Some(0));
    assert_eq!(
        c.by_id(Kind::Object, 2).map(|k| k.name.as_str()),
        Some("fxSunLight")
    );
    assert_eq!(c.by_id(Kind::Object, 3), None);
    assert_eq!(c.total(), 6);
    assert_eq!(c.id_of(Kind::Event, "Camera"), None);
}

#[test]
fn the_sort_does_not_depend_on_the_input_order() {
    let mut reps: Vec<NetRep> = ["b", "a", "C", "_x", "B"]
        .iter()
        .map(|n| rep(n, Kind::Object, Direction::Any))
        .collect();
    let a = NetClasses::derive(&reps).unwrap();
    reps.reverse();
    assert_eq!(NetClasses::derive(&reps).unwrap(), a);
    let order: Vec<&str> = a
        .classes(Kind::Object)
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(order, ["B", "C", "_x", "a", "b"]);
}

#[test]
fn duplicate_and_empty_names_are_refused() {
    let dup = [
        rep("A", Kind::Event, Direction::Any),
        rep("A", Kind::Event, Direction::Any),
    ];
    assert_eq!(
        NetClasses::derive(&dup),
        Err(ClassError::DuplicateName("A".to_string()))
    );
    // The same name in two kinds is fine.
    let two = [
        rep("A", Kind::Event, Direction::Any),
        rep("A", Kind::Object, Direction::Any),
    ];
    assert!(NetClasses::derive(&two).is_ok());
    assert_eq!(
        NetClasses::derive(&[rep("", Kind::Event, Direction::Any)]),
        Err(ClassError::EmptyName)
    );
}

#[test]
fn a_class_id_takes_the_bit_length_of_the_count() {
    let make = |n: usize, kind| {
        let reps: Vec<NetRep> = (0..n)
            .map(|i| rep(&format!("c{i:03}"), kind, Direction::Any))
            .collect();
        NetClasses::derive(&reps).unwrap().id_bits(kind)
    };
    assert_eq!(make(18, Kind::Event), 5);
    assert_eq!(make(30, Kind::DataBlock), 5);
    assert_eq!(make(34, Kind::Object), 6);
    // "Strictly above": 16 classes need 5 bits for the id 16 the count would be; 15 need 4.
    assert_eq!(make(16, Kind::Event), 5);
    assert_eq!(make(15, Kind::Event), 4);
    assert_eq!(make(0, Kind::Event), 0);
}

#[test]
fn the_ghost_width_is_the_ceiling_log2_of_the_limit() {
    use crate::profile::DEFAULT_GHOST_LIMIT;
    assert_eq!(ghost_id_bits(DEFAULT_GHOST_LIMIT), 12);
    assert_eq!(ghost_id_bits(262_143), 18);
    assert_eq!(ghost_id_bits(300_000), 19);
    assert_eq!(ghost_id_bits(4_096), 12);
    // At least 1 bit, at most the crate's bound.
    assert_eq!(ghost_id_bits(0), 1);
    assert_eq!(ghost_id_bits(1), 1);
    assert_eq!(ghost_id_bits(2), 1);
    assert_eq!(ghost_id_bits(u32::MAX), crate::cap::NET_MAX_GHOST_BITS);
}

#[test]
fn directions_are_counted() {
    let reps = [
        rep("A", Kind::Event, Direction::Any),
        rep("B", Kind::Event, Direction::ServerToClient),
        rep("C", Kind::Event, Direction::ServerToClient),
        rep("D", Kind::Event, Direction::ClientToServer),
    ];
    let c = NetClasses::derive(&reps).unwrap();
    assert_eq!(c.direction_counts(Kind::Event), (1, 2, 1));
    assert_eq!(c.direction_counts(Kind::Object), (0, 0, 0));
    assert_eq!(Kind::from_word(2), Some(Kind::Event));
    assert_eq!(Kind::from_word(u32::MAX), None);
    assert_eq!(Direction::from_word(3), None);
}
