use super::*;

#[test]
fn torques_sequence_texts_name_the_sequence_and_the_shape_in_255_bytes() {
    let rules = TorqueRules;
    assert_eq!(
        rules.missing_sequence(b"run.dsq", b"base.dts"),
        "Missing sequence run.dsq for base.dts"
    );
    assert_eq!(
        rules.load_sequence_failed(b"run.dsq", b"base.dts"),
        "Load sequence run.dsq failed for base.dts"
    );
    assert_eq!(rules.missing_sequence(&[b'x'; 400], b"s").len(), 255);
}

#[test]
fn torques_failed_shape_loads_refuse_the_datablock_and_its_constructor_loop_skips_unset_fields() {
    assert_eq!(
        couldnt_load_shape(ShapeClass::ShapeBase, "a/b.dts"),
        "ShapeBaseData: Couldn't load shape \"a/b.dts\""
    );
    assert_eq!(couldnt_load_shape(ShapeClass::Explosion, "a/b.dts"), "Exp");
    assert_eq!(
        couldnt_load_shape(ShapeClass::Tire, "t.dts"),
        "WheeledVehicleTire: Couldn't load shape \"t.dts\""
    );
    assert_eq!(
        couldnt_load_shape(ShapeClass::ShapeBase, &"x".repeat(400)).len(),
        255
    );
    assert!(!TorqueRules.constructor_stops_at_unset());
}

#[test]
fn torques_tire_radius_is_half_the_height() {
    assert_eq!(
        TorqueRules.tire_radius(&[0.0, 0.0, -1.0, 0.0, 0.0, 2.0]),
        1.5
    );
}

#[test]
fn torques_audio_clamp_is_mclampf() {
    assert_eq!(clamp(1.5, 0.0, 1.0), 1.0);
    assert_eq!(clamp(-0.5, 0.0, 1.0), 0.0);
    assert_eq!(clamp(0.25, 0.0, 1.0), 0.25);
}
