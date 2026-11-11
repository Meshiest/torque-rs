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
    let mut vm = Vm::new();
    torque_game::specs::install(&mut vm);
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
    let mut vm = Vm::new();
    torque_game::specs::install(&mut vm);
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
    let mut vm = Vm::new();
    torque_game::specs::install(&mut vm);
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
    let mut vm = Vm::new();
    torque_game::specs::install(&mut vm);
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
