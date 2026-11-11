//! The office played with others on this computer, stepped by hand: a dedicated office server on a UDP socket of the loopback
//! address and players' clients that find it in the list of matches or join it by address, see the catalog mod download, are
//! asked about the server's scripts and play. It is what the multiplayer tests (`tests/multiplayer.rs`) and the pictures of
//! the multiplayer path (`--match-shots`) run; every client is the window's own ([`LevelDemo::for_matches`]) on a loopback
//! socket ([`NetMode::Loopback`]).

use std::path::{Path, PathBuf};

use torque::app::server::Listen;
use torque::app::view::WorldView;
use torque::app::{App, Host};
use torque::gui::GuiDraws;
use torque::gui::client_script::PromptSpot;
use torque::mods::CacheChoice;
use torque_client_script_host::RealmGuest;
use torque_plugin_client_script::{Choices, RealmState};
use torque_render_core::Gpu;
use torque_render_scene::source::Eye;

use crate::catalog::{ABILITIES, Instances, SIZE, TICK_MS, mod_files, press_prompt};
use crate::client::{LevelDemo, MatchOptions, NetMode};
use crate::files::GameFiles;
use crate::office::{OfficeStart, ServerScripts, office_engine, start_office};
use crate::server::WorldServer;

/// Where a client's join stands: what the join screen shows (`$Join::State` in `common/matches.cs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum JoinState {
    /// At a menu, in no join.
    Menu,
    Connecting,
    /// Let in; the server has not said which mods it plays yet.
    Accepted,
    Downloading,
    /// The server's mods were all here already: nothing to download.
    Cached,
    /// Downloaded; waiting for the world.
    Joining,
    Playing,
    /// The join ended: the screen says why.
    Failed,
}

impl JoinState {
    /// The state the scripts' word names (none, or a word they never set: [`JoinState::Menu`]).
    #[must_use]
    pub fn from_script(word: &str) -> JoinState {
        match word {
            "connecting" => JoinState::Connecting,
            "accepted" => JoinState::Accepted,
            "downloading" => JoinState::Downloading,
            "cached" => JoinState::Cached,
            "joining" => JoinState::Joining,
            "playing" => JoinState::Playing,
            "failed" => JoinState::Failed,
            _ => JoinState::Menu,
        }
    }
}

/// Where `demo`'s join stands.
pub fn join_state(demo: &LevelDemo) -> JoinState {
    JoinState::from_script(&demo.client.global("$Join::State"))
}

/// The state of `demo`'s realm (idle when it runs no server scripts).
pub fn realm_state(demo: &LevelDemo) -> RealmState {
    demo.scripts
        .as_ref()
        .map_or(RealmState::Idle, |s| s.realms.state())
}

/// The office's server and its players (module docs).
pub struct MatchSession {
    pub server: WorldServer,
    pub players: Vec<LevelDemo>,
    guest: RealmGuest,
    work: PathBuf,
    /// The time the session has not stepped yet (ms), for the pictures.
    owed: u32,
}

impl MatchSession {
    /// A dedicated office server named `name` on a free UDP port of the loopback address, and no players yet. The players'
    /// downloads go into folders under `work`, emptied first.
    pub fn start(guest: &RealmGuest, work: &Path, name: &str) -> Result<MatchSession, String> {
        let _ = std::fs::remove_dir_all(work);
        let files = GameFiles::bundled().map_err(|e| e.to_string())?;
        let mods = mod_files().map_err(|e| e.to_string())?;
        let engine = office_engine(files, mods, CacheChoice::Memory)?
            .build()
            .map_err(|e| e.to_string())?;
        let listen = Listen::Udp {
            address: std::net::Ipv4Addr::LOCALHOST,
            port: 0,
            public: false,
        };
        let start = OfficeStart {
            name,
            dedicated: true,
        };
        let server = start_office(&engine, listen, &start)?;
        Ok(MatchSession {
            server,
            players: Vec::new(),
            guest: guest.clone(),
            work: work.to_path_buf(),
            owed: 0,
        })
    }

    /// The UDP port number the server listens on.
    pub fn port(&self) -> u16 {
        self.server.address().map_or(0, |a| a.port())
    }

    /// The server's address, as a player types it.
    pub fn address(&self) -> String {
        format!("127.0.0.1:{}", self.port())
    }

    /// A new player called `name`, at its menu, whose downloads are kept in `cache` (a folder under the session's work folder
    /// by that name: two players with the same cache share what they downloaded). Its index in [`MatchSession::players`].
    pub fn add_player(&mut self, name: &str, cache: &str) -> Result<usize, String> {
        let files = GameFiles::bundled().map_err(|e| e.to_string())?;
        let cache = CacheChoice::Folder(self.work.join(cache));
        let engine = office_engine(files, Vec::new(), cache)?
            .build()
            .map_err(|e| e.to_string())?;
        let options = MatchOptions {
            net: NetMode::Loopback,
            name: name.to_string(),
            port: self.port(),
            ..MatchOptions::default()
        };
        let scripts = ServerScripts::new(
            Choices::in_memory(),
            Some(Box::new(Instances(self.guest.clone()))),
        );
        let mut demo = LevelDemo::for_matches(&engine, options, None, scripts)?;
        demo.client.host.surface_size(SIZE);
        self.players.push(demo);
        Ok(self.players.len() - 1)
    }

    /// A step of 32 ms (the server's tick): the server, then every player.
    pub fn step(&mut self) {
        self.server.tick();
        self.server.service();
        for player in &mut self.players {
            player.step(TICK_MS);
        }
    }

    /// `steps` steps of 32 ms.
    pub fn run(&mut self, steps: u32) {
        for _ in 0..steps {
            self.step();
        }
    }

    /// Steps of 32 ms until `done` holds, at most `limit`; whether it did.
    pub fn run_until(&mut self, limit: u32, mut done: impl FnMut(&MatchSession) -> bool) -> bool {
        for _ in 0..limit {
            if done(self) {
                return true;
            }
            self.step();
        }
        done(self)
    }

    /// Player `n` joins the server by its address, as typed into the list of matches' box.
    pub fn join(&mut self, n: usize) -> Result<(), String> {
        let address = self.address();
        let demo = &mut self.players[n];
        demo.client.call("showJoin", &[])?;
        let vm = demo.client.vm_mut();
        let field = vm.objects().find(b"AddressEdit").ok_or("no address box")?;
        vm.call_method(field, b"setValue", &[address.into_bytes()])
            .map_err(|e| e.to_string())?;
        demo.client.call("connectAddress", &[]).map(drop)
    }

    /// Player `n` answers the consent prompt with Allow, as [`crate::catalog::Catalog::allow`] does.
    pub fn allow(&mut self, n: usize) -> Result<(), String> {
        if !self.run_until(3_000, |s| realm_state(&s.players[n]) == RealmState::Asking) {
            return Err(format!(
                "player {n} was not asked: {:?} {:?}",
                realm_state(&self.players[n]),
                join_state(&self.players[n])
            ));
        }
        self.run(24);
        if !press_prompt(&mut self.players[n].client, ABILITIES, PromptSpot::Allow) {
            return Err(format!("player {n}'s prompt took no press"));
        }
        if self.run_until(2_000, |s| realm_state(&s.players[n]) == RealmState::Running) {
            Ok(())
        } else {
            Err(format!(
                "player {n}'s menu did not start: {:?}",
                realm_state(&self.players[n])
            ))
        }
    }

    /// Puts player `name`'s figure at (`x`, `y`) of the map, facing `yaw`, by the server's script.
    pub fn stand(&mut self, name: &str, x: f32, y: f32, yaw: f32) -> Result<(), String> {
        self.server_eval(&format!(
            "for (%i = 0; %i < ClientGroup.getCount(); %i++) {{ %c = ClientGroup.getObject(%i); \
             if (%c.name $= \"{name}\") %c.player.setTransform(\"{x} {y} 0.2 0 0 1 {yaw}\"); }}"
        ))?;
        self.run(12);
        Ok(())
    }

    /// Runs script `code` on the server and returns its result.
    pub fn server_eval(&mut self, code: &str) -> Result<String, String> {
        self.server
            .vm_mut()
            .eval(code.as_bytes())
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .map_err(|e| format!("{code}: {e}"))
    }
}

/// The session as an [`App`]: the first player's view, the session stepped with it.
pub struct SessionView(pub MatchSession);

impl App for SessionView {
    fn host(&mut self) -> &mut Host {
        &mut self.0.players[0].client.host
    }

    fn host_ref(&self) -> &Host {
        &self.0.players[0].client.host
    }

    fn title(&self) -> String {
        "torque-rs level demo: the office with others".to_string()
    }

    fn log_name(&self) -> &str {
        "example-level-demo"
    }

    fn view(&self, gpu: &Gpu) -> WorldView {
        self.0.players[0].view(gpu)
    }

    fn advance(&mut self, ms: u32) {
        self.0.owed += ms;
        while self.0.owed >= TICK_MS {
            self.0.owed -= TICK_MS;
            self.0.step();
        }
        self.0.players[0].add_frame_time(ms as f32 / 1000.0);
    }

    fn frame(&mut self, view: &mut WorldView) -> (GuiDraws, Eye) {
        self.0.players[0].frame(view)
    }
}

/// Pictures of playing the office with others, from the first player, offscreen, into `dir`: the menu with Host a match and
/// Join a match, the list of matches with the server in it, the join screen with the catalog part way down, the consent
/// prompt, and the catalog open in the office with a second player in front. Returns the paths.
pub fn write_shots(guest: &RealmGuest, work: &Path, dir: &Path) -> Result<Vec<PathBuf>, String> {
    use torque::app::shots::{Seat, Shots};
    use torque::app::window::offscreen_gpu;

    let mut session = MatchSession::start(guest, work, "The office")?;
    session.add_player("Ann", "ann")?;
    session.add_player("Bob", "bob")?;
    let gpu = offscreen_gpu().map_err(|e| e.to_string())?;
    let mut seat = Seat::new(SessionView(session), &gpu, SIZE)?;
    let mut shots = Shots::new(dir)?;
    seat.run(&gpu, 300)?;
    shots.save(&seat.picture(&gpu)?, "match-menu")?;
    // The list of matches finds the server.
    seat.game.0.players[0].client.call("showJoin", &[])?;
    let listed = |s: &MatchSession| s.players[0].client.number("$Game::ServerRows") > 0.0;
    if !seat.game.0.run_until(3_000, listed) {
        return Err("the list of matches found no server".into());
    }
    seat.run(&gpu, 300)?;
    shots.save(&seat.picture(&gpu)?, "match-list")?;
    // The join screen part way through the download.
    seat.game.0.players[0].client.call("joinSelected", &[])?;
    let part_way = |s: &MatchSession| {
        let c = &s.players[0].client;
        join_state(&s.players[0]) == JoinState::Downloading
            && c.number("$Join::Received") > c.number("$Join::Bytes") * 0.3
    };
    if !seat.game.0.run_until(3_000, part_way) {
        return Err("the download was not seen part way".into());
    }
    shots.save(&seat.picture(&gpu)?, "match-download")?;
    // Bob joins too; both are asked.
    seat.game.0.join(1)?;
    let asked = |s: &MatchSession| {
        (0..2).all(|n| realm_state(&s.players[n]) == RealmState::Asking && s.players[n].in_world())
    };
    if !seat.game.0.run_until(6_000, asked) {
        return Err("the players were not asked".into());
    }
    seat.run(&gpu, 800)?;
    shots.save(&seat.picture(&gpu)?, "match-consent")?;
    for n in 0..2 {
        seat.game.0.allow(n)?;
    }
    // Ann in the corridor east of the lobby, Bob a little ahead of her, both facing east; the catalog open.
    seat.game
        .0
        .stand("Ann", -5.0, 2.0, std::f32::consts::FRAC_PI_2)?;
    seat.game
        .0
        .stand("Bob", -1.5, 2.6, std::f32::consts::FRAC_PI_2)?;
    seat.run(&gpu, 1_500)?;
    let cards = |s: &MatchSession| !crate::catalog::cards_of(&s.players[0].client).is_empty();
    if !seat.game.0.run_until(600, cards) {
        return Err("the catalog did not open".into());
    }
    crate::catalog::spawn_on(&mut seat.game.0.players[1].client, "Plant", "Bob's plant")?;
    seat.run(&gpu, 1_500)?;
    shots.save(&seat.picture(&gpu)?, "match-catalog")?;
    crate::catalog::press_on(&mut seat.game.0.players[0].client, "Hide catalog")?;
    seat.run(&gpu, 1_200)?;
    shots.save(&seat.picture(&gpu)?, "match-office")?;
    Ok(shots.paths)
}
