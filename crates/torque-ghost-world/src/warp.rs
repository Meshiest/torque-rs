//! A ghost shown between its server's updates: from where the client drew it when an update came to where the update put it,
//! over the time the updates are apart (at least a tick and at most three, the bounds of `Vehicle::unpackUpdate`'s warp), and held
//! there until the next one. It is the drawn pose of a plugin ghost that only has a transform ([`PoseWarps`]); TGE's
//! classes that move have their own rules, and a ghost that does not move is drawn exactly where its record puts it, with no
//! rounding.
//!
//! Credit: bounded as TGE 1.4's `Vehicle::unpackUpdate` warp (`game/vehicles/vehicle.cc`), Copyright (C) GarageGames.com, Inc.;
//! as Torque3D carries it under the MIT license: `T3D/vehicles/vehicle.cpp` (Copyright (c) 2012 GarageGames, LLC). The code here is
//! this crate's own.

use std::collections::BTreeMap;
use std::sync::Mutex;

use collide::math::quat::slerp_f32;
use ghosts::Xf;

use crate::images::TICK_MS;
use crate::world::Pose;

/// The most ticks a warp lasts.
pub const MAX_WARP_TICKS: u32 = 3;

/// A pose as a position and a rotation `x, y, z, w`.
type Parts = ([f32; 3], [f32; 4]);

/// A transform record's position and rotation (`Pose::Affine`'s: the quaternion's `w` from the other three and its sign).
fn parts(xf: &Xf) -> Parts {
    let [x, y, z] = xf.quat.map(|v| v.get());
    let mut w = (1.0 - (x * x + y * y + z * z)).max(0.0).sqrt();
    if xf.negative_w {
        w = -w;
    }
    (xf.position.map(|v| v.get()), [x, y, z, w])
}

/// One ghost's warp.
#[derive(Debug, Clone, PartialEq)]
struct Warp {
    /// The ghost's birth ([`crate::world::ClientWorld::birth_order`]): a ghost made again on its index starts over.
    birth: u64,
    /// The ghost's updates so far when the warp began: another count is a new update.
    updates: u32,
    from: Parts,
    to: Parts,
    /// The client's clock (ms) when the warp began, how long it lasts, and when the update before it came.
    at_ms: u32,
    warp_ms: f32,
    last_ms: u32,
}

impl Warp {
    fn at(&self, now_ms: u32) -> Parts {
        let t = now_ms.wrapping_sub(self.at_ms) as f32;
        if self.warp_ms <= 0.0 || t >= self.warp_ms {
            return self.to;
        }
        let f = (t / self.warp_ms).clamp(0.0, 1.0);
        let p = std::array::from_fn(|k| self.from.0[k] + (self.to.0[k] - self.from.0[k]) * f);
        (p, slerp_f32(self.from.1, self.to.1, f))
    }
}

/// The warps of a world's ghosts, by ghost index. Asked as a frame draws, so it is kept behind a lock: asking twice at one time
/// gives one answer, and it takes no part in comparing two worlds.
#[derive(Debug, Default)]
pub struct PoseWarps(Mutex<BTreeMap<u32, Warp>>);

impl PoseWarps {
    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<u32, Warp>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Where ghost `index` is drawn at `now_ms` when its record puts it at `target` after `updates` updates (born as `birth`): the
    /// pose it is warping through, or `target` itself once the warp is over (and for any pose that is not a transform record's).
    pub fn pose(&self, index: u32, birth: u64, updates: u32, target: Pose, now_ms: u32) -> Pose {
        let Pose::Affine { xf, .. } = &target else {
            return target;
        };
        let to = parts(xf);
        if !to.0.iter().chain(&to.1).all(|c| c.is_finite()) {
            return target;
        }
        let mut warps = self.lock();
        let warp = warps.entry(index).or_insert_with(|| Warp {
            birth,
            updates,
            from: to,
            to,
            at_ms: now_ms,
            warp_ms: 0.0,
            last_ms: now_ms,
        });
        if warp.birth != birth {
            *warp = Warp {
                birth,
                updates,
                from: to,
                to,
                at_ms: now_ms,
                warp_ms: 0.0,
                last_ms: now_ms,
            };
        } else if warp.updates != updates || warp.to != to {
            let from = warp.at(now_ms);
            let apart = now_ms.wrapping_sub(warp.last_ms);
            let ticks = apart.clamp(TICK_MS, MAX_WARP_TICKS * TICK_MS);
            *warp = Warp {
                birth,
                updates,
                from,
                to,
                at_ms: now_ms,
                warp_ms: if from == to { 0.0 } else { ticks as f32 },
                last_ms: now_ms,
            };
        }
        if warp.warp_ms <= 0.0 || now_ms.wrapping_sub(warp.at_ms) as f32 >= warp.warp_ms {
            return target;
        }
        let (position, rotation) = warp.at(now_ms);
        Pose::Quat { position, rotation }
    }

    /// Forgets the ghosts `keep` says are gone.
    pub fn retain(&self, mut keep: impl FnMut(u32) -> bool) {
        self.lock().retain(|i, _| keep(*i));
    }

    /// How many ghosts it holds a warp of.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }
}

impl Clone for PoseWarps {
    fn clone(&self) -> PoseWarps {
        PoseWarps(Mutex::new(self.lock().clone()))
    }
}

impl PartialEq for PoseWarps {
    fn eq(&self, _: &PoseWarps) -> bool {
        true
    }
}

#[cfg(test)]
#[path = "warp_tests.rs"]
mod tests;
