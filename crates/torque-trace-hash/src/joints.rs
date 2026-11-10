//! The joints scenario (`props-joints`): a server with the props plugin and a predicting client that has it too. The figure is
//! grappled to a point above and ahead of it and, for [`TICKS`] ticks, runs, turns and jumps at the end of the grapple, which
//! holds it to a circle on the floor and swings it back on each jump; around it a crate hangs from a hard tether and one from a soft one, a winch reels a third in and pays it
//! out, and a pair welded together turns on a motored hinge. Each tick the hash takes in the server's figure, the client's
//! prediction, every prop's and joint's state on the server and the client's books of both.
//!
//! The figure's grapple is part of its own step on both sides, so the client's prediction must never be corrected over the run
//! (the ticks before it, while the client learns of the grapple, are not counted): the run fails otherwise.

use torque_plugin_props::{ClientJoints, ClientProps, ServerProps};

use crate::hash::TraceHasher;
use crate::props::{figure_at, props_rig, rig_resource};
use crate::rig::{Control, Rig};
use crate::scenarios::{Outcome, TICKS, observe_client, observe_server};

/// The crate of the scenario, defined before the client is sent the datablocks.
const DATABLOCK: &str = "datablock PropData(TraceCrate) { size = \"0.8 0.8 0.8\"; mass = 15; friction = 0.6; restitution = 0.1; };";

/// The ticks after the grapple is made before the run is counted: the client is sent the joint and its predictions catch up.
const WARM: u32 = 64;

/// The joints the scene makes.
const JOINTS: usize = 6;

/// The scene: the props and their joints, then the figure's grapple.
const SCENE: &[u8] = b"\
    %a = propSpawn(TraceCrate, \"4 6 3\"); jointTether(%a, \"4 6 6\", 2.5); \
    %b = propSpawn(TraceCrate, \"-4 6 3\"); %s = jointTether(%b, \"-4 6 6\", 2); jointSetSpring(%s, 1500, 250); \
    %c = propSpawn(TraceCrate, \"7 2 1\"); $TraceWinch = jointTether(%c, \"7 2 7\", 6); \
    jointWinch($TraceWinch, 2, 2, 20000); \
    %d = propSpawn(TraceCrate, \"-7 2 2\"); %e = propSpawn(TraceCrate, \"-6.2 2 2\"); jointWeld(%d, %e); \
    %h = jointHinge(%d, \"\", \"-7 2 2.4\", \"0 1 0\"); jointSetLimits(%h, -1.2, 1.2); jointSetMotor(%h, 1.5, 4000); \
    $TraceHinge = %h; \
    $TraceGrapple = jointGrapple(TraceFigure SPC \"0 0 1.2\", \"0 2 3\", 3.5);";

fn fold(h: &mut TraceHasher, words: Vec<u32>) {
    h.u32(words.len() as u32);
    for w in words {
        h.u32(w);
    }
}

fn corrections(rig: &Rig) -> u64 {
    rig.client
        .borrow()
        .conn
        .conn
        .as_ref()
        .map_or(0, |c| c.predict.stats.corrections)
}

/// The joints scenario's run (module docs).
pub fn grapple() -> Result<Outcome, String> {
    let (mut rig, client) = props_rig(DATABLOCK)?;
    rig.server
        .vm_mut()
        .eval(SCENE)
        .map_err(|e| format!("{e:?}"))?;
    let server = rig_resource::<ServerProps>(&rig)?;
    let joints = server
        .with(|s| s.world.as_ref().map_or(0, |w| w.joints().len()))
        .unwrap_or(0);
    if joints != JOINTS {
        return Err(format!(
            "{joints} joints made: {:?}",
            rig.server.drain_log()
        ));
    }
    let props = client
        .resource::<ClientProps>()
        .ok_or("the client's props")?
        .clone();
    let book = client
        .resource::<ClientJoints>()
        .ok_or("the client's joints")?
        .clone();
    for _ in 0..WARM {
        rig.step();
    }
    let before = corrections(&rig);
    // The grapple holds the figure near where it began: how far it went is the length of its path.
    let mut at = figure_at(&rig)?;
    let mut distance = 0.0;
    let mut h = TraceHasher::new();
    for tick in 0..TICKS {
        // Forward most of the way, turning one way then the other, a jump now and then; the winch pays out halfway.
        rig.set_global(
            "$mvForwardAction",
            if (tick / 90) % 3 == 2 { 0.0 } else { 1.0 },
        );
        rig.set_global("$mvYaw", if (tick / 120) % 2 == 0 { 0.01 } else { -0.012 });
        rig.set_global(
            "$mvTriggerCount2",
            (2 * (tick / 45) + u32::from(tick % 45 < 2)) as f32,
        );
        if tick == TICKS / 2 {
            rig.server
                .vm_mut()
                .eval(b"jointWinch($TraceWinch, 5, 3); jointSetMotor($TraceHinge, -1.5, 4000);")
                .map_err(|e| format!("{e:?}"))?;
        }
        rig.step();
        let now = figure_at(&rig)?;
        distance += (0..3).map(|i| (now[i] - at[i]).powi(2)).sum::<f32>().sqrt();
        at = now;
        observe_server(&rig, &mut h, Control::Figure);
        observe_client(&rig, &mut h);
        let (prop_words, joint_words) = server
            .with(|s| s.world.as_ref().map(|w| (w.state_words(), w.joint_words())))
            .flatten()
            .unwrap_or_default();
        fold(&mut h, prop_words);
        fold(&mut h, joint_words);
        fold(&mut h, props.with(|b| b.state_words()).unwrap_or_default());
        fold(&mut h, book.with(|b| b.state_words()).unwrap_or_default());
    }
    let end = at;
    let corrected = corrections(&rig) - before;
    if corrected != 0 {
        return Err(format!(
            "the grappled figure's prediction was corrected {corrected} times"
        ));
    }
    let held = book.with(|b| b.len()).unwrap_or(0);
    if held != JOINTS {
        return Err(format!("the client holds {held} joints of {JOINTS}"));
    }
    Ok(Outcome {
        hash: h.finish(),
        values: h.count(),
        distance,
        end,
        corrections: corrected,
        driven_ticks: 0,
    })
}
