//! The walkthrough's rope plugin, built step by step in the book's "Writing a plugin" chapter: a rope between two points that a
//! script makes from a datablock, simulated once per client tick and drawn as lines. This program makes a client, runs a
//! script, steps the client for three seconds and prints how far the rope sags. `torque-plugin-rope` is the full plugin (a
//! lit tube, shadows, stiffness, anchors on objects, caps and hostile-input checks).

mod data;
mod natives;
mod pass;
mod plugin;
mod sim;

// ANCHOR: main
use torque_app::plugins::DefaultPlugins;
use torque_app::{ClientBuilder, ClientNet, GameServer};
use torque_engine::EngineBuilder;

use crate::plugin::RopePlugin;
use crate::sim::Ropes;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = EngineBuilder::new()
        .add_plugins((DefaultPlugins, RopePlugin))
        .build()?;
    let mut builder = ClientBuilder::from_engine(&engine, None, ClientNet::None)?;
    builder.vm.eval(
        br#"datablock RopeData(Cable) { segments = 16; length = 9; };
            ropeCreate("-4 0 3", "4 0 3", Cable);"#,
    )?;
    let mut client = builder.build::<GameServer>();
    client.step(3000);

    let ropes = engine.resource::<Ropes>().ok_or("the rope plugin")?;
    let ropes = ropes.lock();
    let lowest = ropes[0]
        .points
        .iter()
        .map(|p| p[2])
        .fold(f32::MAX, f32::min);
    println!(
        "{} rope, its lowest point {lowest:.2} (the ends are at 3)",
        ropes.len()
    );
    assert!(lowest < 1.5);
    Ok(())
}
// ANCHOR_END: main
