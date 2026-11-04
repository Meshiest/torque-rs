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
    // The event's own class takes the update in place.
    let same = Unpacked {
        class: "AudioDescription".to_string(),
        fields: vec![field("volume", "1")],
        ..other
    };
    assert_ne!(receive(&mut vm, &same, None).unwrap(), Received::Refused);
    assert_eq!(vm.eval(b"return QuietDesc.volume;").unwrap(), b"1");
}
