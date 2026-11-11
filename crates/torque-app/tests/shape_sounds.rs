//! A ShapeBase's sound slots end to end, in each network protocol: a server's `playAudio` reaches the client's mixer as a
//! voice playing at the object, the voice follows the object, `stopAudio` and the object's end stop it, a one-shot's slot
//! frees itself on the server, and a looping one does not. A real server and an engine client over the memory transport,
//! stepped by hand; the mixer is the null sink (no sound card is opened).

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use torque_app::GameFiles;
use torque_app::client::{ClientBuilder, ClientNet, GameClient, carry_between};
use torque_app::config::{EngineConfig, GameConfig};
use torque_app::plugins::{AudioPlugin, DefaultPlugins, set_protocol};
use torque_app::server::{GameServer, Listen, ServerStart};
use torque_app::sounds::WorldSounds;
use torque_engine::{Engine, EngineBuilder, ProtocolKind};
use torque_ghosts::TgeClass;

const SERVER: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_000);

/// A PCM WAV of `seconds` of silence, mono, 22.05 kHz, 16 bit.
fn wav(seconds: u32) -> Vec<u8> {
    let n: u32 = 22_050 * 2 * seconds;
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(4 + 24 + 8 + n).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&22_050u32.to_le_bytes());
    out.extend_from_slice(&(22_050u32 * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&n.to_le_bytes());
    out.extend(std::iter::repeat_n(0u8, n as usize));
    out
}

fn files() -> GameFiles {
    let mut files = GameFiles::default();
    files.insert("server.cs", include_bytes!("shape_sounds/server.cs"));
    files.insert("main.cs", include_bytes!("shape_sounds/main.cs"));
    files.insert("game.toml", include_bytes!("shape_sounds/game.toml"));
    let b = [-0.5, -0.5, -0.5, 0.5, 0.5, 0.5];
    files.insert(
        "art/box.dts",
        &torque_shapes::synth::solid_dts(b, &[("detail32", b)], [0, 0, 0, 32767], [0.0; 3], false),
    );
    files.insert("sounds/ping.wav", &wav(5));
    files
}

fn engine(files: &GameFiles, protocol: ProtocolKind) -> Engine {
    let config = GameConfig::load(files).expect("game.toml");
    let mut builder = EngineBuilder::new();
    builder.add_plugins(DefaultPlugins).config(&config);
    set_protocol(&mut builder, protocol);
    builder.build().expect("the engine builds")
}

struct Rig {
    server: GameServer,
    client: GameClient,
}

impl Rig {
    fn new(protocol: ProtocolKind) -> Rig {
        let files = files();
        let server_engine = engine(&files, protocol);
        let server = GameServer::start_engine(
            &server_engine,
            files.mem_files(),
            &ServerStart {
                listen: Listen::Memory(SERVER),
                globals: &[],
                start: "startIt",
                args: &[],
            },
        )
        .expect("the server starts");
        let client_engine = engine(&files, protocol);
        let mut builder = ClientBuilder::from_engine(
            &client_engine,
            Some(files.mem_files()),
            ClientNet::Memory {
                client: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 50_001),
                server: SERVER,
            },
        )
        .expect("the client builds");
        builder.exec_entry().expect("the client's script");
        let mut client: GameClient = builder.build();
        client
            .call("connectTo", &[&SERVER.to_string()])
            .expect("connect");
        let mut rig = Rig { server, client };
        assert!(
            rig.run_until(400, |r| r.global("$Connected") == "1"),
            "the client joins"
        );
        rig
    }

    /// One tick of the server, then of the client.
    fn step(&mut self) {
        carry_between(&self.server, &[&self.client]);
        self.server.tick();
        carry_between(&self.server, &[&self.client]);
        self.client.step(32);
        carry_between(&self.server, &[&self.client]);
    }

    /// Ticks of the server alone, their packets carried to the client but not read: the client's next step reads them all
    /// in one batch.
    fn server_ticks(&mut self, ticks: usize) {
        for _ in 0..ticks {
            carry_between(&self.server, &[&self.client]);
            self.server.tick();
            carry_between(&self.server, &[&self.client]);
        }
    }

    /// The number of the last effect the client's world queued.
    fn effect_seq(&self) -> u64 {
        self.client.with_world(|w| w.effect_seq).unwrap_or(0)
    }

    fn stopped(&self) -> u64 {
        WorldSounds::of(self.client.vm()).map_or(0, |s| s.borrow().stopped)
    }

    fn slot_voices(&self) -> usize {
        WorldSounds::of(self.client.vm()).map_or(0, |s| s.borrow().slot_voices())
    }

    fn run(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.step();
        }
    }

    fn run_until(&mut self, limit: usize, mut done: impl FnMut(&Rig) -> bool) -> bool {
        for _ in 0..limit {
            if done(self) {
                return true;
            }
            self.step();
        }
        done(self)
    }

    fn global(&self, name: &str) -> String {
        self.server
            .vm()
            .global(name)
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .unwrap_or_default()
    }

    fn eval(&mut self, code: &str) -> String {
        self.server
            .vm_mut()
            .eval(code.as_bytes())
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .unwrap_or_else(|e| panic!("{code}: {e}"))
    }

    /// The client's ghost index of the speaker (the one StaticShape).
    fn speaker(&self) -> Option<u32> {
        self.client
            .with_world(|w| w.of_class(TgeClass::StaticShape).map(|(i, _)| i).next())
            .flatten()
    }

    /// Voices started on the client so far.
    fn started(&self) -> u64 {
        WorldSounds::of(self.client.vm()).map_or(0, |s| s.borrow().started)
    }

    /// The voice in sound slot `slot` of the speaker, if one plays.
    fn slot_voice(&self, slot: usize) -> Option<u32> {
        let ghost = self.speaker()?;
        WorldSounds::of(self.client.vm())?
            .borrow()
            .slot_voice(ghost, slot)
    }

    /// Where the client's mixer plays voice `handle`, if it does.
    fn voice_position(&self, handle: u32) -> Option<[f32; 3]> {
        let host = AudioPlugin::host(self.client.vm())?;
        let h = host.borrow();
        let a = h.lock();
        let i = a.find(handle)?;
        a.is_playing(handle).then(|| a.sources[i].voice.position)
    }

    fn sources_in_use(&self) -> usize {
        AudioPlugin::host(self.client.vm()).map_or(0, |h| h.borrow().lock().sources_in_use())
    }
}

fn near(a: [f32; 3], b: [f32; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.05)
}

fn a_slot_sound_plays_at_the_object_and_follows_it(protocol: ProtocolKind) {
    let mut rig = Rig::new(protocol);
    rig.eval("makeSpeaker(3, 4, 1);");
    assert!(
        rig.run_until(50, |r| r.speaker().is_some()),
        "{protocol:?}: the speaker is ghosted"
    );
    assert_eq!(rig.started(), 0);
    assert_eq!(rig.sources_in_use(), 0);

    // `playAudio` on the server: the slot comes with the next update, and the client plays the profile at the object.
    assert_eq!(rig.eval("return $Speaker.playAudio(1, Ping);"), "1");
    assert!(
        rig.run_until(20, |r| r.started() == 1),
        "{protocol:?}: the voice starts"
    );
    let voice = rig.slot_voice(1).expect("the slot's voice is kept");
    assert!(
        rig.voice_position(voice)
            .is_some_and(|p| near(p, [3.0, 4.0, 1.0])),
        "{protocol:?}: at the speaker: {:?}",
        rig.voice_position(voice)
    );
    assert_eq!(rig.sources_in_use(), 1);

    // The object moves: the voice follows it (`updateAudioPos`), with no new voice.
    rig.eval("$Speaker.setTransform(\"6 4 1 0 0 1 0\");");
    assert!(
        rig.run_until(20, |r| r
            .voice_position(voice)
            .is_some_and(|p| near(p, [6.0, 4.0, 1.0]))),
        "{protocol:?}: the voice follows: {:?}",
        rig.voice_position(voice)
    );
    assert_eq!(rig.started(), 1);

    // `stopAudio`: the voice stops.
    assert_eq!(rig.eval("return $Speaker.stopAudio(1);"), "1");
    assert!(
        rig.run_until(20, |r| r.slot_voice(1).is_none()),
        "{protocol:?}: stopped"
    );
    assert_eq!(rig.voice_position(voice), None);
    assert_eq!(rig.sources_in_use(), 0);

    // A slot past the four is refused; a profile that is not one plays nothing.
    assert_eq!(rig.eval("return $Speaker.playAudio(4, Ping);"), "0");
    assert_eq!(rig.eval("return $Speaker.stopAudio(4);"), "0");
    assert_eq!(
        rig.eval("return $Speaker.playAudio(0, PingDescription);"),
        "0"
    );
    rig.run(5);
    assert_eq!(rig.started(), 1);
}

fn a_one_shot_slot_frees_itself_on_the_server_and_a_looping_one_does_not(protocol: ProtocolKind) {
    let mut rig = Rig::new(protocol);
    rig.eval("makeSpeaker(0, 2, 1);");
    assert!(rig.run_until(50, |r| r.speaker().is_some()));
    // The same profile again in a playing slot is nothing new...
    rig.eval("$Speaker.playAudio(0, Ping); $Speaker.playAudio(0, Ping);");
    assert!(rig.run_until(20, |r| r.started() == 1));
    rig.run(5);
    assert_eq!(rig.started(), 1, "{protocol:?}");
    // ...until the server's timeout (500 ms) has passed: the slot is free again, and the profile plays anew.
    rig.run(16);
    rig.eval("$Speaker.playAudio(0, Ping);");
    assert!(
        rig.run_until(20, |r| r.started() == 2),
        "{protocol:?}: played again"
    );
    // A looping profile never times out: it is still playing to the server, so the same profile does nothing.
    rig.eval("$Speaker.playAudio(2, Hum);");
    assert!(
        rig.run_until(20, |r| r.started() == 3),
        "{protocol:?}: the loop starts"
    );
    rig.run(40);
    rig.eval("$Speaker.playAudio(2, Hum);");
    rig.run(10);
    assert_eq!(
        rig.started(),
        3,
        "{protocol:?}: the loop was not started again"
    );
    let loop_voice = rig.slot_voice(2).expect("the loop's voice");
    assert!(rig.voice_position(loop_voice).is_some());

    // The object goes: its voices stop with it.
    rig.eval("$Speaker.delete();");
    assert!(
        rig.run_until(20, |r| r.speaker().is_none()),
        "{protocol:?}: the ghost went"
    );
    rig.run(2);
    assert_eq!(
        rig.voice_position(loop_voice),
        None,
        "{protocol:?}: stopped with the ghost"
    );
    assert_eq!(
        WorldSounds::of(rig.client.vm())
            .unwrap()
            .borrow()
            .slot_voices(),
        0
    );
}

fn a_slot_playing_before_the_client_saw_the_object_comes_with_its_first_update(
    protocol: ProtocolKind,
) {
    let mut rig = Rig::new(protocol);
    // The slot is set before any update of the object went out: the initial update carries it.
    rig.eval("makeSpeaker(1, 1, 1); $Speaker.playAudio(3, Hum);");
    assert!(
        rig.run_until(50, |r| r.started() == 1),
        "{protocol:?}: the initial update's sound plays"
    );
    let voice = rig.slot_voice(3).expect("kept under its slot");
    assert!(
        rig.voice_position(voice)
            .is_some_and(|p| near(p, [1.0, 1.0, 1.0]))
    );
}

/// Two updates of the slot read by the client in one step, as two effects of one batch.
fn two_slot_updates_in_one_batch(rig: &mut Rig, first: &str, second: &str) {
    let before = rig.effect_seq();
    rig.eval(first);
    rig.server_ticks(1);
    rig.eval(second);
    rig.server_ticks(1);
    rig.client.step(32);
    assert_eq!(
        rig.effect_seq(),
        before + 2,
        "both updates came in one batch"
    );
}

fn a_slot_played_and_stopped_in_one_batch_ends_stopped(protocol: ProtocolKind) {
    let mut rig = Rig::new(protocol);
    rig.eval("makeSpeaker(0, 2, 1);");
    assert!(rig.run_until(50, |r| r.speaker().is_some()));
    two_slot_updates_in_one_batch(
        &mut rig,
        "$Speaker.playAudio(0, Hum);",
        "$Speaker.stopAudio(0);",
    );
    assert_eq!((rig.started(), rig.stopped()), (1, 1), "{protocol:?}");
    assert_eq!(rig.slot_voices(), 0, "{protocol:?}");
    assert_eq!(
        rig.sources_in_use(),
        0,
        "{protocol:?}: the loop was stopped"
    );
}

fn a_slot_played_and_replaced_in_one_batch_keeps_the_last_voice(protocol: ProtocolKind) {
    let mut rig = Rig::new(protocol);
    rig.eval("makeSpeaker(0, 2, 1);");
    assert!(rig.run_until(50, |r| r.speaker().is_some()));
    two_slot_updates_in_one_batch(
        &mut rig,
        "$Speaker.playAudio(0, Hum);",
        "$Speaker.playAudio(0, Ping);",
    );
    assert_eq!((rig.started(), rig.stopped()), (2, 1), "{protocol:?}");
    assert_eq!(
        rig.sources_in_use(),
        1,
        "{protocol:?}: the first voice went"
    );
    let voice = rig.slot_voice(0).expect("the second voice is kept");
    assert!(rig.voice_position(voice).is_some());
    let host = AudioPlugin::host(rig.client.vm()).unwrap();
    let h = host.borrow();
    let a = h.lock();
    let i = a.find(voice).unwrap();
    assert!(
        !a.sources[i].voice.looping,
        "{protocol:?}: the one kept is the one-shot"
    );
}

fn a_slot_played_on_a_ghost_gone_in_the_same_batch_plays_nothing(protocol: ProtocolKind) {
    let mut rig = Rig::new(protocol);
    rig.eval("makeSpeaker(0, 2, 1);");
    assert!(rig.run_until(50, |r| r.speaker().is_some()));
    two_slot_updates_in_one_batch(
        &mut rig,
        "$Speaker.playAudio(0, Hum);",
        "$Speaker.delete();",
    );
    assert!(rig.speaker().is_none(), "{protocol:?}: the ghost went");
    assert_eq!(rig.slot_voices(), 0, "{protocol:?}");
    assert_eq!(
        rig.sources_in_use(),
        0,
        "{protocol:?}: no voice of a gone ghost"
    );
}

fn a_ghost_gone_without_its_effect_still_ends_its_voices(protocol: ProtocolKind) {
    let mut rig = Rig::new(protocol);
    rig.eval("makeSpeaker(0, 2, 1); $Speaker.playAudio(0, Hum);");
    assert!(rig.run_until(50, |r| r.started() == 1));
    let ghost = rig.speaker().expect("the speaker");
    // The ghost leaves the world with no effect to say so (as when the ring dropped it).
    {
        let mut st = rig.client.host.client.borrow_mut();
        let conn = st.conn.conn.as_mut().expect("connected");
        assert!(conn.world.ghosts.remove(&ghost).is_some());
    }
    rig.client.step(32);
    assert_eq!(rig.slot_voices(), 0, "{protocol:?}");
    assert_eq!(
        rig.sources_in_use(),
        0,
        "{protocol:?}: the loop was stopped"
    );
    assert_eq!(rig.stopped(), 1);
}

#[test]
fn tge_protocol_a_slot_played_and_stopped_in_one_batch_ends_stopped() {
    a_slot_played_and_stopped_in_one_batch_ends_stopped(ProtocolKind::Tge);
}

#[test]
fn engine_protocol_a_slot_played_and_stopped_in_one_batch_ends_stopped() {
    a_slot_played_and_stopped_in_one_batch_ends_stopped(ProtocolKind::Engine);
}

#[test]
fn tge_protocol_a_slot_played_and_replaced_in_one_batch_keeps_the_last_voice() {
    a_slot_played_and_replaced_in_one_batch_keeps_the_last_voice(ProtocolKind::Tge);
}

#[test]
fn engine_protocol_a_slot_played_and_replaced_in_one_batch_keeps_the_last_voice() {
    a_slot_played_and_replaced_in_one_batch_keeps_the_last_voice(ProtocolKind::Engine);
}

#[test]
fn tge_protocol_a_slot_played_on_a_ghost_gone_in_the_same_batch_plays_nothing() {
    a_slot_played_on_a_ghost_gone_in_the_same_batch_plays_nothing(ProtocolKind::Tge);
}

#[test]
fn engine_protocol_a_slot_played_on_a_ghost_gone_in_the_same_batch_plays_nothing() {
    a_slot_played_on_a_ghost_gone_in_the_same_batch_plays_nothing(ProtocolKind::Engine);
}

#[test]
fn tge_protocol_a_ghost_gone_without_its_effect_still_ends_its_voices() {
    a_ghost_gone_without_its_effect_still_ends_its_voices(ProtocolKind::Tge);
}

#[test]
fn engine_protocol_a_ghost_gone_without_its_effect_still_ends_its_voices() {
    a_ghost_gone_without_its_effect_still_ends_its_voices(ProtocolKind::Engine);
}

#[test]
fn tge_protocol_a_slot_sound_plays_at_the_object_and_follows_it() {
    a_slot_sound_plays_at_the_object_and_follows_it(ProtocolKind::Tge);
}

#[test]
fn engine_protocol_a_slot_sound_plays_at_the_object_and_follows_it() {
    a_slot_sound_plays_at_the_object_and_follows_it(ProtocolKind::Engine);
}

#[test]
fn tge_protocol_a_one_shot_slot_frees_itself_and_a_looping_one_does_not() {
    a_one_shot_slot_frees_itself_on_the_server_and_a_looping_one_does_not(ProtocolKind::Tge);
}

#[test]
fn engine_protocol_a_one_shot_slot_frees_itself_and_a_looping_one_does_not() {
    a_one_shot_slot_frees_itself_on_the_server_and_a_looping_one_does_not(ProtocolKind::Engine);
}

#[test]
fn tge_protocol_a_slot_playing_before_the_client_saw_the_object_comes_with_its_first_update() {
    a_slot_playing_before_the_client_saw_the_object_comes_with_its_first_update(ProtocolKind::Tge);
}

#[test]
fn engine_protocol_a_slot_playing_before_the_client_saw_the_object_comes_with_its_first_update() {
    a_slot_playing_before_the_client_saw_the_object_comes_with_its_first_update(
        ProtocolKind::Engine,
    );
}
