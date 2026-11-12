//! The GPU particle pool's side of the frame ([`HdScene::sprites`](crate::hd::HdScene::sprites)): the record table the pool's
//! particles name (a [`SpriteLook`] with its textures in the scene's table, its material word and its frames' buckets), the
//! buckets they draw in (a texture, a blend and whether lit: one indirect draw each), and the frame's hand-over (its spawns
//! naming records, the step, the clear).
//!
//! A record and a bucket keep their slot while a particle handed over can still name them (each spawn's time left on the
//! pool's clock): a table that is full ([`MAX_RECORDS`] records or [`MAX_BUCKETS`] buckets) frees the ones no live particle can
//! be in and gives their slots to new looks; a look still without room is hidden and counted in [`GpuSprites::overflow`].
//! Records are found by their bytes (`look_key`), and a clear empties the table with the pool. The same clock says which
//! buckets can hold a live particle, so the frame draws only those. A frame handed over must be drawn: one the renderer never
//! runs leaves its steps and spawns out of the pool, and its particles then outlive the clock's count.

use std::collections::HashMap;

use crate::hd::material;
use crate::hd::particle_light::ParticleLight;
use crate::particles::{GpuSpawn, SPRITE_SAMPLER, SpriteFrame, SpriteLook};
use crate::{Blend, Cull, Draw, Sampler, TexEnv};

/// The most records the table holds.
pub const MAX_RECORDS: usize = 4096;
/// The most buckets (one of them, when any look is hidden, the hidden looks' own, never drawn).
pub const MAX_BUCKETS: usize = 64;
/// The most steps a frame hands the pool (one an emitter `advance` since the last frame; a longer stall's are summed).
pub const MAX_STEPS: usize = 32;
/// The most texture frames a record steps through (a longer `animateTexture` list keeps its first ones).
pub const MAX_FRAMES: usize = 32;

/// What one indirect draw of the pool draws with: a texture of the scene's table (none: untextured), its blend, and whether
/// its particles are lit ([`material::LIT`]); `drawn` false for the hidden looks' bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpriteBucket {
    pub texture: Option<(usize, Sampler)>,
    pub blend: Blend,
    pub lit: bool,
    pub drawn: bool,
}

/// The hidden looks' bucket (one for every hidden look).
const HIDDEN: SpriteBucket = SpriteBucket {
    texture: None,
    blend: Blend::Alpha,
    lit: false,
    drawn: false,
};

/// A look as the pool reads it: the look (its frames indices into the scene's texture table), the vertex material word its
/// sprites are drawn with, and the bucket of each frame (one when it does not step through frames).
#[derive(Debug, Clone, PartialEq)]
pub struct SpriteRecord {
    pub look: SpriteLook,
    pub material: u32,
    pub buckets: Vec<u8>,
}

impl SpriteRecord {
    /// The record's bytes as the pool's `Look` holds them (160): the color keys, the size keys, the time keys, `drag, gravity,
    /// wind, fps`, `material, frame count, orient mode, 0`, and the frames' buckets a byte each.
    pub const BYTES: usize = 160;

    pub fn write(&self, out: &mut Vec<u8>) {
        let f = |out: &mut Vec<u8>, v: f32| out.extend_from_slice(&v.to_le_bytes());
        let u = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
        let l = &self.look;
        l.colors.iter().flatten().for_each(|&v| f(out, v));
        l.sizes.iter().for_each(|&v| f(out, v));
        l.times.iter().for_each(|&v| f(out, v));
        for v in [l.drag, l.gravity, l.wind, l.fps as f32] {
            f(out, v);
        }
        for v in [self.material, self.buckets.len() as u32, l.orient as u32, 0] {
            u(out, v);
        }
        let mut frames = [0u8; MAX_FRAMES];
        frames[..self.buckets.len()].copy_from_slice(&self.buckets);
        out.extend_from_slice(&frames);
    }
}

/// The pool's frame data: the record table and the buckets (kept), the frame's spawns (their `look` a record index), the
/// steps and the clear, and the counts the renderer compares.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GpuSprites {
    pub records: Vec<SpriteRecord>,
    pub buckets: Vec<SpriteBucket>,
    /// The frame's particles, each naming a record.
    pub spawns: Vec<GpuSpawn>,
    /// The frame's steps (ms, in order, at most [`MAX_STEPS`]: the rest summed into the last).
    pub steps: Vec<u32>,
    pub clear: bool,
    /// The air's velocity the frame's steps carry the pool's particles by (units a second).
    pub wind: [f32; 3],
    /// Counts the frames handed over (the renderer runs each once: a frame drawn again steps nothing).
    pub generation: u64,
    /// Counts the changes of the record table (the renderer uploads it again).
    pub records_generation: u64,
    /// The looks that found no room (hidden) and the spawns of them.
    pub overflow: u64,
    /// Every shift of the frame's positions summed ([`crate::hd::origin::Shift`]): the renderer moves the pool's particles by
    /// what it has not applied yet (the frame's spawns are shifted already).
    pub offset: [f64; 3],
    /// The pool's clock (ms, the steps summed).
    clock_ms: u64,
    /// Each record's key in `index`, the clock at which its last particle handed over dies, and whether its slot is free.
    keys: Vec<Vec<u8>>,
    record_until: Vec<u64>,
    record_free: Vec<bool>,
    /// The records by their key ([`look_key`]).
    index: HashMap<Vec<u8>, u32>,
    /// Per bucket: the clock at which its last particle handed over dies, the records naming it, whether its slot is free.
    live_until: Vec<u64>,
    bucket_refs: Vec<u32>,
    bucket_free: Vec<bool>,
}

impl GpuSprites {
    /// Takes `frame`: its looks into the table (each frame index `textures` maps to the scene's table; an unmapped one
    /// dropped), lit where `rule` says so, its spawns naming records. Replaces the last frame's spawns and counts a frame.
    /// A clear empties the table with the pool.
    pub fn push(
        &mut self,
        frame: &SpriteFrame,
        textures: impl Fn(usize) -> Option<usize>,
        rule: Option<ParticleLight>,
    ) {
        if frame.clear {
            self.reset();
        }
        self.steps.clear();
        for &ms in frame.steps.iter().filter(|&&ms| ms > 0) {
            match self.steps.len() {
                n if n < MAX_STEPS => self.steps.push(ms),
                _ => *self.steps.last_mut().expect("MAX_STEPS > 0") += ms,
            }
        }
        self.clock_ms += self.steps.iter().map(|&ms| u64::from(ms)).sum::<u64>();
        let records: Vec<Option<u32>> = frame
            .looks
            .iter()
            .map(|l| self.record(l, &textures, rule))
            .collect();
        self.spawns.clear();
        for s in &frame.spawns {
            let Some(r) = records.get(s.look as usize).copied().flatten() else {
                self.overflow += 1;
                continue;
            };
            let until = self.clock_ms + u64::try_from(s.life_ms - s.age_ms).unwrap_or(0) + 1;
            let at = r as usize;
            self.record_until[at] = self.record_until[at].max(until);
            for &b in &self.records[at].buckets {
                let u = &mut self.live_until[usize::from(b)];
                *u = (*u).max(until);
            }
            self.spawns.push(GpuSpawn { look: r, ..*s });
        }
        self.clear = frame.clear;
        self.wind = if frame.wind.iter().all(|c| c.is_finite()) {
            frame.wind
        } else {
            [0.0; 3]
        };
        self.generation = self.generation.wrapping_add(1);
    }

    /// The buckets that may hold a live particle, drawn ones only, in bucket order.
    pub fn live_buckets(&self) -> impl Iterator<Item = (usize, &SpriteBucket)> {
        self.buckets.iter().enumerate().filter(|(i, b)| {
            b.drawn && !self.bucket_free[*i] && self.live_until[*i] > self.clock_ms
        })
    }

    /// The records and buckets in use (their slots not free).
    pub fn in_use(&self) -> (usize, usize) {
        let used = |free: &[bool]| free.iter().filter(|f| !**f).count();
        (used(&self.record_free), used(&self.bucket_free))
    }

    /// The table emptied (the pool's particles are dropped with it).
    fn reset(&mut self) {
        if !self.records.is_empty() || !self.buckets.is_empty() {
            self.records_generation = self.records_generation.wrapping_add(1);
        }
        self.records.clear();
        self.buckets.clear();
        self.keys.clear();
        self.record_until.clear();
        self.record_free.clear();
        self.index.clear();
        self.live_until.clear();
        self.bucket_refs.clear();
        self.bucket_free.clear();
    }

    /// Frees the records none of whose particles can be alive, then the buckets no record names and no particle can be in:
    /// their slots go to the next new looks.
    fn collect(&mut self) {
        let now = self.clock_ms;
        for r in 0..self.records.len() {
            if self.record_free[r] || self.record_until[r] > now {
                continue;
            }
            self.record_free[r] = true;
            self.index.remove(&self.keys[r]);
            for &b in &self.records[r].buckets {
                let refs = &mut self.bucket_refs[usize::from(b)];
                *refs = refs.saturating_sub(1);
            }
        }
        for b in 0..self.buckets.len() {
            if !self.bucket_free[b] && self.bucket_refs[b] == 0 && self.live_until[b] <= now {
                self.bucket_free[b] = true;
            }
        }
    }

    /// `look`'s record (made when it is new; None when the table is full even after its free slots are collected).
    fn record(
        &mut self,
        look: &SpriteLook,
        textures: &impl Fn(usize) -> Option<usize>,
        rule: Option<ParticleLight>,
    ) -> Option<u32> {
        let mut look = look.clone();
        let named = look.frames.len();
        look.frames = look.frames.iter().filter_map(|&t| textures(t)).collect();
        look.frames.truncate(MAX_FRAMES);
        look.hidden |= named > 0 && look.frames.is_empty();
        if look.frames.len() < 2 {
            look.fps = 0;
        }
        let lit = !look.hidden && rule.is_some_and(|r| r.lights(&probe(&look)));
        let key = look_key(&look, lit);
        if let Some(&at) = self.index.get(&key) {
            // Named this frame: kept at least until its spawns are counted.
            let u = &mut self.record_until[at as usize];
            *u = (*u).max(self.clock_ms + 1);
            return Some(at);
        }
        let mut wanted = wanted_buckets(&look, lit);
        if !self.record_room() || !self.bucket_room(&wanted) {
            self.collect();
        }
        if !self.record_room() {
            self.overflow += 1;
            return None;
        }
        if !self.bucket_room(&wanted) {
            self.overflow += 1;
            look.hidden = true;
            wanted = vec![HIDDEN];
        }
        let buckets: Vec<u8> = wanted.into_iter().map(|b| self.bucket(b)).collect();
        let mut named = buckets.clone();
        named.sort_unstable();
        named.dedup();
        for &b in &named {
            self.bucket_refs[usize::from(b)] += 1;
        }
        let record = SpriteRecord {
            material: sprite_material(&look, lit && !look.hidden),
            look,
            buckets,
        };
        let at = match self.record_free.iter().position(|f| *f) {
            Some(at) => {
                self.records[at] = record;
                self.keys[at].clone_from(&key);
                self.record_free[at] = false;
                at
            }
            None => {
                self.records.push(record);
                self.keys.push(key.clone());
                self.record_free.push(false);
                self.record_until.push(0);
                self.records.len() - 1
            }
        };
        self.record_until[at] = self.clock_ms + 1;
        self.index.insert(key, at as u32);
        self.records_generation = self.records_generation.wrapping_add(1);
        Some(at as u32)
    }

    /// Whether a new record fits (a free slot, or room to grow).
    fn record_room(&self) -> bool {
        self.records.len() < MAX_RECORDS || self.record_free.iter().any(|f| *f)
    }

    /// The bucket in use equal to `b`.
    fn find_bucket(&self, b: &SpriteBucket) -> Option<usize> {
        (0..self.buckets.len()).find(|&i| !self.bucket_free[i] && self.buckets[i] == *b)
    }

    /// Whether the buckets of `wanted` not in use fit beside those that are (room kept for the hidden bucket until it is).
    fn bucket_room(&self, wanted: &[SpriteBucket]) -> bool {
        let new = wanted
            .iter()
            .filter(|b| self.find_bucket(b).is_none())
            .count();
        let used = self.bucket_free.iter().filter(|f| !**f).count();
        let reserve = usize::from(self.find_bucket(&HIDDEN).is_none() && !wanted.contains(&HIDDEN));
        new == 0 || used + new + reserve <= MAX_BUCKETS
    }

    /// `b`'s index: the one in use, else a free slot, else a new one (the caller checked the room).
    fn bucket(&mut self, b: SpriteBucket) -> u8 {
        if let Some(at) = self.find_bucket(&b) {
            return at as u8;
        }
        let at = match self.bucket_free.iter().position(|f| *f) {
            Some(at) => {
                self.buckets[at] = b;
                self.bucket_free[at] = false;
                self.live_until[at] = 0;
                at
            }
            None => {
                self.buckets.push(b);
                self.live_until.push(0);
                self.bucket_refs.push(0);
                self.bucket_free.push(false);
                self.buckets.len() - 1
            }
        };
        at as u8
    }
}

/// The buckets of `look`'s frames (the hidden bucket for a hidden look; one untextured for a look with no texture).
fn wanted_buckets(look: &SpriteLook, lit: bool) -> Vec<SpriteBucket> {
    if look.hidden {
        return vec![HIDDEN];
    }
    let of = |texture| SpriteBucket {
        texture,
        blend: look.blend,
        lit,
        drawn: true,
    };
    if look.frames.is_empty() {
        return vec![of(None)];
    }
    look.frames
        .iter()
        .map(|&t| of(Some((t, SPRITE_SAMPLER))))
        .collect()
}

/// A look's key in the table: its keys, coefficients, frames' rate, blend, whether it is lit or hidden, and its frames (the
/// label only decides whether it is lit).
fn look_key(look: &SpriteLook, lit: bool) -> Vec<u8> {
    let mut k = Vec::with_capacity(128 + 8 * look.frames.len());
    let floats = look
        .colors
        .iter()
        .flatten()
        .chain(&look.sizes)
        .chain(&look.times)
        .chain([&look.drag, &look.gravity, &look.wind]);
    for v in floats {
        k.extend_from_slice(&v.to_bits().to_le_bytes());
    }
    k.extend_from_slice(&look.fps.to_le_bytes());
    k.push(
        u8::from(look.blend == Blend::Additive)
            | (u8::from(lit) << 1)
            | (u8::from(look.hidden) << 2),
    );
    k.push(look.orient as u8);
    for &t in &look.frames {
        k.extend_from_slice(&(t as u64).to_le_bytes());
    }
    k
}

/// The draw a particle light rule is asked about for `look` (no vertices: a rule reads its label, blend and texture).
fn probe(look: &SpriteLook) -> Draw {
    Draw {
        label: look.label.clone(),
        vertices: Vec::new(),
        lighting: false,
        tex_env: if look.frames.is_empty() {
            TexEnv::None
        } else {
            TexEnv::Modulate
        },
        texture: look.frames.first().map(|&t| (t, SPRITE_SAMPLER)),
        blend: look.blend,
        cull: Cull::None,
        depth_test: true,
        depth_write: false,
        fog: false,
        lights: Vec::new(),
        vertex_key: None,
    }
}

/// The material word of `look`'s sprites, as the CPU path's draw gets it: an emitter of the sky's kind, unlit (fixed-function
/// lighting off), modulated by its texture when it has one, an emitter of light when added, lit where the rule says so.
pub fn sprite_material(look: &SpriteLook, lit: bool) -> u32 {
    let mut m = (material::KIND_SKY << material::KIND_SHIFT) | material::PARTICLE | material::UNLIT;
    if !look.frames.is_empty() {
        m |= material::tex_env(TexEnv::Modulate);
    }
    if look.blend == Blend::Additive {
        m |= material::EMITTER;
    }
    if lit {
        m |= material::LIT;
    }
    m
}

#[cfg(test)]
#[path = "sprites_tests.rs"]
mod tests;
