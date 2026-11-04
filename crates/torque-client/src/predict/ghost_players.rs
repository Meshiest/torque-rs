//! The other Players' ghosts between the server's updates, as TGE's client moves a Player ghost: run on from the last update
//! along its velocity, and an update that lands somewhere else warped to over a few ticks instead of snapped to. The client
//! draws each one where [`GhostPlayers::frame`] puts it ([`ghost_world::ClientWorld::ghost_render`]).
//!
//! - **Between updates** the Player moves on along the last update's velocity (TGE's client ticks the ghost on with its last move,
//!   `mPredictionCount` ticks at most), for at most [`MAX_EXTRAPOLATION`] past the time the next update is due: a Player whose
//!   updates stop (it stood still) stops too.
//! - **An update** sets the velocity, and the difference between where the Player is drawn and where the update puts it (the
//!   position, the yaw and the head's pitch) is spread over the time the next update is due in, at least a tick and at most
//!   [`MAX_WARP`] (`Player::unpackUpdate`'s warp: `delta.warpOffset` over `warpTicks`, at most `sMaxWarpTicks`). A Player that
//!   turns at a steady rate turns at that rate on the client, with no step at each update. More than [`SNAP_DISTANCE`] away is
//!   a respawn: the Player is put there at once.
//! - **The time between updates** is the running average of the last ones (a server sends a client packets at its own rate, not
//!   every tick).
//! - **Which Players**: every Player and AIPlayer ghost but the control object (the prediction draws it), one the client already
//!   runs itself (a corpse its death camera watches) and a mounted one (drawn at its mount).
//!
//! **Engine limits**: TGE's client runs the ghost's own `processTick` with its last move (collisions, gravity); the client here
//! runs it on along the velocity only.
//!
//! Credit: the warp follows `Player::unpackUpdate` and `Player::processTick`'s ghost branch in Torque3D's MIT-licensed
//! `T3D/player.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The code here is this crate's own.

use std::collections::BTreeMap;
use std::f32::consts::{PI, TAU};

use ghost_world::world::Pose;
use torque_server::server::TICK_MS;

use super::game::PredictWorld;
use crate::records::{EngineRecord, EngineSet};

/// A tick (seconds).
pub const TICK: f32 = TICK_MS as f32 / 1000.0;
/// The longest a warp lasts (seconds; TGE's `sMaxWarpTicks` is 3 ticks, its updates come every tick).
pub const MAX_WARP: f32 = 0.25;
/// How long past the due time of the next update a Player still runs on along its velocity (seconds).
pub const MAX_EXTRAPOLATION: f32 = 0.1;
/// An update further than this from where the Player is drawn is a respawn or a teleport (units).
pub const SNAP_DISTANCE: f32 = 4.0;
/// The time between updates assumed before two came (seconds; the engine's server sends ten packets a second).
pub const FIRST_INTERVAL: f32 = 0.1;
/// The most Players run at once.
pub const MAX_GHOST_PLAYERS: usize = 256;

/// What an update of a Player's record says.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Update {
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub yaw: f32,
    /// The head's pitch as the record's fraction of the look range (-1 up to 1 down).
    pub head: f32,
}

/// A Player's motion as the client draws it.
#[derive(Debug, Clone, PartialEq)]
pub struct GhostMotion {
    position: [f32; 3],
    velocity: [f32; 3],
    yaw: f32,
    head: f32,
    last: Update,
    /// The warp: what it adds a second, and the seconds it has left.
    warp_position: [f32; 3],
    warp_yaw: f32,
    warp_head: f32,
    warp_left: f32,
    /// Seconds the Player still runs on along its velocity.
    run_left: f32,
    /// Seconds since the last update, and the average time between updates.
    since: f32,
    interval: f32,
}

/// `a` turned into `-pi..pi`.
pub fn wrap_angle(a: f32) -> f32 {
    let a = (a + PI).rem_euclid(TAU) - PI;
    if a.is_finite() { a } else { 0.0 }
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f32>().sqrt()
}

impl GhostMotion {
    /// A Player where its first update puts it.
    pub fn new(u: Update) -> GhostMotion {
        GhostMotion {
            position: u.position,
            velocity: u.velocity,
            yaw: u.yaw,
            head: u.head,
            last: u,
            warp_position: [0.0; 3],
            warp_yaw: 0.0,
            warp_head: 0.0,
            warp_left: 0.0,
            run_left: FIRST_INTERVAL + MAX_EXTRAPOLATION,
            since: 0.0,
            interval: FIRST_INTERVAL,
        }
    }

    /// The record as it stands: a new update when it differs from the last one.
    pub fn receive(&mut self, u: Update) {
        if u == self.last {
            return;
        }
        self.last = u;
        if self.since > 0.0 {
            self.interval = (self.interval * 3.0 + self.since.min(1.0)) * 0.25;
        }
        self.since = 0.0;
        self.velocity = u.velocity;
        self.run_left = self.interval + MAX_EXTRAPOLATION;
        let far = distance(u.position, self.position);
        if far.is_nan() || far > SNAP_DISTANCE {
            *self = GhostMotion {
                interval: self.interval,
                ..GhostMotion::new(u)
            };
            return;
        }
        let span = self.interval.clamp(TICK, MAX_WARP);
        self.warp_position = [0, 1, 2].map(|i| (u.position[i] - self.position[i]) / span);
        self.warp_yaw = wrap_angle(u.yaw - self.yaw) / span;
        self.warp_head = (u.head - self.head) / span;
        self.warp_left = span;
    }

    /// `dt` seconds on (at most a second).
    pub fn advance(&mut self, dt: f32) {
        let dt = if dt.is_finite() {
            dt.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.since += dt;
        let run = dt.min(self.run_left);
        self.run_left -= run;
        let warp = dt.min(self.warp_left);
        self.warp_left -= warp;
        for i in 0..3 {
            self.position[i] += self.velocity[i] * run + self.warp_position[i] * warp;
        }
        self.yaw = wrap_angle(self.yaw + self.warp_yaw * warp);
        self.head += self.warp_head * warp;
    }

    pub fn position(&self) -> [f32; 3] {
        self.position
    }

    pub fn yaw(&self) -> f32 {
        self.yaw
    }

    /// The head's pitch as a fraction of the look range.
    pub fn head(&self) -> f32 {
        self.head
    }

    /// The velocity the Player moves at now (none once it stopped running on).
    pub fn velocity(&self) -> [f32; 3] {
        if self.run_left > 0.0 {
            self.velocity
        } else {
            [0.0; 3]
        }
    }
}

/// The other Players the client runs between their updates, by ghost index, and the virtual time of the last frame.
#[derive(Debug, Clone, Default)]
pub struct GhostPlayers {
    pub players: BTreeMap<u32, GhostMotion>,
    last_ms: Option<u32>,
}

impl GhostPlayers {
    /// A frame at virtual time `now_ms`: each Player moved on by the time since the last frame to its record as it stands, and
    /// put in `world`'s `ghost_render` (where the client draws it; the pose's pitch is the record's).
    pub fn frame<W: PredictWorld>(&mut self, world: &mut W, now_ms: u32) {
        let dt = self
            .last_ms
            .map_or(0.0, |l| now_ms.saturating_sub(l) as f32 / 1000.0);
        self.last_ms = Some(now_ms);
        let control = world.control.object;
        let mut seen = Vec::new();
        let mut placed = Vec::new();
        for (&index, g) in &world.ghosts {
            let player = g
                .class
                .map(<W::Set as EngineSet>::class_kind)
                .is_some_and(|k| k.is_player());
            if !player
                || control == Some(index)
                || world.ghost_render.contains_key(&index)
                || world.mount_of(index).is_some()
            {
                continue;
            }
            let (
                Some(Pose::Euler {
                    position,
                    pitch,
                    yaw,
                }),
                Some((velocity, head)),
            ) = (g.pose(), g.record.as_ref().and_then(|r| r.player_motion()))
            else {
                continue;
            };
            let u = Update {
                position,
                velocity,
                yaw,
                head,
            };
            if !self.players.contains_key(&index) && self.players.len() >= MAX_GHOST_PLAYERS {
                continue;
            }
            let m = self
                .players
                .entry(index)
                .or_insert_with(|| GhostMotion::new(u));
            m.receive(u);
            m.advance(dt);
            seen.push(index);
            placed.push((
                index,
                Pose::Euler {
                    position: m.position(),
                    pitch,
                    yaw: m.yaw(),
                },
            ));
        }
        self.players.retain(|i, _| seen.contains(i));
        world.ghost_render.extend(placed);
    }

    /// Player `index` as the last frame drew it, if the client runs it.
    pub fn get(&self, index: u32) -> Option<&GhostMotion> {
        self.players.get(&index)
    }
}

#[cfg(test)]
#[path = "ghost_players_tests.rs"]
mod tests;
