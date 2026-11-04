use std::collections::HashMap;

use ghost_world::tge::TgeClientSet;
use ghost_world::world::ClientWorld;
use ghosts::Value;

use super::*;
use crate::world::WheelSetup;
use crate::world::tests::datablock;

struct Files(HashMap<String, Vec<u8>>);

impl Assets for Files {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.0.get(&path.to_ascii_lowercase()).cloned()
    }
}

/// A vehicle with one hub and a tire datablock: its tire is drawn at the hub, let down its spring.
#[test]
fn a_tire_hangs_from_its_hub_at_its_springs_length_with_no_ground() {
    let b = [-1.0, -1.0, -1.0, 1.0, 1.0, 1.0];
    let body = shapes::synth::hull_shape(b, &[("detail32", b)], &["hub0"], &[]);
    let t = [-0.5, -0.5, -0.5, 0.5, 0.5, 0.5];
    let tire = shapes::synth::solid_dts(t, &[("detail32", t)], [0, 0, 0, 32767], [0.0; 3], false);
    let files = Files([("demo/data/shapes/tire.dts".to_string(), tire)].into());
    let mut w = ClientWorld::<TgeClientSet>::default();
    w.datablocks.insert(
        20,
        datablock(
            "WheeledVehicleTire",
            &[
                ("shapeFile", Value::str("demo/data/shapes/tire.dts")),
                ("radius", Value::F32(0.5)),
            ],
        ),
    );
    w.datablocks.insert(
        21,
        datablock("WheeledVehicleSpring", &[("length", Value::F32(2.0))]),
    );
    let mut charge = |_: u64| true;
    let mut instance = ShapeInstance::new(&body.shape, &mut charge).unwrap();
    instance.animate(&body.shape, 0, None, &mut charge).unwrap();
    let mut models = Models::default();
    let mut textures = TextureSet::new(&files);
    let mut notes = Vec::new();
    let tires = Tires {
        world: &w,
        assets: &files,
        wheels: Wheels {
            steering: 0.0,
            wheels: vec![Some(WheelSetup {
                tire: 20,
                spring: 21,
                steering: 0.0,
            })],
        },
        look: None,
        vehicle: &body,
        instance: &instance,
        xform: &Mat::IDENTITY,
        scale: [1.0; 3],
        eye: [0.0, -10.0, 0.0],
        pixels_per_meter: 400.0,
        point: &[],
        // (The synthetic shapes carry no radius: the most detailed level.)
        policy: DetailPolicy::Highest,
        nodes: false,
    };
    let draws = tire_draws(&tires, &mut models, &mut textures, &mut notes);
    assert!(!draws.is_empty(), "{notes:?}");
    assert!(draws.iter().all(|d| d.label.contains("tire 0")));
    // The hub at the origin, the spring's full length (2) down: the tire's box about z = -2.
    let zs: Vec<f32> = draws
        .iter()
        .flat_map(|d| d.vertices.iter().map(|v| v.position[2]))
        .collect();
    assert!(zs.iter().all(|z| (-2.6..=-1.4).contains(z)), "{zs:?}");
}
