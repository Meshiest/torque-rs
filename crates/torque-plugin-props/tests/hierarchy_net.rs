//! Props in the scene hierarchy over the engine's own protocol: a client draws a child prop on its parent (the prop's record carries
//! no update for the parent's motion), never as resting while its parent moves, and its prediction finds a child where the
//! parent's latest pose and its local pose put it and leaves the prop and its parents to the server.

#[path = "net/rig.rs"]
mod rig;

use rig::{PeerProps, Rig, RigProps, files};
use torque_app::GameFiles;
use torque_app::config::{EngineConfig, GameConfig};
use torque_app::plugins::DefaultPlugins;
use torque_engine::{Engine, EngineBuilder, ProtocolKind};
use torque_plugin_props::PropsPlugin;

fn engine(files: &GameFiles) -> Engine {
    let config = GameConfig::load(files).expect("game.toml");
    let mut builder = EngineBuilder::new();
    builder.add_plugins(DefaultPlugins).config(&config);
    torque_app_server::plugins::set_protocol(&mut builder, ProtocolKind::Engine);
    builder.add_plugins(PropsPlugin);
    builder.build().expect("the engine builds")
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>().sqrt()
}

/// Moves a static shape; its `setTransform` marks the record's position.
fn move_to(rig: &mut Rig, id: u32, pose: &str) {
    rig.eval(&format!("{id}.setTransform(\"{pose}\");"));
}

fn slot_of(rig: &Rig, id: u32) -> u16 {
    rig.props()
        .with(|s| s.world.as_ref().and_then(|w| w.get(id)).map(|p| p.slot))
        .flatten()
        .expect("a prop")
}

#[test]
fn a_client_finds_a_child_prop_where_its_parent_puts_it_and_never_draws_it_resting() {
    let files = files();
    let mut rig = Rig::with_engine(&files, &[], engine(&files));
    rig.join(&files, engine(&files), 50_001, &[]);
    assert!(rig.run_until(400, |r| r.peers[0].ghosts() >= 1));
    let platform: u32 = rig
        .eval("return new StaticShape() { dataBlock = FloorData; position = \"0 12 5\"; scale = \"4 4 0.5\"; };")
        .parse()
        .expect("a platform");
    let rider: u32 = rig
        .eval("return propSpawn(Crate, \"0 12 5.75\");")
        .parse()
        .expect("a prop");
    // A root on the floor with a part at its side, put to sleep as a map's props are.
    let root: u32 = rig
        .eval("return propSpawn(Crate, \"10 0 0.5\");")
        .parse()
        .expect("a prop");
    let part: u32 = rig
        .eval("return propSpawn(Crate, \"11 0 0.5\");")
        .parse()
        .expect("a prop");
    assert_eq!(
        rig.eval(&format!("return {rider}.setParent({platform});")),
        "1"
    );
    assert_eq!(rig.eval(&format!("return {part}.setParent({root});")), "1");
    let (rider_slot, root_slot, part_slot) = (
        slot_of(&rig, rider),
        slot_of(&rig, root),
        slot_of(&rig, part),
    );
    let mut resting_while_moving = 0;
    let mut worst = 0.0f32;
    let mut checked = 0;
    for t in 0..300u32 {
        // The platform slides and turns, then stands still for the last hundred ticks.
        let moving = t < 200;
        let t = t.min(200) as f32;
        move_to(
            &mut rig,
            platform,
            &format!("{} {} 5 0 0 1 {}", t * 0.05, 12.0 + t * 0.02, t * 0.004),
        );
        rig.step();
        let book = rig.peers[0].book();
        let b = book.0.borrow();
        let Some(p) = b.props().values().find(|p| p.slot == rider_slot) else {
            continue;
        };
        if moving && t > 20.0 {
            resting_while_moving += usize::from(p.asleep || p.frozen);
        }
        if !moving {
            // Standing still: the client's tick pose is the server's within a few millimeters of the wire's steps.
            let server = rig
                .props()
                .with(|s| {
                    s.world
                        .as_ref()
                        .and_then(|w| w.get(rider))
                        .map(|p| p.q.position())
                })
                .flatten()
                .expect("the server's rider");
            worst = worst.max(distance(p.now().position(), server));
            assert!(p.place.child, "the client found the rider's parent");
            checked += 1;
        }
    }
    assert!(
        checked >= 90,
        "the rider reached the client ({checked} ticks checked)"
    );
    assert_eq!(
        resting_while_moving, 0,
        "ticks a child of a moving parent was drawn as resting"
    );
    assert!(
        worst < 5e-3,
        "the client's pose of the child was {worst} from the server's"
    );
    // Let the pair on the floor settle and sleep: the root rests, its part does not, and both are in the hierarchy.
    assert!(rig.run_until(400, |r| {
        let book = r.peers[0].book();
        let b = book.0.borrow();
        b.props()
            .values()
            .find(|p| p.slot == root_slot)
            .is_some_and(|p| p.asleep && p.place.carries)
    }));
    let book = rig.peers[0].book();
    let b = book.0.borrow();
    let part_prop = b
        .props()
        .values()
        .find(|p| p.slot == part_slot)
        .expect("the part");
    assert!(!part_prop.asleep && part_prop.place.child && part_prop.is_linked());
    // A prediction neither simulates a child nor the prop that carries it.
    assert!(b.props().values().filter(|p| p.is_linked()).count() >= 2);
}
