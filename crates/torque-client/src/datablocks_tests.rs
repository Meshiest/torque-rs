use ts_vm::vm::Vm;

use super::*;

fn field(name: &str, text: &str) -> UnpackedField {
    UnpackedField {
        name: name.as_bytes().to_vec(),
        index: Vec::new(),
        text: text.as_bytes().to_vec(),
        pointer: None,
    }
}

#[test]
fn an_event_for_a_datablock_of_another_class_is_refused_and_leaves_it_alone() {
    let mut vm = torque_game::testing::vm();
    let id = vm
        .eval(b"datablock AudioDescription(QuietDesc) { volume = 0.5; }; return QuietDesc.getId();")
        .unwrap();
    let id: u32 = String::from_utf8_lossy(&id).parse().unwrap();
    let other = Unpacked {
        id,
        class: "AudioProfile".to_string(),
        fields: vec![field("fileName", "x.wav"), field("volume", "1")],
        error: None,
        emitter_ids: None,
    };
    assert_eq!(receive(&mut vm, &other, None).unwrap(), Received::Refused);
    assert_eq!(
        vm.eval(b"return QuietDesc.getClassName() SPC QuietDesc.volume SPC QuietDesc.fileName;")
            .unwrap(),
        b"AudioDescription 0.5 "
    );
    // The event's own class is an update, but the object is not the client's to write (a script made it, as a server's own
    // datablock is on the engine a local client shares): preloaded, its fields left as they are, its generation moved.
    let same = Unpacked {
        class: "AudioDescription".to_string(),
        fields: vec![field("volume", "1")],
        ..other
    };
    assert_eq!(
        receive(&mut vm, &same, None).unwrap(),
        Received::Updated(id)
    );
    assert_eq!(vm.eval(b"return QuietDesc.volume;").unwrap(), b"0.5");
    let objects = DatablockObjects::of(&vm).expect("the objects' table");
    assert!(!objects.made(&vm, id));
    assert_eq!(objects.generation(id), 1);
    assert_eq!(
        receive(&mut vm, &same, None).unwrap(),
        Received::Updated(id)
    );
    assert_eq!(
        objects.generation(id),
        2,
        "a record for an object not the client's counts as a change"
    );
    assert_eq!(vm.eval(b"return QuietDesc.volume;").unwrap(), b"0.5");
}

#[test]
fn an_object_the_client_made_takes_an_update_in_place_and_an_unchanged_resend_moves_nothing() {
    let mut vm = torque_game::testing::vm();
    let id = ts_vm::sim::FIRST_DATABLOCK_ID + 7;
    let event = |volume: &str| Unpacked {
        id,
        class: "AudioDescription".to_string(),
        fields: vec![field("volume", volume)],
        error: None,
        emitter_ids: None,
    };
    assert_eq!(
        receive(&mut vm, &event("0.5"), None).unwrap(),
        Received::Added(id)
    );
    let objects = DatablockObjects::of(&vm).expect("the objects' table");
    assert!(objects.made(&vm, id));
    let (generation, epoch) = (objects.generation(id), objects.epoch());
    assert_eq!(generation, 1);
    let read = |vm: &Vm| vm.read_field_of(ts_vm::vm::FieldTarget::Live(id), b"volume", b"");
    assert_eq!(read(&vm), b"0.5");
    for _ in 0..100 {
        assert_eq!(
            receive(&mut vm, &event("0.5"), None).unwrap(),
            Received::Updated(id)
        );
    }
    assert_eq!(
        (objects.generation(id), objects.epoch()),
        (generation, epoch)
    );
    assert_eq!(
        receive(&mut vm, &event("0.25"), None).unwrap(),
        Received::Updated(id)
    );
    assert_eq!(read(&vm), b"0.25");
    assert_eq!(objects.generation(id), 2);
    assert_eq!(objects.epoch(), epoch + 1);
}

#[test]
fn a_scripts_datablock_at_an_id_the_clients_own_had_is_not_written() {
    let mut vm = torque_game::testing::vm();
    let id = ts_vm::sim::FIRST_DATABLOCK_ID + 9;
    let event = |volume: &str| Unpacked {
        id,
        class: "AudioDescription".to_string(),
        fields: vec![field("volume", volume)],
        error: None,
        emitter_ids: None,
    };
    assert_eq!(
        receive(&mut vm, &event("0.5"), None).unwrap(),
        Received::Added(id)
    );
    let objects = DatablockObjects::of(&vm).expect("the objects' table");
    assert!(objects.made(&vm, id));
    // The client's object goes and a script's own datablock takes the id.
    vm.delete_object(id).unwrap();
    assert!(!objects.made(&vm, id));
    vm.objects_mut().set_next_datablock_id(id);
    let taken = vm
        .eval(b"datablock AudioDescription(Mine) { volume = 0.25; }; return Mine.getId();")
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&taken).parse::<u32>().unwrap(), id);
    assert!(!objects.made(&vm, id));
    assert_eq!(
        receive(&mut vm, &event("1"), None).unwrap(),
        Received::Updated(id)
    );
    assert_eq!(vm.eval(b"return Mine.volume;").unwrap(), b"0.25");
    assert!(
        !objects.made(&vm, id),
        "the entry no longer says the client's"
    );
    assert_eq!(objects.generation(id), 2);
}

#[test]
fn over_a_script_engine_shared_with_the_server_the_servers_own_datablock_is_left_as_it_is() {
    let mut vm = torque_game::testing::vm();
    let id = vm
        .eval(
            b"datablock AudioDescription(ServerDesc) { volume = 0.5; }; return ServerDesc.getId();",
        )
        .unwrap();
    let id: u32 = String::from_utf8_lossy(&id).parse().unwrap();
    let event = Unpacked {
        id,
        class: "AudioDescription".to_string(),
        fields: vec![field("volume", "1")],
        error: None,
        emitter_ids: None,
    };
    assert_eq!(
        receive_from(&mut vm, &event, None, Sharing::WithServer).unwrap(),
        Received::Updated(id)
    );
    assert_eq!(vm.eval(b"return ServerDesc.volume;").unwrap(), b"0.5");
    // A datablock a script made is never written by an event, whatever the sharing: only an object the client made from an
    // earlier event takes the fields (`DatablockObjects::made`); the event still runs the preload.
    assert_eq!(
        receive_from(&mut vm, &event, None, Sharing::Own).unwrap(),
        Received::Updated(id)
    );
    assert_eq!(vm.eval(b"return ServerDesc.volume;").unwrap(), b"0.5");
}

#[test]
fn an_update_puts_an_out_of_range_value_in_range_as_the_first_add_does_and_says_nothing() {
    let mut vm = torque_game::testing::vm();
    torque_game::shapes::register(&mut vm);
    let id = ts_vm::sim::FIRST_DATABLOCK_ID + 9;
    let event = |lifetime: &str| Unpacked {
        id,
        class: "DebrisData".to_string(),
        fields: vec![field("lifetime", lifetime)],
        error: None,
        emitter_ids: None,
    };
    let read = |vm: &Vm| vm.read_field_of(ts_vm::vm::FieldTarget::Live(id), b"lifetime", b"");
    assert_eq!(
        receive(&mut vm, &event("-1"), None).unwrap(),
        Received::Added(id)
    );
    let added = read(&vm);
    assert_ne!(added, b"-1", "the first add clamps");
    assert_eq!(
        receive(&mut vm, &event("4"), None).unwrap(),
        Received::Updated(id)
    );
    assert_eq!(read(&vm), b"4");
    vm.drain_log();
    assert_eq!(
        receive(&mut vm, &event("-1"), None).unwrap(),
        Received::Updated(id)
    );
    assert_eq!(read(&vm), added, "an update clamps the same way");
    assert!(
        vm.log_texts().iter().all(|l| !l.contains("DebrisData")),
        "{:?}",
        vm.log_texts()
    );
}

/// A client over the game classes' behaviors: its add-time checks run.
fn checked_client() -> (Vm, Rc<torque_game::shapes::Shapes>) {
    let mut vm = torque_game::testing::vm();
    let shapes = torque_game::shapes::register(&mut vm);
    (vm, shapes)
}

#[test]
fn an_unchanged_resend_is_not_checked_again() {
    let (mut vm, _) = checked_client();
    let id = ts_vm::sim::FIRST_DATABLOCK_ID + 11;
    let event = Unpacked {
        id,
        class: "DebrisData".to_string(),
        fields: vec![field("lifetime", "2")],
        error: None,
        emitter_ids: None,
    };
    assert_eq!(receive(&mut vm, &event, None).unwrap(), Received::Added(id));
    // Another field the record does not carry is out of range; a resend that changes nothing leaves it, a changed one checks it.
    vm.set_object_member(id, b"friction", b"", b"50");
    let friction = |vm: &Vm| vm.read_field_of(ts_vm::vm::FieldTarget::Live(id), b"friction", b"");
    assert_eq!(
        receive(&mut vm, &event, None).unwrap(),
        Received::Updated(id)
    );
    assert_eq!(friction(&vm), b"50");
    let changed = Unpacked {
        fields: vec![field("lifetime", "3")],
        ..event
    };
    assert_eq!(
        receive(&mut vm, &changed, None).unwrap(),
        Received::Updated(id)
    );
    assert_ne!(friction(&vm), b"50");
}

#[test]
fn a_datablock_the_client_did_not_make_is_not_checked() {
    let (mut vm, _) = checked_client();
    let id = vm
        .eval(
            b"datablock DebrisData(Mine) { lifetime = 4; }; Mine.lifetime = -1; return Mine.getId();",
        )
        .unwrap();
    let id: u32 = String::from_utf8_lossy(&id).parse().unwrap();
    let event = Unpacked {
        id,
        class: "DebrisData".to_string(),
        fields: vec![field("lifetime", "-1")],
        error: None,
        emitter_ids: None,
    };
    for sharing in [Sharing::Own, Sharing::WithServer] {
        assert_eq!(
            receive_from(&mut vm, &event, None, sharing).unwrap(),
            Received::Updated(id)
        );
        assert_eq!(vm.eval(b"return Mine.lifetime;").unwrap(), b"-1");
    }
}

#[test]
fn a_client_emitter_takes_its_ids_for_the_particle_string_and_is_clamped_on_update() {
    let (mut vm, shapes) = checked_client();
    let values = shapes.values();
    let id = ts_vm::sim::FIRST_DATABLOCK_ID + 13;
    let event = |fields: Vec<UnpackedField>| Unpacked {
        id,
        class: "ParticleEmitterData".to_string(),
        fields,
        error: None,
        emitter_ids: Some(vec![7]),
    };
    let read = |vm: &Vm, name: &[u8]| vm.read_field_of(ts_vm::vm::FieldTarget::Live(id), name, b"");
    // The first add: no `particles` string, only the ids; the lifetime is clamped.
    assert_eq!(
        receive(
            &mut vm,
            &event(vec![field("lifetimeMS", "-3")]),
            Some(values)
        )
        .unwrap(),
        Received::Added(id)
    );
    assert_eq!(read(&vm, b"lifetimeMS"), b"0");
    assert_eq!(values.emitter_ids(id), vec![7]);
    // An update with another out-of-range lifetime is clamped the same way.
    assert_eq!(
        receive(
            &mut vm,
            &event(vec![field("lifetimeMS", "-7")]),
            Some(values)
        )
        .unwrap(),
        Received::Updated(id)
    );
    assert_eq!(read(&vm, b"lifetimeMS"), b"0");
    assert_eq!(values.emitter_ids(id), vec![7]);
    // An update the checks refuse (a particle string over 255 characters) is dropped: the fields the record carried, those the
    // checks clamped before refusing and the ids all stay as they were.
    let long = "a".repeat(300);
    let refused = Unpacked {
        emitter_ids: Some(vec![9]),
        ..event(vec![
            field("lifetimeMS", "5"),
            field("ejectionPeriodMS", "0"),
            field("particles", &long),
        ])
    };
    let period = read(&vm, b"ejectionPeriodMS");
    assert_eq!(
        receive(&mut vm, &refused, Some(values)).unwrap(),
        Received::Refused
    );
    assert_eq!(read(&vm, b"lifetimeMS"), b"0");
    assert_eq!(read(&vm, b"ejectionPeriodMS"), period);
    assert!(read(&vm, b"particles").is_empty());
    assert_eq!(values.emitter_ids(id), vec![7]);
}
