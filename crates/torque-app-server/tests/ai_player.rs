//! An AIPlayer steered by its console methods on a server stepped by hand: it walks to the destination it is given and its
//! datablock hears it arrive, it turns to face the place it aims at while it walks elsewhere, `stop` stands it still, and the
//! getters give what the setters set (clamped as Torque clamps them).

use std::net::{Ipv4Addr, SocketAddr};

use torque_app_server::plugins::MinimalPlugins;
use torque_app_server::server::{GameServer, Listen, ServerStart};
use torque_engine::EngineBuilder;
use torque_ts_vm::host::MemFiles;

const SERVER_CS: &str = r#"
datablock StaticShapeData(CubeData) { shapeFile = "kit/cube.dts"; };

datablock PlayerData(FigureData)
{
   shapeFile = "kit/figure.dts";
   mass = 90;
   drag = 0.3;
   maxDamage = 100;
   boundingBox = "1 1 2";
   runForce = 3600;
   maxForwardSpeed = 6;
   maxBackwardSpeed = 6;
   maxSideSpeed = 6;
   maxStepHeight = 0.6;
   runSurfaceAngle = 60;
   jumpForce = 720;
};

function FigureData::onReachDestination(%data, %obj)
{
   $Test::Reached++;
}

function startTest()
{
   new StaticShape(Floor) { datablock = CubeData; position = "0 0 -0.5"; scale = "200 200 1"; };
   new AIPlayer(Bot) { dataBlock = FigureData; position = "0 0 0.1"; };
}
"#;

fn server() -> GameServer {
    let mut builder = EngineBuilder::new();
    builder.add_plugins(MinimalPlugins);
    let engine = builder.build().expect("the engine builds");
    let files = MemFiles::new();
    files.insert("server.cs", SERVER_CS.as_bytes());
    let dts = torque_shapes::write::write_dts;
    files.insert("kit/cube.dts", &dts(&torque_example_assets::shapes::cube()));
    files.insert(
        "kit/figure.dts",
        &dts(&torque_example_assets::shapes::figure()),
    );
    GameServer::start_engine(
        &engine,
        files,
        &ServerStart {
            listen: Listen::Memory(SocketAddr::from((Ipv4Addr::LOCALHOST, 28_000))),
            globals: &[],
            start: "startTest",
            args: &[],
        },
    )
    .expect("the server starts")
}

fn eval(server: &mut GameServer, script: &str) -> String {
    let out = server
        .vm_mut()
        .eval(script.as_bytes())
        .unwrap_or_else(|e| panic!("{script}: {e:?}"));
    String::from_utf8_lossy(&out).into_owned()
}

fn numbers(text: &str) -> Vec<f32> {
    text.split_whitespace()
        .map(|w| w.parse().expect("a number"))
        .collect()
}

fn ticks(server: &mut GameServer, n: u32) {
    for _ in 0..n {
        server.tick();
    }
}

#[test]
fn an_aiplayer_walks_to_its_destination_and_its_datablock_hears_it_arrive() {
    let mut s = server();
    ticks(&mut s, 30);
    let start = numbers(&eval(&mut s, "return Bot.getPosition();"));
    // Without an order it stands.
    assert!(start[0].abs() < 0.01 && start[1].abs() < 0.01, "{start:?}");
    eval(&mut s, "Bot.setMoveDestination(\"8 0 0\", false);");
    assert_eq!(eval(&mut s, "return Bot.getMoveDestination();"), "8 0 0");
    ticks(&mut s, 160);
    let at = numbers(&eval(&mut s, "return Bot.getPosition();"));
    assert!(
        (at[0] - 8.0).abs() < 0.5 && at[1].abs() < 0.5,
        "it walked there: {at:?}"
    );
    assert_eq!(
        eval(&mut s, "return $Test::Reached;"),
        "1",
        "onReachDestination once"
    );

    // Aiming elsewhere while walking: it walks on the line and turns to face the aim.
    eval(
        &mut s,
        "Bot.setAimLocation(\"8 50 1\"); Bot.setMoveDestination(\"0 0 0\");",
    );
    assert_eq!(eval(&mut s, "return Bot.getAimLocation();"), "8 50 1");
    ticks(&mut s, 160);
    let at = numbers(&eval(&mut s, "return Bot.getPosition();"));
    assert!(at[0].abs() < 0.5, "it walked back: {at:?}");
    // Facing +y: the forward vector of its transform's rotation is +y.
    let forward = numbers(&eval(&mut s, "return Bot.getForwardVector();"));
    assert!(forward[1] > 0.95, "it faces where it aims: {forward:?}");

    // Stopped, it stays where it is.
    eval(&mut s, "Bot.setMoveDestination(\"-20 0 0\"); Bot.stop();");
    let before = numbers(&eval(&mut s, "return Bot.getPosition();"));
    ticks(&mut s, 60);
    let after = numbers(&eval(&mut s, "return Bot.getPosition();"));
    assert!((after[0] - before[0]).abs() < 0.05, "{before:?} {after:?}");
}

#[test]
fn the_aiplayers_getters_give_what_its_setters_set() {
    let mut s = server();
    assert_eq!(eval(&mut s, "return Bot.getMoveSpeed();"), "1");
    eval(&mut s, "Bot.setMoveSpeed(2);");
    assert_eq!(
        eval(&mut s, "return Bot.getMoveSpeed();"),
        "1",
        "clamped to 1"
    );
    eval(&mut s, "Bot.setMoveSpeed(0.25);");
    assert_eq!(eval(&mut s, "return Bot.getMoveSpeed();"), "0.25");
    assert_eq!(eval(&mut s, "return Bot.getAimObject();"), "-1");
    eval(&mut s, "Bot.setAimObject(Floor);");
    assert_eq!(
        eval(&mut s, "return Bot.getAimObject() == Floor.getId();"),
        "1"
    );
    eval(&mut s, "Bot.clearAim();");
    assert_eq!(eval(&mut s, "return Bot.getAimObject();"), "-1");
    eval(&mut s, "Bot.setAimObject(NoSuchThing);");
    assert_eq!(eval(&mut s, "return Bot.getAimObject();"), "-1");
}
