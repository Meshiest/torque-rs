//! `ambient_generic`: a sound the map starts, stops and changes through inputs, as events a host plays
//! ([`crate::MapLogic::take_sounds`]). The bus does not read sound files: `message` is handed on as the map names it (a sound
//! file or a sound script entry), for the host's own sound support to look up.
//!
//! Keys: `message` the sound, `health` the volume (0 to 10), `pitch` (100 normal; 1 to 255), `radius` (units it is heard
//! within), `SourceEntityName` (the entity it plays at, else its own origin). Spawnflags: 1 heard everywhere, 16 starts silent,
//! 32 not looped (both on by default). Inputs: `PlaySound`, `StopSound`, `ToggleSound`, `Volume` (0 to 10; 0 stops it),
//! `Pitch`, `FadeIn` and `FadeOut` (seconds). A sound that is not silent at the start plays on the first tick.
//! [`crate::MapLogic::playing_sounds`] lists the looped sounds playing now, for a client that joins late.

use crate::bus::{Ctx, EntityId, MapLogic};
use crate::entities::Keys;
use crate::value::atof32;

/// What a sound event asks of the host.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SoundAction {
    Play,
    Stop,
    /// Volume or pitch changed while playing.
    Change,
    /// Rises to its volume over these seconds.
    FadeIn(f32),
    /// Falls to silence over these seconds, then stops.
    FadeOut(f32),
}

/// A sound event.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundEvent {
    pub entity: EntityId,
    pub action: SoundAction,
    /// The sound as the map names it.
    pub sound: String,
    /// Where it plays (the map's space).
    pub origin: [f32; 3],
    /// 0 to 1.
    pub volume: f32,
    /// 1 normal.
    pub pitch: f32,
    pub radius: f32,
    pub looped: bool,
    pub everywhere: bool,
}

/// `ambient_generic`'s state.
#[derive(Debug, Clone, PartialEq)]
pub struct Ambient {
    pub sound: String,
    pub volume: f32,
    pub pitch: f32,
    pub radius: f32,
    pub looped: bool,
    pub everywhere: bool,
    pub silent_start: bool,
    pub playing: bool,
    /// The entity it plays at (lowercase name), or empty.
    pub source: String,
}

impl Ambient {
    pub(crate) fn new(k: &mut Keys<'_>) -> Ambient {
        let flags = k.ent.spawnflags;
        Ambient {
            sound: k.str("message").trim().to_string(),
            volume: (k.f32("health") / 10.0).clamp(0.0, 1.0),
            pitch: (k.f32("pitch").clamp(1.0, 255.0)) / 100.0,
            radius: k.f32("radius").max(0.0),
            looped: flags & 32 == 0,
            everywhere: flags & 1 != 0,
            silent_start: flags & 16 != 0,
            playing: false,
            source: k.str("SourceEntityName").trim().to_ascii_lowercase(),
        }
    }

    fn origin(&self, ml: &MapLogic, me: EntityId) -> [f32; 3] {
        if !self.source.is_empty()
            && let Some(id) = ml.find(&self.source).first()
            && let Some(e) = ml.entity(*id)
        {
            return e.origin;
        }
        ml.entity(me).map_or([0.0; 3], |e| e.origin)
    }

    pub(crate) fn event(&self, ml: &MapLogic, me: EntityId, action: SoundAction) -> SoundEvent {
        SoundEvent {
            entity: me,
            action,
            sound: self.sound.clone(),
            origin: self.origin(ml, me),
            volume: self.volume,
            pitch: self.pitch,
            radius: self.radius,
            looped: self.looped,
            everywhere: self.everywhere,
        }
    }

    fn send(&mut self, ml: &mut MapLogic, me: EntityId, action: SoundAction) {
        if self.sound.is_empty() {
            return;
        }
        match action {
            SoundAction::Play | SoundAction::FadeIn(_) => self.playing = self.looped,
            SoundAction::Stop | SoundAction::FadeOut(_) => self.playing = false,
            SoundAction::Change => {}
        }
        let ev = self.event(ml, me, action);
        if ml.sounds.len() < ml.limits.max_pending_reports {
            ml.sounds.push(ev);
        }
    }

    pub(crate) fn spawn(&mut self, ml: &mut MapLogic, me: EntityId) {
        if !self.silent_start {
            self.send(ml, me, SoundAction::Play);
        }
    }

    pub(crate) fn input(&mut self, ml: &mut MapLogic, ctx: Ctx, input: &str, value: &str) -> bool {
        let me = ctx.me;
        match input {
            "playsound" => self.send(ml, me, SoundAction::Play),
            "stopsound" => self.send(ml, me, SoundAction::Stop),
            "togglesound" => {
                let a = if self.playing {
                    SoundAction::Stop
                } else {
                    SoundAction::Play
                };
                self.send(ml, me, a);
            }
            "volume" => {
                self.volume = (atof32(value) / 10.0).clamp(0.0, 1.0);
                if self.volume <= 0.0 {
                    self.send(ml, me, SoundAction::Stop);
                } else if self.playing {
                    self.send(ml, me, SoundAction::Change);
                }
            }
            "pitch" => {
                self.pitch = atof32(value).clamp(1.0, 255.0) / 100.0;
                if self.playing {
                    self.send(ml, me, SoundAction::Change);
                }
            }
            "fadein" => self.send(ml, me, SoundAction::FadeIn(atof32(value).max(0.0))),
            "fadeout" => self.send(ml, me, SoundAction::FadeOut(atof32(value).max(0.0))),
            _ => return false,
        }
        true
    }
}
