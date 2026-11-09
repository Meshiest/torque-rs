//! The props scenario (`props-drop`): a server with the props plugin and a predicting client that has it too. 64 crates are spawned
//! over the floor in front of the figure (four layers of sixteen, falling onto the floor and onto each other), then the figure
//! walks through them for [`TICKS`] ticks, pushing them, while the scripted keys turn it a little. Each tick the hash takes in the
//! server's figure, the client's prediction, every prop's state on the server (its physics world, at the wire's precision) and the
//! client's book of the props it was sent.

use torque_engine::{ClientStage, EngineBuilder};
use torque_plugin_props::{ClientProps, PropsPlugin, ServerProps};

use crate::hash::TraceHasher;
use crate::rig::{Control, Mode, Rig};
use crate::scenarios::{Outcome, TICKS, observe_client, observe_server};

/// The most ticks the scenario waits for the connection and the prediction.
const SETTLE_LIMIT: u32 = 400;

/// The crate the scenario spawns, defined before the client is sent the datablocks.
const DATABLOCK: &str = "datablock PropData(TraceCrate) { size = \"0.8 0.8 0.8\"; mass = 15; friction = 0.6; restitution = 0.1; };";

/// A rig of a server with the props plugin and a predicting client that has it too (the client's half replayed from an engine of
/// the plugin, the props' collision source in its prediction), `datablocks` defined before the client is sent them, settled into
/// the client's prediction. The client's engine is returned with it (its resources are the client's books).
pub(crate) fn props_rig(datablocks: &str) -> Result<(Rig, torque_engine::Engine), String> {
    // The client's half of the plugin, from an engine of the same plugin (the classes and the book are derived from the names
    // alone), and the props' collision source in its prediction.
    let client = EngineBuilder::new()
        .add_plugins(PropsPlugin)
        .build()
        .map_err(|e| e.to_string())?;
    let mut rig = Rig::with_client(Mode::Tge, PropsPlugin, Control::Figure, |vm| {
        for &stage in ClientStage::ALL {
            client.replay(stage, vm).map_err(|e| e.to_string())?;
        }
        let sources: Vec<_> = client
            .list::<torque_collide::source::PluginCollision>()
            .to_vec();
        let warnings = torque_game::plugin_collision::install(
            vm,
            &sources,
            torque_collide::source::Side::Client,
        );
        if warnings.is_empty() {
            Ok(())
        } else {
            Err(warnings.join("; "))
        }
    })?;
    rig.server
        .vm_mut()
        .eval(datablocks.as_bytes())
        .map_err(|e| format!("{e:?}"))?;
    if !rig.settle(SETTLE_LIMIT) {
        return Err(format!(
            "the client never predicted its control object in {SETTLE_LIMIT} ticks: {:?} {:?}",
            rig.server.drain_log(),
            rig.client_vm
                .drain_log()
                .iter()
                .map(|l| String::from_utf8_lossy(&l.text).into_owned())
                .filter(|l| !l.starts_with("onDataBlockObjectReceived"))
                .collect::<Vec<_>>()
        ));
    }
    Ok((rig, client))
}

/// The props scenario's run (module docs).
pub fn drop() -> Result<Outcome, String> {
    let (mut rig, client) = props_rig(DATABLOCK)?;
    // Four layers of sixteen crates over the figure's way, a little apart and a little turned, falling onto the floor and each other.
    rig.server
        .vm_mut()
        .eval(
            b"for (%i = 0; %i < 64; %i++) { \
                %x = (%i % 4) * 0.9 - 1.35 + (mFloor(%i / 16) % 2) * 0.2; \
                %y = mFloor((%i % 16) / 4) * 0.9 + 4; \
                %z = 0.5 + mFloor(%i / 16) * 0.85; \
                propSpawn(TraceCrate, %x SPC %y SPC %z SPC \"0 0 1\" SPC (%i % 7) * 0.05); \
              }",
        )
        .map_err(|e| format!("{e:?}"))?;
    let server_props = rig_resource::<ServerProps>(&rig)?;
    let book = client
        .resource::<ClientProps>()
        .ok_or("the client's book")?
        .clone();
    let start = figure_at(&rig)?;
    let mut h = TraceHasher::new();
    for tick in 0..TICKS {
        // Forward the whole way, turning gently one way, then the other.
        rig.set_global("$mvForwardAction", 1.0);
        rig.set_global("$mvYaw", if (tick / 150) % 2 == 0 { 0.002 } else { -0.003 });
        rig.step();
        observe_server(&rig, &mut h, Control::Figure);
        observe_client(&rig, &mut h);
        let words = server_props
            .with(|s| s.world.as_ref().map(|w| w.state_words()))
            .flatten()
            .unwrap_or_default();
        h.u32(words.len() as u32);
        for w in words {
            h.u32(w);
        }
        let words = book.with(|b| b.state_words()).unwrap_or_default();
        h.u32(words.len() as u32);
        for w in words {
            h.u32(w);
        }
    }
    let end = figure_at(&rig)?;
    let distance = (0..3)
        .map(|i| (end[i] - start[i]).powi(2))
        .sum::<f32>()
        .sqrt();
    let corrections = rig
        .client
        .borrow()
        .conn
        .conn
        .as_ref()
        .map_or(0, |c| c.predict.stats.corrections);
    let props = server_props
        .with(|s| s.world.as_ref().map_or(0, |w| w.len()))
        .unwrap_or(0);
    let held = book.with(|b| b.len()).unwrap_or(0);
    if props != 64 || held != 64 {
        return Err(format!("{props} props on the server, {held} on the client"));
    }
    Ok(Outcome {
        hash: h.finish(),
        values: h.count(),
        distance,
        end,
        corrections,
        driven_ticks: 0,
    })
}

pub(crate) fn figure_at(rig: &Rig) -> Result<[f32; 3], String> {
    let vm = rig.server.vm();
    vm.objects()
        .find(b"TraceFigure")
        .and_then(|id| torque_game::sim::player::position(vm, id))
        .ok_or_else(|| "the server has no figure".to_string())
}

/// A resource of the rig's server's engine.
pub(crate) fn rig_resource<T: Clone + 'static>(rig: &Rig) -> Result<T, String> {
    rig.engine
        .resource::<T>()
        .map(|r| (*r).clone())
        .ok_or_else(|| format!("no {}", std::any::type_name::<T>()))
}
