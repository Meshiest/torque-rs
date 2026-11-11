//! The world's sounds on the client's mixer: what the server's updates and events ask the client to play, played through
//! [`AudioPlugin`]'s host as TGE's client plays them.
//!
//! At the end of each tick of TGE's connection ([`tick`], the `TgeTicked` hook the client builder sets; the plugins' ticks
//! come after it): the effects the world queued since the last pass ([`ClientWorld::effects_after`](torque_ghost_world::ClientWorld::effects_after)) are played or stopped, the slot
//! sounds follow their ghosts (`ShapeBase::updateAudioPos`) and the listener moves to the eye.
//!
//! - [`Effect::ShapeSound`] (a ShapeBase sound slot, `playAudio`/`stopAudio` as the update carried it): the slot's voice
//!   stops, and with a profile a new one plays at the ghost (`ShapeBase::updateAudioState`: `alxPlay(profile,
//!   &getTransform())`), kept by ghost and slot so the next tick moves it with the ghost and a later update or the ghost's
//!   end stops it.
//! - [`Effect::Sound2D`], [`Effect::Sound3D`] (the server's audio events): a one-shot where the listener is, or at the
//!   event's point.
//! - [`Effect::Image`] with [`ImageEvent::Sound`]: a mounted image's state sound, a one-shot at the ghost.
//! - [`Effect::GhostGone`]: the ghost's slot voices stop; so do the voices of any ghost the world no longer holds, should
//!   the effect have been dropped.
//!
//! The effects run in their order, so a slot played and stopped (or replaced, or whose ghost went) in one batch ends as the
//! last one says.
//!
//! The profile is the server's datablock id. It plays nothing unless the object the client holds under that id is an
//! AudioProfile (the client registers its copy of each datablock under the server's id); its description comes from the
//! world's datablock record ([`ClientDatablock::audio_description`]: a remote client's profile object cannot name it).
//!
//! A new connection numbers its effects from 1 and holds other ghosts: the old world's slot voices stop and the count
//! starts again (the connect count of [`ClientState`] tells one world from the next).
//!
//! Credit: follows TGE's client as Torque3D (MIT) keeps it in `T3D/shapeBase.cpp` (`updateAudioState`, `updateAudioPos`)
//! and `T3D/gameBase/gameConnectionEvents.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic,
//! Inc.). The code is this crate's own.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use torque_audio::al::AL_POSITION;
use torque_audio::natives::{self, AudioHost};
use torque_audio::plugin::AudioPlugin;
use torque_client::conn::tge::{TgeConn, TgeWorld};
use torque_client::state::ClientState;
use torque_ghost_world::effects::Effect;
use torque_ghost_world::images::ImageEvent;
use torque_ghost_world::set::{ClientDatablock, ClientSet};
use torque_ghost_world::tge::TgeClientSet;
use torque_render_scene::math::Mat;
use torque_ts_vm::vm::Vm;

/// What the client keeps of the world's sounds: the effects taken so far and the slot voices playing, with counts a test
/// reads. A script-engine extension of the client ([`WorldSounds::of`]).
#[derive(Debug, Default)]
pub struct WorldSounds {
    /// The number of the last effect taken.
    seen: u64,
    /// The connect count the slot voices belong to.
    connects: u64,
    /// The voice of each ShapeBase sound slot playing, by ghost index and slot.
    slots: HashMap<(u32, usize), u32>,
    /// Voices started.
    pub started: u64,
    /// Voices stopped by an update, a ghost's end or a new world (not ones that ended on their own).
    pub stopped: u64,
}

impl WorldSounds {
    /// The client's world sounds, once its first tick ran (None before, or for a client without the audio plugin).
    pub fn of(vm: &Vm) -> Option<Rc<RefCell<WorldSounds>>> {
        vm.extension::<RefCell<WorldSounds>>()
    }

    /// The voice playing in sound slot `slot` of ghost `ghost`, if one is.
    pub fn slot_voice(&self, ghost: u32, slot: usize) -> Option<u32> {
        self.slots.get(&(ghost, slot)).copied()
    }

    /// How many slot voices are kept.
    pub fn slot_voices(&self) -> usize {
        self.slots.len()
    }
}

/// The host as the natives reach it: the state is the host itself.
fn own(host: &mut AudioHost) -> &mut AudioHost {
    host
}

/// Where ghost `ghost` is in the world (a child on its parent), when the world knows.
fn position(world: &TgeWorld, ghost: u32) -> Option<[f32; 3]> {
    let (m, _) = world.world_pose(ghost, &|_, _| None)?;
    Some([m[3], m[7], m[11]])
}

/// The listener's place and facing at the eye: TGE's camera transform as the listener's (`alxListenerMatrixF`): its
/// forward (+y turned by the pitch and yaw) is the "at" vector, its +z the "up".
fn listener(world: &TgeWorld) -> Option<([f32; 3], [f32; 6])> {
    let eye = world.eye()?;
    let camera = Mat::mul(&Mat::euler_z(eye.yaw), &Mat::euler_x(eye.pitch));
    let (at, up) = (camera.column(1), camera.column(2));
    Some((eye.position, [at[0], at[1], at[2], up[0], up[1], up[2]]))
}

/// Stops voice `h`.
fn stop(host: &Rc<RefCell<AudioHost>>, s: &mut WorldSounds, h: u32) {
    host.borrow().lock().stop(h);
    s.stopped += 1;
}

/// Plays `profile` at `at` (None: where the listener is), kept under `key` for a slot sound (a voice already under the key
/// stops). Nothing unless `profile` is an AudioProfile the client holds.
fn play(
    vm: &mut Vm,
    host: &Rc<RefCell<AudioHost>>,
    s: &mut WorldSounds,
    world: &TgeWorld,
    profile: u32,
    at: Option<[f32; 3]>,
    key: Option<(u32, usize)>,
) {
    let description = world
        .datablocks
        .get(&profile)
        .and_then(ClientDatablock::audio_description);
    let (h, _) = natives::play_profile_at(vm, host, own, profile, description, at);
    if h == 0 {
        return;
    }
    s.started += 1;
    if let Some(key) = key
        && let Some(old) = s.slots.insert(key, h)
    {
        stop(host, s, old);
    }
}

/// Stops the slot voices of ghost `ghost`.
fn stop_ghost(host: &Rc<RefCell<AudioHost>>, s: &mut WorldSounds, ghost: u32) {
    let keys: Vec<(u32, usize)> = s.slots.keys().filter(|k| k.0 == ghost).copied().collect();
    for key in keys {
        if let Some(h) = s.slots.remove(&key) {
            stop(host, s, h);
        }
    }
}

/// One effect of the world, in its order: what came before it has already been played or stopped.
fn apply(
    vm: &mut Vm,
    host: &Rc<RefCell<AudioHost>>,
    s: &mut WorldSounds,
    world: &TgeWorld,
    effect: &Effect<<TgeClientSet as ClientSet>::Effect>,
) {
    match *effect {
        Effect::Sound2D { profile } => play(vm, host, s, world, profile, None, None),
        Effect::Sound3D {
            profile, position, ..
        } => play(vm, host, s, world, profile, Some(position), None),
        Effect::Image {
            ghost,
            event: ImageEvent::Sound { profile },
            ..
        } => play(vm, host, s, world, profile, position(world, ghost), None),
        Effect::ShapeSound {
            ghost,
            slot,
            profile,
        } => {
            if let Some(h) = s.slots.remove(&(ghost, slot)) {
                stop(host, s, h);
            }
            // A slot of a ghost the world no longer holds (it went in the same batch) plays nothing.
            if let Some(profile) = profile
                && let Some(at) = position(world, ghost)
            {
                play(vm, host, s, world, profile, Some(at), Some((ghost, slot)));
            }
        }
        Effect::GhostGone { ghost } => stop_ghost(host, s, ghost),
        Effect::Image { .. } | Effect::Explosion { .. } | Effect::Game(_) => {}
    }
}

/// The world's sounds for this tick (see the module docs). Nothing without the audio plugin.
pub fn tick(vm: &mut Vm, client: &Rc<RefCell<ClientState<TgeConn>>>) {
    let Some(host) = AudioPlugin::host(vm) else {
        return;
    };
    let sounds = WorldSounds::of(vm).unwrap_or_else(|| {
        let s = Rc::new(RefCell::new(WorldSounds::default()));
        vm.set_extension(Rc::clone(&s));
        s
    });
    let mut s = sounds.borrow_mut();
    let st = client.borrow();
    let conn = st.conn.conn.as_deref();
    // Another world: what was kept of the old one ends.
    if st.conn.connects != s.connects || conn.is_none_or(|c| c.world.effect_seq < s.seen) {
        let old: Vec<u32> = s.slots.drain().map(|(_, h)| h).collect();
        for h in old {
            stop(&host, &mut s, h);
        }
        s.seen = 0;
        s.connects = st.conn.connects;
    }
    let Some(c) = conn else {
        return;
    };
    let world = &c.world;
    // The effects in their order: a slot played, replaced and stopped in one batch ends stopped.
    for e in world.effects_after(s.seen) {
        apply(vm, &host, &mut s, world, &e.effect);
    }
    s.seen = world.effect_seq;
    // A ghost gone without a word (its effect dropped past the ring's cap): its voices end all the same.
    let gone: Vec<u32> = s
        .slots
        .keys()
        .map(|k| k.0)
        .filter(|g| !world.ghosts.contains_key(g))
        .collect();
    for ghost in gone {
        stop_ghost(&host, &mut s, ghost);
    }
    let h = host.borrow();
    let mut a = h.lock();
    for (&(ghost, _), &handle) in &s.slots {
        if let Some(at) = position(world, ghost) {
            a.source_3f(handle, AL_POSITION, at);
        }
    }
    if let Some((at, orientation)) = listener(world) {
        a.listener_3f(AL_POSITION, at);
        a.listener.orientation = orientation;
    }
    // A one-shot that ended freed its source: its slot is free too.
    s.slots.retain(|_, h| a.find(*h).is_some());
}

/// Opens the default sound card for the client's audio host (a window client, feature `audio-device`): the line to print,
/// naming the device, or why none plays. Without one the host stays on the null sink and the game plays on, silent.
#[cfg(feature = "audio-device")]
pub fn open_device(vm: &Vm) -> String {
    let Some(host) = AudioPlugin::host(vm) else {
        return "sound: no audio plugin (silent)".to_string();
    };
    match host.borrow_mut().open_device() {
        Ok(name) => format!("sound: {name}"),
        Err(why) => format!("sound: no output device ({why}); playing on, silent"),
    }
}
