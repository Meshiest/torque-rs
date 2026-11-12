use super::*;

fn field_type(vm: &Vm, class: &str, field: &str) -> Option<FieldType> {
    let classes = &vm.objects().classes;
    let def = classes.get(classes.find(class.as_bytes())?)?;
    def.layout.field(field.as_bytes()).map(|(f, _)| f.ty)
}

#[test]
fn the_classes_alone_make_the_game_tree_with_its_typed_fields() {
    let mut vm = Vm::new();
    assert_eq!(install(&mut vm), CLASSES.len());
    let classes = &vm.objects().classes;
    let net = classes.find(b"NetObject").expect("NetObject");
    let datablock = classes.find(b"SimDataBlock").expect("SimDataBlock");
    for spec in CLASSES {
        let class = classes.find(spec.name.as_bytes()).expect(spec.name);
        let parent = classes.find(spec.parent.unwrap().as_bytes()).unwrap();
        assert!(
            classes.is_a(class, parent),
            "{} under {:?}",
            spec.name,
            spec.parent
        );
        // Every class is a network object (the scene tree, the sun) or a datablock, but the path a camera follows and the
        // connections (groups).
        assert!(
            classes.is_a(class, net)
                || classes.is_a(class, datablock)
                || ["Path", "NetConnection", "GameConnection"].contains(&spec.name),
            "{}",
            spec.name
        );
    }
    // A subclass reads its ancestors' fields (any case), each with its type.
    assert_eq!(
        field_type(&vm, "AIPlayer", "POSITION"),
        Some(FieldType::MatrixPosition)
    );
    assert_eq!(
        field_type(&vm, "Player", "dataBlock"),
        field_type(&vm, "GameBase", "dataBlock")
    );
    assert_eq!(
        field_type(&vm, "PlayerData", "shapeFile"),
        Some(FieldType::Filename)
    );
    assert_eq!(
        field_type(&vm, "PlayerData", "category"),
        Some(FieldType::CaseString)
    );
    assert_eq!(
        field_type(&vm, "ShapeBaseData", "explosion"),
        Some(FieldType::DataBlock("ExplosionData"))
    );
    assert_eq!(field_type(&vm, "Camera", "shapeFile"), None);
    // A second install adds nothing.
    assert_eq!(install(&mut vm), 0);
}

#[test]
fn position_and_rotation_share_the_transform() {
    let mut vm = Vm::new();
    install(&mut vm);
    let classes = &vm.objects().classes;
    let def = classes.get(classes.find(b"SceneObject").unwrap()).unwrap();
    let (_, position) = def.layout.field(b"position").unwrap();
    let (_, rotation) = def.layout.field(b"rotation").unwrap();
    let (_, scale) = def.layout.field(b"scale").unwrap();
    assert_eq!(position, rotation);
    assert_ne!(position, scale);
}

#[test]
fn a_class_the_host_registered_first_keeps_its_own_fields() {
    let mut vm = Vm::new();
    let own = vec![ts_vm::sim::StaticField {
        name: b"ownField".to_vec(),
        ty: FieldType::S32,
        count: 1,
        offset: None,
        validator: None,
        doc: None,
    }];
    vm.register_class_typed("NetObject", "SimObject", own, true)
        .unwrap();
    assert_eq!(install(&mut vm), CLASSES.len() - 1);
    assert_eq!(
        field_type(&vm, "SceneObject", "ownField"),
        Some(FieldType::S32)
    );
    assert_eq!(field_type(&vm, "Item", "static"), Some(FieldType::Bool));
}

#[test]
fn the_map_classes_come_alone_with_their_defaults_and_a_hosts_own_fields() {
    use ts_vm::fields::Value;
    static OWN: &[FieldSpec] = &[
        field("zeroBumpScale", FieldType::F32, 1),
        field("ownFlag", FieldType::Bool, 1),
    ];
    static EXTRA: &[ClassExtra] = &[ClassExtra {
        class: "terrainblock",
        fields: OWN,
        defaults: &[("ownFlag", "1"), ("squareSize", "16")],
    }];
    let mut vm = Vm::new();
    let ancestors = CLASSES
        .iter()
        .filter(|c| ["NetObject", "SceneObject"].contains(&c.name));
    for spec in ancestors {
        install_spec(&mut vm, spec);
    }
    assert_eq!(install_map_classes(&mut vm, EXTRA), MAP_CLASSES.len());
    assert_eq!(install_map_classes(&mut vm, EXTRA), 0);
    assert!(vm.objects().classes.find(b"Player").is_none());
    let classes = &vm.objects().classes;
    let terrain = classes.get(classes.find(b"TerrainBlock").unwrap()).unwrap();
    let names: Vec<&[u8]> = terrain
        .layout
        .fields
        .iter()
        .map(|(f, _)| &f.name[..])
        .collect();
    // The host's declaration replaces TGE's in place; its own field comes last.
    assert_eq!(names.iter().position(|n| *n == b"zeroBumpScale"), Some(10));
    assert_eq!(names.last(), Some(&&b"ownFlag"[..]));
    let initial = |def: &ts_vm::sim::ClassDef, name: &[u8]| {
        let (_, slots) = def.layout.field(name).unwrap();
        def.layout.initial[slots[0]].clone()
    };
    assert_eq!(
        field_type(&vm, "TerrainBlock", "zeroBumpScale"),
        Some(FieldType::F32)
    );
    assert_eq!(initial(terrain, b"zeroBumpScale"), Value::Float(8.0));
    assert_eq!(initial(terrain, b"squareSize"), Value::Int(16));
    assert_eq!(initial(terrain, b"ownFlag"), Value::Bool(true));
    assert_eq!(initial(terrain, b"scale"), Value::Floats3([1.0; 3]));
    let water = classes.get(classes.find(b"WaterBlock").unwrap()).unwrap();
    assert_eq!(initial(water, b"liquidType"), Value::Int(1));
    assert_eq!(initial(water, b"envMapIntensity"), Value::Float(0.4));
}

#[test]
fn a_scene_object_and_a_player_datablock_read_their_constructors_values() {
    let mut vm = Vm::new();
    install(&mut vm);
    let text = |vm: &mut Vm, expr: &str| {
        let line = format!("$out = {expr};");
        vm.eval(line.as_bytes()).unwrap();
        String::from_utf8_lossy(&vm.global("$out").unwrap()).into_owned()
    };
    vm.eval(b"new StaticShape(Box); new PlayerData(Walker) { runForce = 900; };")
        .unwrap();
    // `SceneObject`'s constructor: a scale of 1 (a zero scale makes a point of every box).
    assert_eq!(text(&mut vm, "Box.scale"), "1 1 1");
    // A PlayerData keeps what the script set and reads the constructor's value for the rest.
    assert_eq!(text(&mut vm, "Walker.runForce"), "900");
    assert_eq!(text(&mut vm, "Walker.horizMaxSpeed"), "80");
    assert_eq!(text(&mut vm, "Walker.maxForwardSpeed"), "10");
    assert_eq!(text(&mut vm, "Walker.cameraMaxFov"), "120");
}

#[test]
fn the_scene_and_shape_classes_name_the_methods_their_natives_replace() {
    for (class, method) in [
        ("SceneObject", "setTransform"),
        ("SceneObject", "getPosition"),
        ("ShapeBase", "playThread"),
        ("ShapeBase", "mountImage"),
        ("ShapeBase", "setVelocity"),
        ("Player", "setActionThread"),
        ("Camera", "setOrbitMode"),
        ("Item", "isStatic"),
        ("WheeledVehicle", "getWheelCount"),
        ("GameConnection", "activateGhosting"),
        ("GameConnection", "setControlObject"),
    ] {
        assert!(
            crate::methods::find_in(CLASSES, class, method).is_some(),
            "{class}::{method}"
        );
    }
}

/// A datablock a script made reads the defaults that leave TGE's particles as they are: an emitter node's `timeMultiple` is 1
/// (TGE's constructor), a particle's engine fields their `ParticleExt` defaults, and the rest of the new fields their zero.
#[test]
fn the_particle_datablocks_read_defaults_that_leave_tges_particles_as_they_are() {
    use torque_ghosts::datablocks::effects::ParticleExt;
    use ts_vm::fields::Value;
    let mut vm = Vm::new();
    assert_eq!(install(&mut vm), CLASSES.len());
    let classes = &vm.objects().classes;
    let initial = |class: &str, name: &str| {
        let def = classes
            .get(classes.find(class.as_bytes()).unwrap())
            .unwrap();
        let (_, slots) = def.layout.field(name.as_bytes()).unwrap();
        def.layout.initial[slots[0]].clone()
    };
    assert_eq!(
        initial("ParticleEmitterNodeData", "timeMultiple"),
        Value::Float(1.0)
    );
    for (name, want) in [
        ("turbulenceScale", ParticleExt::DEFAULT_TURBULENCE_SCALE),
        ("bounceElasticity", ParticleExt::DEFAULT_BOUNCE_ELASTICITY),
        ("bounceFriction", ParticleExt::DEFAULT_BOUNCE_FRICTION),
        ("softDistance", ParticleExt::DEFAULT_SOFT_DISTANCE),
        ("turbulence", 0.0),
        ("sizeByVelocity", 0.0),
    ] {
        assert_eq!(initial("ParticleData", name), Value::Float(want), "{name}");
    }
    assert_eq!(initial("ParticleData", "collide"), Value::Bool(false));
    assert_eq!(initial("ParticleData", "lit"), Value::Int(0));
    assert_eq!(initial("ParticleEmitterData", "budget"), Value::Int(0));
    assert_eq!(initial("ParticleEmitterData", "trail"), Value::Bool(false));
}

/// The distortion's script defaults are the wire block's: a ShapeBaseData or a DecalData a script made reads a map scale of 1
/// and no tint, and every class built on ShapeBaseData has the fields.
#[test]
fn the_distortion_defaults_are_the_wires() {
    use torque_ghosts::datablocks::distortion::{DISTORTION_DEFAULTS, DistortionExt};
    use ts_vm::fields::Value;
    let mut vm = Vm::new();
    assert_eq!(install(&mut vm), CLASSES.len());
    let classes = &vm.objects().classes;
    let initial = |class: &str, name: &str, k: usize| {
        let def = classes
            .get(classes.find(class.as_bytes()).unwrap())
            .unwrap();
        let (_, slots) = def
            .layout
            .field(name.as_bytes())
            .unwrap_or_else(|| panic!("{class} has no {name}"));
        def.layout.initial[slots[k]].clone()
    };
    let d = DistortionExt::default();
    for class in ["ShapeBaseData", "DecalData"] {
        assert_eq!(
            initial(class, "distortionMapScale", 0),
            Value::Float(d.map_scale.get()),
            "{class}"
        );
        assert_eq!(
            initial(class, "distortion", 0),
            Value::Float(0.0),
            "{class}"
        );
    }
    for class in [
        "StaticShapeData",
        "PlayerData",
        "ItemData",
        "WheeledVehicleData",
    ] {
        // Every class built on ShapeBaseData reads the defaults (one with its own list too).
        assert_eq!(
            initial(class, "distortionMapScale", 0),
            Value::Float(d.map_scale.get()),
            "{class}"
        );
        assert_eq!(
            initial(class, "distortionOnly", 0),
            Value::Bool(false),
            "{class}"
        );
    }
    for (name, text) in DISTORTION_DEFAULTS {
        assert!(
            SHAPE_BASE_DATA_DEFAULTS.contains(&(*name, *text)),
            "ShapeBaseData's defaults lack {name} = {text}"
        );
    }
}
