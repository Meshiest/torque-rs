//! Prop prediction over the memory transport with about 100 ms there and back and 2% of the datagrams lost: a held prop on a
//! long walk, a Player riding a sliding prop and pushing a row, predicted impulses (one the server makes too, one it never
//! does, and the caps), and the client's own shots.

use crate::rig;

use rig::{Link, PeerProps, Rig, RigProps, engine, files};
use torque_app::GameFiles;
use torque_engine::{Engine, EngineBuilder};
use torque_plugin_props::hits::ShotPush;
use torque_plugin_props::predict::{
    PredictConfig, PredictStats, PropPrediction, PropPredictionPlugin,
};
use torque_plugin_props::{ClientProps, PropsPlugin};

/// An engine of the default plugins, the props and their prediction of `config`.
fn predicting(files: &GameFiles, config: PredictConfig) -> Engine {
    use torque_app::config::EngineConfig;
    let game = torque_app::config::GameConfig::load(files).expect("game.toml");
    let mut builder = EngineBuilder::new();
    builder
        .add_plugins(torque_app::plugins::DefaultPlugins)
        .config(&game);
    builder.add_plugins((PropsPlugin, PropPredictionPlugin { config }));
    builder.build().expect("the engine builds")
}

/// A server with a figure for its one client, connected and ghosted, then the lossy link.
fn setup(predict: bool) -> (Rig, u32) {
    setup_with(predict.then(PredictConfig::default))
}

/// [`setup`] with the client predicting by `config` (none: not predicting).
fn setup_with(config: Option<PredictConfig>) -> (Rig, u32) {
    let files = files();
    let mut rig = Rig::new(&files, &[("$Test::Players", "1")]);
    let client = match config {
        Some(c) => predicting(&files, c),
        None => engine(&files, true),
    };
    rig.join(&files, client, 50_001, &[]);
    assert!(rig.run_until(400, |r| r.peers[0].ghosts() >= 2));
    let player: u32 = rig
        .eval("return ClientGroup.getObject(0).getControlObject();")
        .parse()
        .expect("a player");
    rig.link = Link::lossy();
    (rig, player)
}

fn prediction(rig: &Rig) -> PropPrediction {
    rig.peers[0]
        .engine
        .resource::<PropPrediction>()
        .expect("the prediction")
        .clone()
}

fn book(rig: &Rig) -> ClientProps {
    rig.peers[0].book()
}

/// The client's ghost of the server's prop `id` (by its slot).
fn ghost_of(rig: &Rig, id: u32) -> Option<u32> {
    let slot = rig
        .props()
        .with(|s| s.world.as_ref()?.get(id).map(|p| p.slot))
        .flatten()?;
    book(rig).with(|b| {
        b.props()
            .iter()
            .find_map(|(&g, p)| (p.slot == slot).then_some(g))
    })?
}

fn drawn_at(p: &PropPrediction, ghost: u32) -> Option<[f32; 3]> {
    p.drawn().iter().find(|d| d.0 == ghost).map(|d| d.1)
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|k| a[k] - b[k])
}

fn len(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// What a run of a drawn prop came to: the largest change of its drawn step from one tick to the next (a snap shows as one)
/// and the same of the server's own prop (its own motion: a tether's yank shows there too), and the prediction's counters.
#[derive(Debug)]
struct Run {
    jerk: f32,
    server_jerk: f32,
    stats: PredictStats,
    predicted_ticks: u32,
}

/// The largest change of a path's step from one tick to the next.
#[derive(Default)]
struct Jerk {
    last: Option<[f32; 3]>,
    step: Option<[f32; 3]>,
    max: f32,
}

impl Jerk {
    fn at(&mut self, at: Option<[f32; 3]>) {
        let Some(at) = at else {
            *self = Jerk {
                max: self.max,
                ..Jerk::default()
            };
            return;
        };
        if let Some(l) = self.last {
            let s = sub(at, l);
            if let Some(prev) = self.step {
                self.max = self.max.max(len(sub(s, prev)));
            }
            self.step = Some(s);
        }
        self.last = Some(at);
    }
}

/// Steps `ticks` ticks with the client's keys `keys(tick)` (forward, yaw), watching how the server's prop `id` (the client's
/// ghost `ghost`) is drawn.
fn walk(rig: &mut Rig, id: u32, ghost: u32, ticks: u32, keys: impl Fn(u32) -> (f32, f32)) -> Run {
    let p = prediction(rig);
    let (mut drawn, mut server) = (Jerk::default(), Jerk::default());
    let mut predicted_ticks = 0;
    for t in 0..ticks {
        let (forward, yaw) = keys(t);
        let vm = rig.peers[0].client.vm_mut();
        vm.set_global("$mvForwardAction", &forward.to_string());
        vm.set_global("$mvYaw", &yaw.to_string());
        rig.step();
        server.at(server_at(rig, id));
        let at = drawn_at(&p, ghost);
        predicted_ticks += u32::from(at.is_some());
        drawn.at(at);
    }
    Run {
        jerk: drawn.max,
        server_jerk: server.max,
        stats: p.stats(),
        predicted_ticks,
    }
}

fn server_at(rig: &Rig, id: u32) -> Option<[f32; 3]> {
    rig.props()
        .with(|s| s.world.as_ref()?.get(id).map(|p| p.q.position()))
        .flatten()
}

/// The client's own Player's corrections so far.
fn player_corrections(rig: &Rig) -> u64 {
    rig.peers[0]
        .client
        .host
        .client
        .borrow()
        .conn
        .conn
        .as_ref()
        .map_or(0, |c| c.predict.stats.corrections)
}

fn number(rig: &mut Rig, code: &str) -> u32 {
    rig.eval(code).parse().unwrap_or(0)
}

/// Runs client script `code` and gives its result.
fn client_eval(rig: &mut Rig, code: &str) -> String {
    rig.peers[0]
        .client
        .vm_mut()
        .eval(code.as_bytes())
        .map(|v| String::from_utf8_lossy(&v).into_owned())
        .unwrap_or_else(|e| panic!("{code}: {e:?}"))
}

/// A crate spawned at `at`, held by the player's grapple of `length`, known to the client and predicted: its id and ghost.
fn held_crate(rig: &mut Rig, player: u32, at: &str, length: f32) -> (u32, u32) {
    let id = number(rig, &format!("return propSpawn(Crate, \"{at}\");"));
    let grapple = number(
        rig,
        &format!("return jointGrapple({player} SPC \"0 0 1\", {id}, {length});"),
    );
    assert!(id > 0 && grapple > 0, "{:?}", rig.server.drain_log());
    assert!(rig.run_until(200, |r| {
        ghost_of(r, id).is_some_and(|g| prediction(r).is_predicted(g))
    }));
    rig.run(40);
    (id, ghost_of(rig, id).expect("its ghost"))
}

/// The mean of the corrections between two counters.
fn mean_between(a: &PredictStats, b: &PredictStats) -> f64 {
    let n = b.corrections - a.corrections;
    if n == 0 {
        0.0
    } else {
        (b.correction_sum - a.correction_sum) / n as f64
    }
}

#[test]
fn a_held_prop_on_a_long_walk_is_drawn_smoothly_where_it_belongs() {
    let (mut rig, player) = setup(true);
    let (id, ghost) = held_crate(&mut rig, player, "0 -10 0.5", 2.5);
    let run = walk(&mut rig, id, ghost, 600, |t| {
        (1.0, if (t / 150) % 2 == 0 { 0.002 } else { -0.002 })
    });
    println!("carry: {run:?}");
    assert!(run.predicted_ticks >= 590, "{run:?}");
    let server = server_at(&rig, id).expect("the crate");
    assert!(server[1] > 40.0, "the crate was dragged along: {server:?}");
    // Drawn with no snap: its step changes from tick to tick by no more than the server's own prop's does, and 5 cm.
    assert!(run.jerk <= run.server_jerk + 0.05, "{run:?}");
    assert!(run.stats.mean_correction() <= 0.03, "{run:?}");
}

/// A run of a player riding a sliding raft, then pushing a row of four crates: the client's own Player's corrections and,
/// when it predicts props, the prediction's counters over the run.
fn ride_and_push(predict: bool) -> (u64, Option<PredictStats>) {
    let (mut rig, player) = setup(predict);
    let raft = number(&mut rig, "return propSpawn(Raft, \"0 -8 0.2\");");
    rig.eval(&format!("{player}.setTransform(\"0 -8 0.5\");"));
    for k in 0..4u8 {
        rig.eval(&format!(
            "propSpawn(Crate, \"{} -2 0.5\");",
            f32::from(k) * 1.1 - 1.65
        ));
    }
    rig.run(30);
    rig.eval(&format!("propSetVelocity({raft}, \"0.6 0 0\");"));
    let before = player_corrections(&rig);
    let start = predict.then(|| prediction(&rig).stats());
    for t in 0..360u32 {
        let forward = if t < 90 { 0.0 } else { 1.0 };
        rig.peers[0]
            .client
            .vm_mut()
            .set_global("$mvForwardAction", &forward.to_string());
        rig.step();
    }
    let pushed = rig
        .props()
        .with(|s| {
            s.world.as_ref().map_or(0, |w| {
                w.props()
                    .values()
                    .filter(|p| p.q.position()[1] > -1.5)
                    .count()
            })
        })
        .unwrap_or(0);
    assert!(pushed >= 2, "the row was pushed: {pushed}");
    let stats = start.map(|a| {
        let b = prediction(&rig).stats();
        PredictStats {
            corrections: b.corrections - a.corrections,
            correction_sum: b.correction_sum - a.correction_sum,
            ..b
        }
    });
    (player_corrections(&rig) - before, stats)
}

#[test]
fn riding_a_sliding_prop_and_pushing_a_row_is_predicted_no_worse_than_not_predicting() {
    let (interpolated, _) = ride_and_push(false);
    let (predicted, stats) = ride_and_push(true);
    let stats = stats.expect("the prediction's counters");
    println!(
        "ride and push: player corrections {predicted} (interpolated {interpolated}), {stats:?}"
    );
    assert!(
        stats.restores > 0 && stats.mean_correction() <= 0.03,
        "{stats:?}"
    );
    assert!(
        predicted <= interpolated,
        "the player was corrected {predicted} times predicting props, {interpolated} without"
    );
}

#[test]
fn a_predicted_impulse_the_server_makes_too_moves_at_once_and_settles() {
    let (mut rig, player) = setup(true);
    let (id, ghost) = held_crate(&mut rig, player, "0 -10 0.5", 2.5);
    let p = prediction(&rig);
    let before = drawn_at(&p, ghost).expect("drawn");
    assert_eq!(
        client_eval(&mut rig, "return propPredicted();"),
        ghost.to_string()
    );
    let call = format!("return propPredictImpulse({ghost}, \"0 -10 0.5\", \"0 0 120\");");
    assert_eq!(client_eval(&mut rig, &call), "1");
    rig.step();
    let now = drawn_at(&p, ghost).expect("drawn");
    assert!(
        now[2] > before[2] + 0.1,
        "it moved at once: {before:?} -> {now:?}"
    );
    // The server's push for the same cause comes when the move that caused it reaches it.
    rig.eval(&format!("propApplyImpulse({id}, \"0 0 120\");"));
    let start = p.stats();
    rig.run(60);
    let end = p.stats();
    let mean = mean_between(&start, &end);
    println!("matched impulse: mean correction {mean}, {end:?}");
    assert!(mean <= 0.03, "{start:?} {end:?}");
}

#[test]
fn a_predicted_impulse_the_server_never_makes_eases_out_with_no_pop() {
    let (mut rig, player) = setup(true);
    let (id, ghost) = held_crate(&mut rig, player, "0 -10 0.5", 2.5);
    let p = prediction(&rig);
    assert!(p.impulse(ghost, [0.0, -10.0, 0.5], [0.0, 0.0, 120.0]));
    let mut jerk = Jerk::default();
    let mut highest = 0.0f32;
    for _ in 0..40 {
        rig.step();
        let at = drawn_at(&p, ghost);
        highest = highest.max(at.map_or(0.0, |a| a[2]));
        jerk.at(at);
    }
    let server = server_at(&rig, id).expect("the crate");
    let drawn = drawn_at(&p, ghost).expect("drawn");
    println!(
        "unmatched impulse: highest {highest}, jerk {}, drawn {drawn:?}, server {server:?}",
        jerk.max
    );
    assert!(highest > 0.7, "it was shown rising");
    // Back where the server has it within the window, never jumping there.
    assert!(len(sub(drawn, server)) < 0.05, "{drawn:?} {server:?}");
    assert!(jerk.max < 0.3, "{}", jerk.max);
}

#[test]
fn predicted_impulses_are_capped_and_never_sent() {
    let (mut rig, player) = setup(true);
    let (id, ghost) = held_crate(&mut rig, player, "0 -10 0.5", 2.5);
    let far = number(&mut rig, "return propSpawn(Crate, \"20 20 0.5\");");
    assert!(rig.run_until(100, |r| ghost_of(r, far).is_some()));
    let far = ghost_of(&rig, far).expect("its ghost");
    let p = prediction(&rig);
    let at = [0.0, -10.0, 0.5];
    // Not predicted, not finite, out of the world, a blast that pushes nothing: refused.
    assert!(!p.impulse(far, at, [0.0, 0.0, 1.0]));
    assert!(!p.impulse(ghost, at, [f32::NAN, 0.0, 1.0]));
    assert!(!p.impulse(ghost, [1.0e9, 0.0, 0.0], [0.0, 0.0, 1.0]));
    assert!(!p.blast(at, 0.0, 100.0));
    assert!(!p.blast(at, f32::INFINITY, f32::NAN));
    // Eight a tick, the ninth refused; a huge one held to the cap.
    for _ in 0..7 {
        assert!(p.impulse(ghost, at, [0.0, 0.0, 1.0]));
    }
    assert!(p.impulse(ghost, at, [0.0, 0.0, 1.0e9]));
    assert!(!p.impulse(ghost, at, [0.0, 0.0, 1.0]));
    assert!(!p.blast(at, 3.0, 100.0));
    let call = format!("return propPredictImpulse({far}, \"0 0 0\", \"0 0 1\");");
    assert_eq!(client_eval(&mut rig, &call), "0");
    assert_eq!(
        client_eval(&mut rig, "return propPredictImpulse(1, \"x\", \"0 0 1\");"),
        "0"
    );
    let stats = p.stats();
    assert!(stats.refused >= 8, "{stats:?}");
    let server = server_at(&rig, id).expect("the crate");
    rig.step();
    let shown = drawn_at(&p, ghost).expect("drawn");
    assert!(
        shown[2] > server[2] + 0.1,
        "the capped push was shown: {shown:?}"
    );
    assert_eq!(p.stats().impulses, stats.impulses + 8);
    // Never sent: the server's crate stays where it was.
    rig.run(8);
    let now = server_at(&rig, id).expect("the crate");
    assert!(len(sub(now, server)) < 0.01, "{server:?} {now:?}");
}

#[test]
fn a_projectile_pushes_a_prop_along_its_flight_held_to_the_cap() {
    let (mut rig, _) = setup(false);
    let id = number(&mut rig, "return propSpawn(Crate, \"0 6 0.5\");");
    rig.run(30);
    let before = server_at(&rig, id).expect("the crate");
    rig.eval(
        "new Projectile() { dataBlock = TestShot; initialPosition = \"0 0 0.5\"; initialVelocity = \"0 60 0\"; };",
    );
    rig.run(10);
    let moved = sub(server_at(&rig, id).expect("the crate"), before);
    assert!(
        moved[1] > 0.01 && moved[0].abs() < 0.005,
        "pushed along the shot: {moved:?}"
    );
    // Both are told: the shot's onCollision names the prop, the prop's names the shot.
    assert_eq!(rig.eval("return $ShotHit;"), id.to_string());
    assert!(!rig.eval("return $PropHitBy;").is_empty());
    // A datablock `impulse` past the cap is held to it.
    rig.eval("TestShot.impulse = 1e9;");
    let id2 = number(&mut rig, "return propSpawn(Crate, \"10 6 0.5\");");
    rig.run(30);
    rig.eval(
        "new Projectile() { dataBlock = TestShot; initialPosition = \"10 0 0.5\"; initialVelocity = \"0 60 0\"; };",
    );
    let mut fastest = 0.0f32;
    for _ in 0..10 {
        rig.step();
        let v: Vec<f32> = rig
            .eval(&format!("return propGetVelocity({id2});"))
            .split(' ')
            .filter_map(|w| w.parse().ok())
            .collect();
        if v.len() == 3 {
            fastest = fastest.max(len([v[0], v[1], v[2]]));
        }
    }
    // 20 kg: the cap's speed, or the crate's top speed if that is lower.
    let cap = torque_plugin_props::hits::MAX_HIT_IMPULSE / 20.0;
    assert!(
        fastest > 1.0 && fastest <= cap + 1.0,
        "{fastest} (cap {cap})"
    );
}

#[test]
fn the_shooter_sees_a_predicted_prop_move_when_its_shot_arrives() {
    let (mut rig, player) = setup(true);
    let (id, ghost) = held_crate(&mut rig, player, "0 -6 0.5", 2.5);
    let p = prediction(&rig);
    let before = drawn_at(&p, ghost).expect("drawn");
    // A shot of the client's own Player at the crate from far enough that the client sees it fly.
    rig.eval(&format!(
        "new Projectile() {{ dataBlock = TestShot; initialPosition = \"0 24 0.5\"; initialVelocity = \"0 -60 0\"; sourceObject = {player}; }};"
    ));
    // The move of the hit is stepped again with its push: the crate is drawn moving in the frame of the hit.
    let mut seen = None;
    for t in 0..40 {
        rig.step();
        if seen.is_none() && p.stats().shot_hits > 0 {
            seen = Some(t);
            let now = drawn_at(&p, ghost).expect("drawn");
            assert!(
                now[1] < before[1] - 0.005,
                "moved with the hit: {before:?} {now:?}"
            );
        }
    }
    assert!(
        seen.is_some(),
        "the client followed its shot into the crate: {:?}",
        p.stats()
    );
    let start = p.stats();
    rig.run(40);
    let mean = mean_between(&start, &p.stats());
    println!(
        "shot: mean correction {mean}, {:?}, server {:?}",
        p.stats(),
        server_at(&rig, id)
    );
    assert!(mean <= 0.03, "{mean}");
}

#[test]
fn an_explosion_pushes_the_predicted_props_in_its_reach_at_once() {
    // Torque's ProjectileData record carries no area impulse: the client is told the game's (`TestBlast`'s).
    let config = PredictConfig {
        shot: ShotPush::of(None, Some(400.0), Some(4.0)),
        ..PredictConfig::default()
    };
    let (mut rig, player) = setup_with(Some(config));
    let (id, ghost) = held_crate(&mut rig, player, "0 -6 0.5", 2.5);
    let p = prediction(&rig);
    let before = drawn_at(&p, ghost).expect("drawn");
    rig.eval(&format!(
        "new Projectile() {{ dataBlock = TestBlast; initialPosition = \"0 24 0.5\"; initialVelocity = \"0 -60 0\"; sourceObject = {player}; }};"
    ));
    let mut seen = false;
    for _ in 0..40 {
        rig.step();
        if !seen && p.stats().shot_hits > 0 {
            seen = true;
            let now = drawn_at(&p, ghost).expect("drawn");
            // The hit and the blast, in the frame of the hit: it moves away from the shot and up off the floor.
            assert!(len(sub(now, before)) > 0.05, "{before:?} {now:?}");
        }
    }
    assert!(seen, "{:?}", p.stats());
    let start = p.stats();
    rig.run(40);
    let mean = mean_between(&start, &p.stats());
    let server = server_at(&rig, id).expect("the crate");
    println!(
        "blast: mean correction {mean}, server {server:?}, {:?}",
        p.stats()
    );
    assert!(
        len(sub(server, [0.0, -6.0, 0.5])) > 0.2,
        "the server's blast pushed it: {server:?}"
    );
    assert!(mean <= 0.03, "{mean}");
}

/// The fastest a prop moves over `ticks` ticks.
fn fastest(rig: &mut Rig, id: u32, ticks: u32) -> f32 {
    let mut fastest = 0.0f32;
    for _ in 0..ticks {
        rig.step();
        let v: Vec<f32> = rig
            .eval(&format!("return propGetVelocity({id});"))
            .split(' ')
            .filter_map(|w| w.parse().ok())
            .collect();
        if v.len() == 3 {
            fastest = fastest.max(len([v[0], v[1], v[2]]));
        }
    }
    fastest
}

#[test]
fn a_projectile_gone_at_its_hit_pushes_all_the_same_and_an_old_explosion_pushes_no_new_prop() {
    let (mut rig, _) = setup(false);
    let id = number(&mut rig, "return propSpawn(Crate, \"0 6 0.5\");");
    rig.run(30);
    rig.eval(
        "new Projectile() { dataBlock = TestShotGone; initialPosition = \"0 0 0.5\"; initialVelocity = \"0 60 0\"; };",
    );
    // 120 on a 20 kg crate: 6 units a second, where the shot's speed alone would give it 0.75.
    let speed = fastest(&mut rig, id, 10);
    assert!(speed > 5.0, "{speed}");
    // An explosion on the floor far from every prop, then a prop where it went off: the old explosion pushes nothing.
    rig.eval(
        "new Projectile() { dataBlock = TestBlast; initialPosition = \"30 0 3\"; initialVelocity = \"0 0 -60\"; };",
    );
    rig.run(6);
    let late = number(&mut rig, "return propSpawn(Crate, \"30 0.5 0.5\");");
    let speed = fastest(&mut rig, late, 10);
    assert!(speed < 0.5, "{speed}");
}

#[test]
fn a_predicted_spawn_is_adopted_by_its_ghost_once_and_one_never_confirmed_fades() {
    let (mut rig, _) = setup(true);
    let p = prediction(&rig);
    let data = rig.eval("return Crate.getId();");
    // A client names a datablock by its id (the server's and the client's are the same).
    let id: u8 = client_eval(
        &mut rig,
        &format!("return propPredictSpawn({data}, \"3 -8 0.5\");"),
    )
    .parse()
    .expect("an id");
    assert!(id > 0);
    assert_eq!(p.spawns().len(), 1);
    let made = number(
        &mut rig,
        &format!("return propSpawn(Crate, \"3 -8 0.5\", ClientGroup.getObject(0), {id});"),
    );
    assert!(made > 0);
    assert!(
        rig.run_until(100, |_| p.spawns().is_empty()),
        "{:?}",
        p.spawns()
    );
    // A second one the server never makes: waiting, then fading, then gone, within the window.
    let other: u8 = client_eval(
        &mut rig,
        &format!("return propPredictSpawn({data}, \"5 -8 0.5\");"),
    )
    .parse()
    .expect("an id");
    assert!(other > 0 && other != id);
    rig.run(70);
    assert!(
        p.spawns().iter().all(|(i, _, _)| *i == 0),
        "{:?}",
        p.spawns()
    );
    rig.run(10);
    assert!(p.spawns().is_empty(), "{:?}", p.spawns());
    assert_eq!(
        client_eval(&mut rig, "return propPredictSpawn(NoSuchData, \"0 0 0\");"),
        "0"
    );
}

#[test]
fn a_replay_past_the_ticks_work_draws_the_props_interpolated() {
    // Room for no physics work at all: every tick's replay or step runs out.
    let config = PredictConfig {
        max_work: 1,
        ..PredictConfig::default()
    };
    let (mut rig, player) = setup_with(Some(config));
    let id = number(&mut rig, "return propSpawn(Crate, \"0 -10 0.5\");");
    number(
        &mut rig,
        &format!("return jointGrapple({player} SPC \"0 0 1\", {id}, 2.5);"),
    );
    assert!(rig.run_until(200, |r| {
        ghost_of(r, id).is_some_and(|g| prediction(r).is_predicted(g))
    }));
    let ghost = ghost_of(&rig, id).expect("its ghost");
    let p = prediction(&rig);
    for _ in 0..60 {
        rig.peers[0]
            .client
            .vm_mut()
            .set_global("$mvForwardAction", "1");
        rig.step();
        // Predicted, but drawn as the client shows it: no predicted state, nothing drawn by the part.
        assert!(p.is_predicted(ghost));
        assert_eq!(drawn_at(&p, ghost), None);
        assert_eq!(predicted_at(&rig, ghost), None);
    }
    assert!(p.stats().over_budget >= 60, "{:?}", p.stats());
}

/// The client's predicted state of prop `ghost`.
fn predicted_at(rig: &Rig, ghost: u32) -> Option<[f32; 3]> {
    book(rig).with(|b| b.get(ghost)?.predicted.map(|q| q.position()))?
}

/// What a run of the client's Player pushing a crate toward another player came to.
#[derive(Debug)]
struct Toward {
    /// The farthest the drawn crate was from the server's over the last 100 ticks, and the mean correction of the run.
    apart: f32,
    mean: f64,
    /// The server's crate at the end.
    server: [f32; 3],
    restored: u64,
}

/// The client's Player walks into a crate and pushes it toward another player, who stands in its way; the server's crate stops
/// at that player's box. `bystanders`: whether the client's world holds the other player's box.
fn push_toward_a_player(bystanders: bool) -> Toward {
    let config = PredictConfig {
        bystanders,
        ..PredictConfig::default()
    };
    let (mut rig, player) = setup_with(Some(config));
    // The other player: not controlled by the client, standing 6 units ahead of it.
    let other: u32 = rig
        .eval("return new Player() { dataBlock = FigureData; position = \"0 -2 0.5\"; };")
        .parse()
        .expect("a player");
    assert!(other > 0 && other != player);
    rig.eval(&format!("{player}.setTransform(\"0 -8 0.5\");"));
    let id = number(&mut rig, "return propSpawn(Crate, \"0 -6.9 0.5\");");
    assert!(id > 0, "{:?}", rig.server.drain_log());
    assert!(rig.run_until(400, |r| r.peers[0].ghosts() >= 4
        && ghost_of(r, id).is_some()));
    let ghost = ghost_of(&rig, id).expect("its ghost");
    let p = prediction(&rig);
    let start = p.stats();
    let mut apart = 0.0f32;
    for t in 0..300u32 {
        let forward = if t < 20 { 0.0 } else { 1.0 };
        rig.peers[0]
            .client
            .vm_mut()
            .set_global("$mvForwardAction", &forward.to_string());
        rig.step();
        if t >= 200
            && let (Some(drawn), Some(server)) = (drawn_at(&p, ghost), server_at(&rig, id))
        {
            apart = apart.max(len(sub(drawn, server)));
        }
    }
    let end = p.stats();
    let toward = Toward {
        apart,
        mean: mean_between(&start, &end),
        server: server_at(&rig, id).expect("the crate"),
        restored: end.restores - start.restores,
    };
    println!("toward a player (bystanders {bystanders}): {toward:?}");
    toward
}

/// The other player's box starts at y = -2.5: the crate (half a unit) stops short of it on the server. With the box in the
/// client's world the predicted crate stops there too (Torque's protocol: apart 0.03, mean correction 0.003; the engine's: 0.26
/// and 0.003); without it the predicted crate runs on through the player (apart 1.46 and mean 0.40; 5.4 and 0.145), which must
/// fail the same thresholds.
#[test]
fn a_prop_pushed_toward_another_player_stops_at_it_as_on_the_server() {
    // Squeezed between the walker and the other player the crate shivers a few centimeters.
    let holds = |t: &Toward| t.apart <= 0.4 && t.mean <= 0.03;
    let with = push_toward_a_player(true);
    assert!(with.server[1] > -4.0 && with.server[1] <= -2.9, "{with:?}");
    assert!(with.restored > 0, "{with:?}");
    assert!(holds(&with), "{with:?}");
    let without = push_toward_a_player(false);
    assert!(
        !holds(&without),
        "the thresholds do not tell the boxes apart: {without:?}"
    );
}
