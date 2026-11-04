use ghost_world::tge::TgeClientSet;
use ghost_world::world::ClientWorld;

use super::*;
use crate::{Blend, Cull, TexEnv, Vertex};

fn draw(label: &str, x: f32) -> Draw {
    let v = |p: [f32; 3]| Vertex {
        position: p,
        normal: [0.0, 0.0, 1.0],
        color: [255; 4],
        uv: [0.0; 2],
    };
    Draw {
        label: label.to_string(),
        vertices: vec![v([x, 0.0, 0.0]), v([x + 1.0, 0.0, 0.0]), v([x, 1.0, 0.0])],
        lighting: true,
        tex_env: TexEnv::None,
        texture: None,
        blend: Blend::Opaque,
        cull: Cull::BackCcw,
        depth_test: true,
        depth_write: true,
        fog: false,
        lights: Vec::new(),
        vertex_key: None,
    }
}

/// Each ghost's draws become one object at the transform its draws were made with; the next frame's object of the ghost
/// knows where its vertices were.
#[test]
fn a_ghosts_draws_become_one_object_that_remembers_its_last_frame() {
    let w = ClientWorld::<TgeClientSet>::default();
    let mut shaped = ShapeDraws {
        opaque: vec![
            draw("shape StaticShape 1 a.dts object 0", 0.0),
            draw("shape StaticShape 2 b.dts object 0", 5.0),
        ],
        opaque_owners: vec![1, 2],
        ..Default::default()
    };
    let mut at = Mat::IDENTITY;
    at.set_column3([3.0, 0.0, 0.0]);
    shaped.transforms.insert(1, at);
    let mut drawn = Vec::new();
    let first = objects_of(&w, &shaped, &[], &mut drawn, None);
    assert_eq!(
        first.iter().map(|o| o.ghost).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(first[0].transform.column(3), [3.0, 0.0, 0.0]);
    // No ghost in the world and no transform: the identity.
    assert_eq!(first[1].transform, Mat::IDENTITY);
    assert!(
        first
            .iter()
            .all(|o| !o.translucent && o.draws.len() == 1 && o.stamp.is_none())
    );
    assert!(
        first[0].draws[0]
            .vertices
            .iter()
            .all(|v| v.material & material::PLAYER == 0)
    );
    // The next frame: ghost 1 moved by one unit; its object's last places are this frame's.
    shaped.opaque[0] = draw("shape StaticShape 1 a.dts object 0", 1.0);
    let mut drawn = first;
    let next = objects_of(&w, &shaped, &[], &mut drawn, None);
    let prev = next[0].previous.as_ref().expect("the last frame's places");
    assert_eq!(prev[0], [0.0, 0.0, 0.0]);
    assert_eq!(next[0].draws[0].vertices[0].position, [1.0, 0.0, 0.0]);
}
