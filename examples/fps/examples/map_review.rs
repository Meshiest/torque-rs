//! Review pictures of the keep and the yard and of the round's end between them: two clients over the loopback address on a
//! rotation of the two maps, drawn offscreen into the folder given.

use std::path::PathBuf;

use torque::app::shots::{FRAME_MS, Seat, Shots, run};
use torque_example_fps::client::{ArenaGame, ClientOptions, NetMode};
use torque_example_fps::files::GameFiles;
use torque_render_core::Gpu;
use torque_render_scene::hd::kit::look_at;

type S = Seat<ArenaGame>;

fn until(
    gpu: &Gpu,
    a: &mut S,
    b: &mut S,
    what: &str,
    done: impl Fn(&S, &S) -> bool,
) -> Result<(), String> {
    let mut waited = 0;
    while !done(a, b) {
        if waited > 30_000 {
            return Err(format!("timed out waiting for {what}"));
        }
        run(gpu, &mut [&mut *a, &mut *b], FRAME_MS)?;
        waited += FRAME_MS;
    }
    Ok(())
}

fn server(a: &mut S, text: &str) -> Result<String, String> {
    let vm = a.game.server.as_mut().ok_or("no server")?.vm_mut();
    vm.eval(text.as_bytes())
        .map(|v| String::from_utf8_lossy(&v).into_owned())
        .map_err(|e| e.to_string())
}

fn place(a: &mut S, name: &str, at: [f32; 3], yaw: f32) -> Result<(), String> {
    server(
        a,
        &format!(
            "placeFigure(\"{name}\", {}, {}, {}, {yaw});",
            at[0], at[1], at[2]
        ),
    )?;
    Ok(())
}

/// The yaw that faces from `from` toward `to`.
fn facing(from: [f32; 3], to: [f32; 3]) -> f32 {
    (to[0] - from[0]).atan2(to[1] - from[1])
}

fn playing(a: &S, b: &S, maps: i32) -> bool {
    [a, b].iter().all(|s| {
        s.game.global("$Arena::Maps").parse::<i32>().unwrap_or(0) == maps
            && s.game.global("$Arena::OwnGhost") != "-1"
            && s.game.scene().others().len() == 1
    })
}

fn main() -> Result<(), String> {
    let dir = PathBuf::from(std::env::args().nth(1).ok_or("give a folder")?);
    let size = (1280, 720);
    let gpu = Gpu::new().map_err(|e| e.to_string())?;
    let seat = |name: &str| -> Result<S, String> {
        let game = ArenaGame::new(
            GameFiles::bundled().map_err(|e| e.to_string())?,
            ClientOptions {
                net: NetMode::Loopback,
                name: name.to_string(),
                port: 0,
                map: Some("missions/keep.mis".to_string()),
                ..ClientOptions::default()
            },
        )?;
        Seat::new(game, &gpu, size)
    };
    let (mut a, mut b) = (seat("Ann")?, seat("Bob")?);
    let mut shots = Shots::new(&dir)?;
    a.game.call("hostGame", &[])?;
    until(&gpu, &mut a, &mut b, "the hosted game", |a, _| {
        a.game.in_game()
    })?;
    let address = a.game.hosted_address().ok_or("no address")?.to_string();
    b.game.call("connectTo", &[&address])?;
    until(&gpu, &mut a, &mut b, "Bob", |a, b| playing(a, b, 1))?;
    server(
        &mut a,
        "$Pref::Server::MapList = \"missions/keep.mis missions/yard.mis\"; $Pref::Server::RoundEndSeconds = 4;",
    )?;
    run(&gpu, &mut [&mut a, &mut b], 3000)?;

    // The keep: from above; from a hall under the south deck across the court, Bob on the bridge; from the north deck down
    // over the court, Bob behind a tall block.
    let mut eye = look_at([-4.0, -21.0, 36.0], [0.0, 0.5, 0.0]);
    eye.fov = Some(70);
    a.eye = Some(eye);
    shots.save(&a.picture(&gpu)?, "keep-overview")?;
    a.eye = None;
    let (ann, bob) = ([-12.0, -12.5, 0.1], [0.0, 3.0, 3.1]);
    place(&mut a, "Ann", ann, facing(ann, bob))?;
    place(&mut a, "Bob", bob, facing(bob, ann))?;
    run(&gpu, &mut [&mut a, &mut b], 800)?;
    a.game.set_view_pitch(-0.12);
    run(&gpu, &mut [&mut a, &mut b], 200)?;
    shots.save(&a.picture(&gpu)?, "keep-hall")?;
    let (ann, bob) = ([-6.0, 11.0, 3.1], [4.5, -8.0, 0.1]);
    place(&mut a, "Ann", ann, facing(ann, bob))?;
    place(&mut a, "Bob", bob, facing(bob, ann))?;
    run(&gpu, &mut [&mut a, &mut b], 800)?;
    a.game.set_view_pitch(0.14);
    run(&gpu, &mut [&mut a, &mut b], 200)?;
    shots.save(&a.picture(&gpu)?, "keep-deck")?;

    // The round's end: a short round, Ann ahead; the scores over her view.
    server(
        &mut a,
        "$Arena::MinRoundSeconds = 0; $Pref::Server::RoundSeconds = 0.5; startRound(); $Pref::Server::RoundSeconds = 600; \
         for (%i = 0; %i < ClientGroup.getCount(); %i++) { %c = ClientGroup.getObject(%i); %c.score = %c.name $= \"Ann\" ? 7 : 4; }",
    )?;
    a.game.set_view_pitch(0.1);
    until(&gpu, &mut a, &mut b, "the round's end", |a, b| {
        a.game.global("$Arena::RoundShown") == "1" && b.game.global("$Arena::RoundShown") == "1"
    })?;
    run(&gpu, &mut [&mut a, &mut b], 400)?;
    shots.save(&a.picture(&gpu)?, "round-end")?;

    // The yard, loaded in place: from above; from a stand down the long lane, Bob by the middle pillar; from the flank
    // behind a long low wall across the field.
    until(&gpu, &mut a, &mut b, "the yard", |a, b| playing(a, b, 2))?;
    run(&gpu, &mut [&mut a, &mut b], 3000)?;
    let mut eye = look_at([-12.0, -50.0, 52.0], [0.0, 2.0, 0.0]);
    eye.fov = Some(70);
    a.eye = Some(eye);
    shots.save(&a.picture(&gpu)?, "yard-overview")?;
    a.eye = None;
    let (ann, bob) = ([-33.0, 0.5, 1.6], [6.0, 3.5, 0.1]);
    place(&mut a, "Ann", ann, facing(ann, [36.0, 0.0, 0.0]))?;
    place(&mut a, "Bob", bob, facing(bob, ann))?;
    run(&gpu, &mut [&mut a, &mut b], 800)?;
    a.game.set_view_pitch(0.0);
    run(&gpu, &mut [&mut a, &mut b], 200)?;
    shots.save(&a.picture(&gpu)?, "yard-lane")?;
    let (ann, bob) = ([-20.0, -12.0, 0.1], [4.0, 14.0, 0.1]);
    place(&mut a, "Ann", ann, facing(ann, bob))?;
    place(&mut a, "Bob", bob, facing(bob, ann))?;
    run(&gpu, &mut [&mut a, &mut b], 800)?;
    a.game.set_view_pitch(0.0);
    run(&gpu, &mut [&mut a, &mut b], 200)?;
    shots.save(&a.picture(&gpu)?, "yard-flank")?;
    for p in &shots.paths {
        println!("{}", p.display());
    }
    Ok(())
}
