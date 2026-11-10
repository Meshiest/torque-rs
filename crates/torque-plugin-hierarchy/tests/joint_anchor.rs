//! A joint's anchor on a child of the scene hierarchy is where the hierarchy puts the child, the server's tick hook reading it
//! while the tree still has work to do: a grapple from a Player hung on a moving platform.

use std::net::{Ipv4Addr, SocketAddr};

use torque_app_server::plugins::{MinimalPlugins, set_protocol};
use torque_app_server::server::{GameServer, Listen, ServerStart};
use torque_engine::ProtocolKind;
use torque_plugin_props::{PropsPlugin, ServerProps};
use torque_ts_vm::host::MemFiles;

const SERVER_CS: &str = r#"
datablock StaticShapeData(MarkerData)
{
};

datablock PlayerData(FigureData)
{
   shapeFile = "kit/figure.dts";
   mass = 90;
   drag = 0.3;
   maxDamage = 100;
   boundingBox = "1 1 2";
   runForce = 3600;
   maxForwardSpeed = 6;
   maxBackwardSpeed = 4;
   maxSideSpeed = 5;
   maxStepHeight = 0.6;
   runSurfaceAngle = 60;
   jumpForce = 720;
   jumpSurfaceAngle = 70;
   minImpactSpeed = 40;
};

function startTest()
{
   // A floor 400 units square with its top at z = 0.
   new StaticShape(Floor) { datablock = MarkerData; position = "0 0 -0.5"; scale = "400 400 1"; };
}
"#;

const ANCHOR: [f32; 3] = [30.0, 0.0, 5.0];

fn eval(server: &mut GameServer, script: &str) -> String {
    let out = server
        .vm_mut()
        .eval(script.as_bytes())
        .unwrap_or_else(|e| panic!("{script}: {e:?}"));
    String::from_utf8_lossy(&out).into_owned()
}

/// The platform's pose at tick `t`: it slides and turns by whole steps, so every tick's pose differs.
fn platform_pose(t: u32) -> String {
    let t = t as f32;
    format!("{} {} 0 0 0 1 {}", t * 0.1, t * 0.03, t * 0.01)
}

#[test]
fn a_grapple_from_a_player_on_a_moving_platform_measures_from_where_the_platform_puts_it() {
    let mut builder = torque_engine::EngineBuilder::new();
    builder.add_plugins(MinimalPlugins).add_plugins(PropsPlugin);
    set_protocol(&mut builder, ProtocolKind::Engine);
    let engine = builder.build().expect("the engine builds");
    let props = engine.resource::<ServerProps>().expect("the props").clone();
    let files = MemFiles::new();
    files.insert("server.cs", SERVER_CS.as_bytes());
    files.insert(
        "kit/figure.dts",
        &torque_shapes::write::write_dts(&torque_example_assets::shapes::figure()),
    );
    let mut s = GameServer::start_engine(
        &engine,
        files,
        &ServerStart {
            listen: Listen::Memory(SocketAddr::from((Ipv4Addr::LOCALHOST, 28_000))),
            globals: &[],
            start: "startTest",
            args: &[],
        },
    )
    .expect("the server starts");
    let platform: u32 = eval(
        &mut s,
        "return new StaticShape() { datablock = MarkerData; position = \"0 0 0\"; };",
    )
    .parse()
    .expect("a platform");
    let player: u32 = eval(
        &mut s,
        "return new Player() { datablock = FigureData; position = \"2 0 1\"; };",
    )
    .parse()
    .expect("a player");
    assert_eq!(
        eval(&mut s, &format!("return {player}.setParent({platform});")),
        "1"
    );
    let [x, y, z] = ANCHOR;
    let joint: i32 = eval(
        &mut s,
        &format!("return jointGrapple({player}, \"{x} {y} {z}\", 100);"),
    )
    .parse()
    .expect("a joint");
    assert!(joint > 0, "the grapple was refused");
    let distance = |s: &mut GameServer| -> f32 {
        eval(s, &format!("return jointGetDistance({joint});"))
            .parse()
            .expect("a distance")
    };
    let mut good = 0;
    for t in 0..600 {
        eval(
            &mut s,
            &format!("{platform}.setTransform(\"{}\");", platform_pose(t)),
        );
        s.tick();
        // The platform moves again after the tick's own resolve, and the props' hook runs with the tree holding that work: its
        // anchor for the player is still where the platform puts the player.
        eval(
            &mut s,
            &format!("{platform}.setTransform(\"{}\");", platform_pose(t + 1)),
        );
        torque_plugin_props::tick::tick(s.vm_mut(), &props).expect("the props tick");
        torque_game::scene::links::resolve(s.vm_mut());
        let at = torque_game::sim::player::position(s.vm(), player).expect("the player");
        let expected = (0..3)
            .map(|k| (at[k] - ANCHOR[k]).powi(2))
            .sum::<f32>()
            .sqrt();
        good += usize::from((distance(&mut s) - expected).abs() < 1e-3);
    }
    assert_eq!(
        good, 600,
        "ticks whose distance was measured from the resolved player"
    );
}
