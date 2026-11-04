use ghost_world::tge::TgeClientSet;
use ghost_world::world::ClientWorld;
use ghosts::shapes::StaticShape;
use ghosts::{F32, TgeClass, TgeRecord, TgeShapeBase, Value, Xf};

use super::*;
use crate::world::tests::{datablock, ghost};

const GLOWING: [(&str, [u8; 4]); 1] = [("paper", [150, 120, 90, 255])];

const LAMP: Lamp = Lamp {
    shape: "/props/lantern.dts",
    color: [1.0, 0.5, 0.25],
    brightness: 10.0,
    radius: 50.0,
    lift: 2.0,
    shade_brightness: 4.0,
    shade_radius: 30.0,
    shade_lift: 0.0,
};

#[test]
fn a_glowing_texture_glows_and_others_do_not() {
    assert!(glow_of("map interiors packs/house/paper", &GLOWING).is_some());
    assert!(glow_of("map interiors packs/house/Paper lit", &GLOWING).is_some());
    assert!(glow_of("map interiors packs/house/paper.jpg", &GLOWING).is_some());
    assert!(glow_of("map interiors packs/house/carpet1", &GLOWING).is_none());
    assert!(glow_of("map interiors packs/house/notpaper", &GLOWING).is_none());
    assert!(glow_of("map interiors packs/house/paper", &[]).is_none());
}

#[test]
fn a_static_shape_of_a_lamps_shape_lights_above_its_centre() {
    let mut w = ClientWorld::<TgeClientSet>::default();
    w.datablocks.insert(
        4,
        datablock(
            "StaticShapeData",
            &[("shapeFile", Value::str("demo/data/props/Lantern.dts"))],
        ),
    );
    let lantern = StaticShape {
        shape: TgeShapeBase {
            game: ghosts::parts::GameBase {
                scale: None,
                datablock: Some(4),
            },
            body: None,
        },
        transform: Some((
            Xf {
                position: [F32::new(1.0), F32::new(2.0), F32::new(3.0)],
                quat: [F32::new(0.0); 3],
                negative_w: false,
            },
            [F32::new(1.0); 3],
        )),
        powered: false,
    };
    w.ghosts.insert(
        7,
        ghost(TgeClass::StaticShape, TgeRecord::StaticShape(lantern)),
    );
    let l = lights(&w, &[LAMP]);
    assert_eq!(
        l,
        vec![LampLight {
            position: [1.0, 2.0, 5.0],
            color: [10.0, 5.0, 2.5],
            radius: 50.0,
            ghost: 7,
        }]
    );
    assert_eq!(shade_lights(&w, &[LAMP])[0].position, [1.0, 2.0, 3.0]);
    assert!(lights(&w, &[]).is_empty());
}
