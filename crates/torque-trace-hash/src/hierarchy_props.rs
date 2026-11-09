//! The props-in-the-hierarchy scenario (`hierarchy-props`, engine mode): a server with the props plugin and the hierarchy, and a
//! predicting client. Four crates hang on a lift that slides, rises and turns (driven children) with a loose crate riding each; three
//! crates carry two parts each (compound bodies) while the figure walks through them; and along the way a driven child is let go, a
//! loose crate is hung on a body, a body is frozen and thawed (its parts become driven and join it again), a part's local pose is
//! set, a body with parts is deleted, and a driven child is hung on another. Each tick the hash takes in the figure as the server
//! and the client's prediction have it, every prop's state, every part's pose in its body's frame, every child's local pose and
//! the client's book.

use torque_plugin_hierarchy::ServerHierarchy;
use torque_plugin_props::{ClientProps, ServerProps};

use crate::hash::TraceHasher;
use crate::hierarchy::{eval, move_shape, object, triangle};
use crate::props::{figure_at, props_rig_in, rig_resource};
use crate::rig::{Control, Mode};
use crate::scenarios::{Outcome, TICKS, observe_client, observe_server};

/// The crate the scenario spawns, defined before the client is sent the datablocks.
const DATABLOCK: &str = "datablock PropData(TraceCrate) { size = \"0.8 0.8 0.8\"; mass = 15; friction = 0.6; restitution = 0.1; };";

/// The scene: the lift, four crates hung on it with a loose crate over each, and three bodies with two parts each.
const SCENE: &str = "\
$HP::Lift = new StaticShape() { dataBlock = TraceFloorData; position = \"-6 8 0.25\"; scale = \"8 3 0.5\"; };\n\
for (%i = 0; %i < 4; %i++) {\n\
   $HP::K[%i] = propSpawn(TraceCrate, (-8.5 + %i * 1.6) SPC 8 SPC 0.9);\n\
   $HP::L[%i] = propSpawn(TraceCrate, (-8.5 + %i * 1.6) SPC 8.1 SPC 2.2);\n\
   if (!$HP::K[%i].setParent($HP::Lift)) return \"kinematic \" @ %i @ \": \" @ getParentError();\n\
}\n\
for (%i = 0; %i < 3; %i++) {\n\
   $HP::R[%i] = propSpawn(TraceCrate, (2 + %i * 3.5) SPC 9 SPC 1.5);\n\
   $HP::A[%i] = propSpawn(TraceCrate, (2 + %i * 3.5 - 1) SPC 9 SPC 1.5);\n\
   $HP::B[%i] = propSpawn(TraceCrate, (2 + %i * 3.5 + 1) SPC 9 SPC 1.5);\n\
   if (!($HP::A[%i].setParent($HP::R[%i]) && $HP::B[%i].setParent($HP::R[%i]))) return \"compound \" @ %i @ \": \" @ getParentError();\n\
}\n\
return 1;";

/// What happens to the tree at a tick, besides the lift's move.
fn change(tick: u32) -> Option<&'static str> {
    match tick {
        // A driven child is let go at its lift's speed.
        150 => Some("$HP::K0.setParent(0);"),
        // A loose crate is hung on a body (it joins it where it lies).
        200 => Some("$HP::L3.setParent($HP::R0);"),
        // A body is frozen: its parts are driven where they are; thawed, they join it again.
        300 => Some("propFreeze($HP::R1);"),
        380 => Some("propUnfreeze($HP::R1);"),
        // A part's local pose changes (its collider moves in its body's frame).
        450 => Some("$HP::A0.setLocalTransform(\"-1.3 0.2 0.3 0 0 1 0.3\");"),
        // A body with parts is deleted; they leave where they are.
        520 => Some("propDelete($HP::R2);"),
        // A driven child is hung on another driven child.
        560 => Some("$HP::K1.setParent($HP::K2);"),
        _ => None,
    }
}

/// The lift's pose at `tick`: whole steps of a fixed size, so the text is the same on every target.
fn lift_pose(tick: u32) -> String {
    let x = -6.0 + triangle(tick, 100) as f32 * 0.02;
    let z = 0.25 + triangle(tick, 80) as f32 * 0.004;
    let yaw = triangle(tick, 120) as f32 * 0.001;
    format!("{x} 8 {z} 0 0 1 {yaw}")
}

/// The scenario's run (module docs).
pub fn run() -> Result<Outcome, String> {
    let client = torque_engine::EngineBuilder::new()
        .add_plugins(torque_plugin_props::PropsPlugin)
        .build()
        .map_err(|e| e.to_string())?;
    let mut rig = props_rig_in(Mode::Engine, DATABLOCK, &client)?;
    let made = eval(&mut rig, SCENE)?;
    if made != "1" {
        return Err(format!("the scene was refused: {made}"));
    }
    let lift = object(&mut rig, "$HP::Lift")?;
    let server_props = rig_resource::<ServerProps>(&rig)?;
    let book = client
        .resource::<ClientProps>()
        .ok_or("the client's book")?
        .clone();
    let hierarchy = ServerHierarchy::of(rig.server.vm()).ok_or("the server has no hierarchy")?;
    let start = figure_at(&rig)?;
    let mut h = TraceHasher::new();
    for tick in 0..TICKS {
        move_shape(&mut rig, lift, &lift_pose(tick))?;
        if let Some(script) = change(tick) {
            eval(&mut rig, script)?;
        }
        // The figure walks forward through the bodies, turning gently one way and the other.
        rig.set_global("$mvForwardAction", 1.0);
        rig.set_global("$mvYaw", if (tick / 150) % 2 == 0 { 0.002 } else { -0.003 });
        rig.step();
        observe_server(&rig, &mut h, Control::Figure);
        observe_client(&rig, &mut h);
        let (words, ids) = server_props
            .with(|s| {
                s.world.as_ref().map(|w| {
                    let mut words = w.state_words();
                    words.extend(w.physics().part_words());
                    (words, w.props().keys().copied().collect::<Vec<u32>>())
                })
            })
            .flatten()
            .unwrap_or_default();
        h.u32(words.len() as u32);
        for w in words {
            h.u32(w);
        }
        for id in ids {
            match hierarchy.local(id) {
                Some(local) => {
                    h.floats(&local.translation);
                    h.floats(&local.rotation);
                }
                None => h.bool(false),
            }
        }
        let words = book.with(|b| b.state_words()).unwrap_or_default();
        h.u32(words.len() as u32);
        for w in words {
            h.u32(w);
        }
    }
    hierarchy.audit().map_err(|v| format!("{v:?}"))?;
    let end = figure_at(&rig)?;
    let distance = (0..3)
        .map(|i| (end[i] - start[i]).powi(2))
        .sum::<f32>()
        .sqrt();
    // The body that was deleted is gone and its parts went on alone; every other part and driven prop is still a child.
    let (props, children) = server_props
        .with(|s| {
            s.world.as_ref().map_or((0, 0), |w| {
                (
                    w.len(),
                    w.props().values().filter(|p| p.link.is_child()).count(),
                )
            })
        })
        .unwrap_or((0, 0));
    if props != 4 + 4 + 3 * 3 - 1 || children < 8 {
        return Err(format!("{props} props, {children} of them children"));
    }
    Ok(Outcome {
        hash: h.finish(),
        values: h.count(),
        distance,
        end,
        corrections: rig
            .client
            .borrow()
            .conn
            .conn
            .as_ref()
            .map_or(0, |c| c.predict.stats.corrections),
        driven_ticks: 0,
    })
}
