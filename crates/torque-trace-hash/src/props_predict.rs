//! The props' prediction scenarios: a server with the props plugin and a client that predicts props too
//! ([`torque_plugin_props::predict`]).
//!
//! - `carry`: the figure holds a crate by a grapple and walks it for [`TICKS`] ticks, turning a little, past two crates it
//!   pushes aside on its way.
//! - `projectile-props`: the figure holds a crate and stands among four others while shots of its own hit the held crate and
//!   the ground near it, every other one an explosion that pushes all around it; it steps forward now and then.
//!
//! Each tick the hash takes in the server's figure, the client's prediction of it, every prop's state on the server, the
//! client's book of the props it was sent, the states its prediction has for the props it predicts and where it draws them:
//! the client's prop prediction must give the same bits on every target.

use torque_engine::EngineBuilder;
use torque_plugin_props::predict::{PredictConfig, PropPrediction, PropPredictionPlugin};
use torque_plugin_props::{ClientProps, PropsPlugin, ServerProps};

use crate::hash::TraceHasher;
use crate::props::{figure_at, props_rig_with, rig_resource};
use crate::rig::{Control, Rig};
use crate::scenarios::{Outcome, TICKS, observe_client, observe_server};

/// The crate and the shots of the scenarios, defined before the client is sent the datablocks.
const DATABLOCKS: &str = "datablock PropData(TraceCrate) { size = \"0.8 0.8 0.8\"; mass = 15; friction = 0.6; restitution = 0.1; };\
    datablock ProjectileData(TraceShot) { muzzleVelocity = 40; lifetime = 2000; };\
    datablock ProjectileData(TraceBlast) { muzzleVelocity = 40; lifetime = 2000; areaImpulse = 300; areaImpulseRadius = 3; };";

/// The ticks after the grapple is made before the run is counted: the client is sent the props and starts predicting.
const WARM: u32 = 64;

/// A rig whose client predicts props, its books and its prediction.
fn rig() -> Result<(Rig, ClientProps, PropPrediction), String> {
    let mut client = EngineBuilder::new();
    client.add_plugins((
        PropsPlugin,
        PropPredictionPlugin {
            // Torque's ProjectileData record carries no area impulse: the client is told the blast's.
            config: PredictConfig {
                shot: torque_plugin_props::hits::ShotPush::of(None, Some(300.0), Some(3.0)),
                ..PredictConfig::default()
            },
        },
    ));
    let client = client.build().map_err(|e| e.to_string())?;
    let book = client
        .resource::<ClientProps>()
        .ok_or("the client's book")?
        .clone();
    let prediction = client
        .resource::<PropPrediction>()
        .ok_or("the client's prediction")?
        .clone();
    let rig = props_rig_with(DATABLOCKS, &client)?;
    Ok((rig, book, prediction))
}

fn fold(h: &mut TraceHasher, words: Vec<u32>) {
    h.u32(words.len() as u32);
    for w in words {
        h.u32(w);
    }
}

/// One tick's words: the figure on both sides, the server's props, the client's book, its predicted states and drawn poses.
fn observe(
    rig: &Rig,
    h: &mut TraceHasher,
    server: &ServerProps,
    book: &ClientProps,
    prediction: &PropPrediction,
) {
    observe_server(rig, h, Control::Figure);
    observe_client(rig, h);
    fold(
        h,
        server
            .with(|s| s.world.as_ref().map(|w| w.state_words()))
            .flatten()
            .unwrap_or_default(),
    );
    fold(h, book.with(|b| b.state_words()).unwrap_or_default());
    let predicted: Vec<u32> = book
        .with(|b| {
            b.props()
                .iter()
                .filter_map(|(&g, p)| Some((g, p.predicted?)))
                .flat_map(|(g, q)| std::iter::once(g).chain(q.words().map(|w| w as u32)))
                .collect()
        })
        .unwrap_or_default();
    fold(h, predicted);
    let drawn: Vec<u32> = prediction
        .drawn()
        .iter()
        .flat_map(|(g, p, q)| {
            std::iter::once(*g)
                .chain(p.iter().map(|v| v.to_bits()))
                .chain(q.iter().map(|v| v.to_bits()))
        })
        .collect();
    fold(h, drawn);
}

/// The run's outcome; an error when the client predicted nothing.
fn outcome(
    rig: &Rig,
    h: &TraceHasher,
    start: [f32; 3],
    prediction: &PropPrediction,
) -> Result<Outcome, String> {
    let stats = prediction.stats();
    if stats.restores == 0 || stats.steps == 0 {
        return Err(format!("the client predicted no prop: {stats:?}"));
    }
    let end = figure_at(rig)?;
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
    Ok(Outcome {
        hash: h.finish(),
        values: h.count(),
        distance,
        end,
        corrections,
        driven_ticks: 0,
    })
}

/// The `carry` scenario (module docs).
pub fn carry() -> Result<Outcome, String> {
    let (mut rig, book, prediction) = rig()?;
    rig.server
        .vm_mut()
        .eval(
            b"%c = propSpawn(TraceCrate, \"0 -2.5 0.4\"); jointGrapple(TraceFigure SPC \"0 0 1\", %c, 2.5); \
              propSpawn(TraceCrate, \"0.4 12 0.4\"); propSpawn(TraceCrate, \"-0.6 20 0.4\");",
        )
        .map_err(|e| format!("{e:?}"))?;
    let server = rig_resource::<ServerProps>(&rig)?;
    for _ in 0..WARM {
        rig.step();
    }
    let start = figure_at(&rig)?;
    let mut h = TraceHasher::new();
    for tick in 0..TICKS {
        rig.set_global("$mvForwardAction", if tick % 200 < 170 { 1.0 } else { 0.0 });
        rig.set_global("$mvYaw", if (tick / 120) % 2 == 0 { 0.003 } else { -0.004 });
        rig.step();
        observe(&rig, &mut h, &server, &book, &prediction);
    }
    outcome(&rig, &h, start, &prediction)
}

/// The `projectile-props` scenario (module docs).
pub fn shots() -> Result<Outcome, String> {
    let (mut rig, book, prediction) = rig()?;
    rig.server
        .vm_mut()
        .eval(
            b"$TraceHeld = propSpawn(TraceCrate, \"0 2 0.4\"); jointGrapple(TraceFigure SPC \"0 0 1\", $TraceHeld, 2.5); \
              for (%i = 0; %i < 4; %i++) propSpawn(TraceCrate, (%i - 1.5) * 1.6 SPC \"4 0.4\");",
        )
        .map_err(|e| format!("{e:?}"))?;
    let server = rig_resource::<ServerProps>(&rig)?;
    for _ in 0..WARM {
        rig.step();
    }
    let start = figure_at(&rig)?;
    let mut h = TraceHasher::new();
    for tick in 0..TICKS {
        rig.set_global("$mvForwardAction", if tick % 150 < 20 { 1.0 } else { 0.0 });
        if tick % 25 == 0 {
            // From far off, at the held crate or the ground beside it, by turns a plain shot and a blast.
            let data = if (tick / 25) % 2 == 0 {
                "TraceShot"
            } else {
                "TraceBlast"
            };
            let side = [0.0, 0.6, -0.6, 1.4][(tick / 25 % 4) as usize];
            let code = format!(
                "%at = $TraceHeld.getPosition(); \
                 new Projectile() {{ dataBlock = {data}; initialPosition = getWord(%at, 0) + {side} SPC \"30 0.5\"; \
                 initialVelocity = \"0 -40 0\"; sourceObject = TraceFigure.getId(); }};"
            );
            rig.server
                .vm_mut()
                .eval(code.as_bytes())
                .map_err(|e| format!("{e:?}"))?;
        }
        rig.step();
        observe(&rig, &mut h, &server, &book, &prediction);
    }
    if prediction.stats().shot_hits == 0 {
        return Err(format!(
            "no shot of the figure's met a predicted crate: {:?}",
            prediction.stats()
        ));
    }
    outcome(&rig, &h, start, &prediction)
}
