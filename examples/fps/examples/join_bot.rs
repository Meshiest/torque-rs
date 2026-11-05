//! A desktop player with no window, for checks: joins a server over UDP on the loopback address and stands there for a while.
//!
//! `cargo run -p torque-example-fps --example join_bot -- <address:port> [--seconds <n>] [--name <name>]` (default 60 s, "Desk").
//! It prints `join-bot: in game` once the server put it in the game, then each second the players' figures it holds.

use std::time::{Duration, Instant};

use torque::app::cli::{Args, unknown};
use torque_example_fps::client::{ArenaGame, ClientOptions, NetMode};
use torque_example_fps::files::GameFiles;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = Args::from_env();
    let mut address = None;
    let mut seconds = 60u64;
    let mut name = "Desk".to_string();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--seconds" => seconds = args.number(&arg)?,
            "--name" => name = args.value(&arg, "a name")?,
            _ if address.is_none() && !arg.starts_with("--") => address = Some(arg),
            _ => return Err(unknown(&arg).into()),
        }
    }
    let address = address.ok_or("join_bot <address:port>")?;
    let mut bot = ArenaGame::new(
        GameFiles::bundled()?,
        ClientOptions {
            net: NetMode::Loopback,
            name,
            port: 0,
            ..ClientOptions::default()
        },
    )?;
    bot.call("connectTo", &[&address])?;
    let start = Instant::now();
    let mut next = start;
    let mut said = false;
    let mut last_report = start;
    while start.elapsed() < Duration::from_secs(seconds) {
        bot.step(32);
        if bot.in_game() && !said {
            println!("join-bot: in game");
            said = true;
        }
        if last_report.elapsed() >= Duration::from_secs(1) {
            last_report = Instant::now();
            let figures = bot
                .with_world(|w| {
                    w.records
                        .ghosts
                        .values()
                        .filter(|g| g.class == Some(torque::ghosts::TgeClass::Player))
                        .count()
                })
                .unwrap_or(0);
            println!("join-bot: {figures} figures");
        }
        next += Duration::from_millis(32);
        if let Some(wait) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        } else {
            next = Instant::now();
        }
    }
    Ok(())
}
