//! The window client's review pictures, drawn offscreen (no window): the main menu, the network's list of games with a
//! server in it, the download of the server's mod part way through, a second join with the mod already downloaded, the
//! match, and its Esc menu. The server is a soccer server on the loopback address that the client ticks itself.

use std::path::{Path, PathBuf};

use torque::app::GameServer;
use torque::app::shots::{Seat, Shots};
use torque::app::window::offscreen_gpu;
use torque::mods::CacheChoice;
use torque_render_core::Gpu;

use crate::client_builder;
use crate::client_files;
use crate::game::{ClientOptions, JoinState, SoccerGame};
use crate::server::{ServerOptions, server_engine, start};

/// The pictures' size.
pub const SIZE: (u32, u32) = (1280, 720);

/// A soccer server on the loopback address, on a free port, with its bots.
pub fn loopback_server(lua: &crate::Lua, name: &str) -> Result<GameServer, String> {
    let options = ServerOptions {
        port: 0,
        name: name.to_string(),
        ..ServerOptions::default()
    };
    start(&server_engine(lua)?, &options, options.udp())
}

/// A client of the game made by its own engine, with none of the mods, keeping what it downloads in `cache`.
pub fn client(options: ClientOptions, cache: &Path) -> Result<SoccerGame, String> {
    let files = client_files().map_err(|e| e.to_string())?;
    let engine = client_builder(&files, CacheChoice::Folder(cache.to_path_buf()))?
        .build()
        .map_err(|e| e.to_string())?;
    SoccerGame::from_engine(&engine, &files, options, None)
}

/// Runs `seat` (drawing) until `done` holds, at most `limit` ms; whether it did.
fn until(
    seat: &mut Seat<SoccerGame>,
    gpu: &Gpu,
    limit: u32,
    done: impl Fn(&SoccerGame) -> bool,
) -> Result<bool, String> {
    let mut ran = 0;
    while !done(&seat.game) {
        if ran >= limit {
            return Ok(false);
        }
        seat.run(gpu, 50)?;
        ran += 50;
    }
    Ok(true)
}

/// The share of the download received, 0 to 1.
fn received(game: &SoccerGame) -> f32 {
    let bytes = game.client.number("$Join::Bytes");
    if bytes > 0.0 {
        game.client.number("$Join::Received") / bytes
    } else {
        0.0
    }
}

/// The pictures, written into `dir`, of a client made with `options` (its Lua extension starts the server); its downloads
/// go into a fresh folder under `work`. Returns the paths.
pub fn write_shots(
    dir: &Path,
    options: ClientOptions,
    work: &Path,
) -> Result<Vec<PathBuf>, String> {
    let lua = options
        .lua
        .clone()
        .ok_or("the pictures need the Lua extension for their server (build it with `just lua`)")?;
    let cache = work.join("shots-cache");
    let _ = std::fs::remove_dir_all(&cache);
    let server = loopback_server(&lua, "Soccer in the park")?;
    let address = server.address().ok_or("the server has no socket")?;
    let options = ClientOptions {
        port: address.port(),
        ..options
    };
    let mut game = client(options, &cache)?;
    game.client.start_hosting(server);
    let gpu = offscreen_gpu().map_err(|e| e.to_string())?;
    let mut seat = Seat::new(game, &gpu, SIZE)?;
    let mut shots = Shots::new(dir)?;

    seat.run(&gpu, 300)?;
    shots.save(&seat.picture(&gpu)?, "menu")?;

    seat.game.client.call("showJoin", &[])?;
    if !until(&mut seat, &gpu, 5_000, |g| {
        g.client.number("$Game::ServerRows") > 0.0
    })? {
        return Err("the list of games found no server".into());
    }
    shots.save(&seat.picture(&gpu)?, "join-list")?;

    seat.game.client.call("joinSelected", &[])?;
    let partway = |g: &SoccerGame| g.join_state() == JoinState::Downloading && received(g) > 0.4;
    if !until(&mut seat, &gpu, 20_000, partway)? {
        return Err(format!(
            "no download part way: {:?}",
            seat.game.join_state()
        ));
    }
    shots.save(&seat.picture(&gpu)?, "download")?;
    if !until(&mut seat, &gpu, 30_000, SoccerGame::playing)? {
        return Err(format!(
            "the client did not join: {:?}",
            seat.game.join_state()
        ));
    }
    seat.run(&gpu, 2_500)?;
    shots.save(&seat.picture(&gpu)?, "match")?;

    seat.game.client.call("openPause", &[])?;
    seat.run(&gpu, 200)?;
    shots.save(&seat.picture(&gpu)?, "pause")?;
    seat.game.client.call("closePause", &[])?;

    // Out and in again: the mod is in the cache, so nothing is downloaded.
    seat.game.client.call("disconnect", &[])?;
    seat.run(&gpu, 500)?;
    seat.game.join(&address.to_string())?;
    if !until(&mut seat, &gpu, 10_000, |g| {
        g.join_state() == JoinState::Cached
    })? {
        return Err(format!(
            "the second join downloaded: {:?}",
            seat.game.join_state()
        ));
    }
    // The screen says so only until the match's first word arrives: the picture is of the frame that saw it.
    shots.save(&seat.picture(&gpu)?, "already-downloaded")?;
    Ok(shots.paths)
}
