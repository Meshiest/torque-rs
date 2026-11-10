//! Test support: a toy world implementing [`ObjectSource`] and a toy record codec for the reference client.
//!
//! A record body is eight flag-gated bytes (mask bits 0..7) and a flag-gated bulk blob (mask bit 8: `int(16)` length in bytes, then
//! the bytes). `pack_update` returns `unsent & mask` as the bits it could not send.

use std::collections::BTreeMap;

use crate::NetError;
use crate::bits::{BitReader, BitWriter};

use super::client::{ClassForm, GhostCodec};
use super::source::{CamInfo, ObjId, ObjectSource, net_flags};

#[derive(Debug, Clone)]
pub struct Obj {
    pub class: u32,
    /// Of the test policy's implied class.
    pub implied: bool,
    pub nflags: u32,
    pub fields: [u8; 8],
    pub bulk: usize,
    pub pos: [f32; 3],
    pub deps: Vec<ObjId>,
    pub mount: Option<ObjId>,
    pub unsent: u32,
    pub prio: f32,
    /// Its packet share group and part (`ObjectSource::packet_share`).
    pub share: Option<(u32, f32)>,
    /// What its first record counts for more ([`ObjectSource::first_record_bonus`]).
    pub first_bonus: f32,
}

impl Obj {
    pub fn new(class: u32) -> Obj {
        Obj {
            class,
            implied: false,
            nflags: net_flags::GHOSTABLE,
            fields: [0; 8],
            bulk: 0,
            pos: [0.0; 3],
            deps: Vec::new(),
            mount: None,
            unsent: 0,
            prio: 0.0,
            share: None,
            first_bonus: 0.0,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct World {
    pub objs: BTreeMap<ObjId, Obj>,
    pub always: Vec<ObjId>,
    pub recent: Vec<ObjId>,
    /// The container order the scope walk visits in.
    pub container: Vec<ObjId>,
    pub sky: Option<f32>,
    pub scope_pos: [f32; 3],
    /// How many `pack_update` calls ran (the work a section did).
    pub packs: usize,
    /// The feed of new objects (`ObjectSource::fresh_objects`): the objects, the first numbered `fresh_base`.
    pub fresh: Vec<ObjId>,
    pub fresh_base: u64,
    /// The objects the full scope query would take whatever their centre's distance (`ObjectSource::reaches_scope`).
    pub reach: Vec<ObjId>,
    /// The stamp of the scope query ([`ObjectSource::scope_stamp`]); a test that sets it bumps it whenever the objects the query
    /// takes change.
    pub stamp: Option<u64>,
    /// How many times a scope walk asked whether an object exists.
    pub existence_asked: std::cell::Cell<usize>,
    /// The objects the host keeps out of scope ([`ObjectSource::scope_hidden`]) and the stamp that moves when the set does.
    pub hidden: std::collections::BTreeSet<ObjId>,
    pub hidden_stamp: u64,
    /// The scope object has no camera ([`ObjectSource::camera_info`] answers none).
    pub no_camera: bool,
    /// No object has a distance from the scope object ([`ObjectSource::scope_distance` answers none).
    pub no_distance: bool,
}

impl World {
    pub fn add(&mut self, id: ObjId, obj: Obj) {
        self.stamp = self.stamp.map(|s| s + 1);
        self.objs.insert(id, obj);
        if self.objs[&id].nflags & net_flags::GHOSTABLE != 0 {
            self.container.push(id);
        }
    }

    /// Keeps `id` out of scope (or lets it back in).
    pub fn set_hidden(&mut self, id: ObjId, hidden: bool) {
        let changed = if hidden {
            self.hidden.insert(id)
        } else {
            self.hidden.remove(&id)
        };
        if changed {
            self.hidden_stamp += 1;
        }
    }

    pub fn remove(&mut self, id: ObjId) {
        self.stamp = self.stamp.map(|s| s + 1);
        self.objs.remove(&id);
        self.container.retain(|&c| c != id);
        self.recent.retain(|&c| c != id);
        self.always.retain(|&c| c != id);
    }
}

impl ObjectSource for World {
    fn exists(&self, obj: ObjId) -> bool {
        self.existence_asked.set(self.existence_asked.get() + 1);
        self.objs.contains_key(&obj)
    }

    fn class_id(&self, obj: ObjId) -> Option<u32> {
        self.objs.get(&obj).map(|o| o.class)
    }

    fn net_flags(&self, obj: ObjId) -> u32 {
        self.objs.get(&obj).map_or(0, |o| o.nflags)
    }

    fn scope_stamp(&self) -> Option<u64> {
        self.stamp
    }

    fn scope_hidden(&self, obj: ObjId) -> bool {
        self.hidden.contains(&obj)
    }

    fn scope_hidden_stamp(&self) -> u64 {
        self.hidden_stamp
    }

    fn pack_update(
        &mut self,
        obj: ObjId,
        mask: u32,
        w: &mut BitWriter,
        _ghost_of: &dyn Fn(ObjId) -> Option<u32>,
    ) -> u32 {
        self.packs += 1;
        let Some(o) = self.objs.get(&obj) else {
            return 0;
        };
        for b in 0..8 {
            if mask & (1 << b) != 0 {
                w.write_flag(true);
                w.write_bits(u32::from(o.fields[b]), 8);
            } else {
                w.write_flag(false);
            }
        }
        if mask & 0x100 != 0 {
            w.write_flag(true);
            w.write_bits(o.bulk as u32, 16);
            for k in 0..o.bulk {
                w.write_bits((k & 0xff) as u32, 8);
            }
        } else {
            w.write_flag(false);
        }
        o.unsent & mask
    }

    fn update_priority(&self, obj: ObjId, _cam: &CamInfo, _mask: u32, _skip: u8) -> f32 {
        self.objs.get(&obj).map_or(0.0, |o| o.prio)
    }

    fn packet_share(&self, obj: ObjId) -> Option<(u32, f32)> {
        self.objs.get(&obj)?.share
    }

    fn first_record_bonus(&self, obj: ObjId) -> f32 {
        self.objs.get(&obj).map_or(0.0, |o| o.first_bonus)
    }

    fn camera_info(&self, _scope: ObjId) -> Option<CamInfo> {
        if self.no_camera {
            return None;
        }
        Some(CamInfo {
            eye: self.scope_pos,
            ..CamInfo::default()
        })
    }

    fn sky_visible_distance(&self) -> Option<f32> {
        self.sky
    }

    fn mount_of(&self, scope: ObjId) -> Option<ObjId> {
        self.objs.get(&scope).and_then(|o| o.mount)
    }

    fn scope_query(
        &self,
        _scope: ObjId,
        cam: &CamInfo,
        radius: f32,
        visit: &mut dyn FnMut(ObjId) -> bool,
    ) {
        for &id in &self.container {
            let Some(o) = self.objs.get(&id) else {
                continue;
            };
            if o.nflags & 0x140 != 0x100 {
                continue;
            }
            let near = (0..3).all(|k| (o.pos[k] - cam.eye[k]).abs() <= radius);
            if near && !visit(id) {
                return;
            }
        }
    }

    fn fresh_objects(&self, from: u64) -> (u64, &[ObjId]) {
        let skip = (from.saturating_sub(self.fresh_base) as usize).min(self.fresh.len());
        (self.fresh_base + skip as u64, &self.fresh[skip..])
    }

    fn fresh_end(&self) -> u64 {
        self.fresh_base + self.fresh.len() as u64
    }

    fn reaches_scope(&self, obj: ObjId, _eye: [f32; 3], _radius: f32) -> bool {
        self.reach.contains(&obj)
    }

    fn scope_distance(&self, obj: ObjId, _scope: ObjId) -> Option<f32> {
        if self.no_distance {
            return None;
        }
        let o = self.objs.get(&obj)?;
        Some(
            (0..3)
                .map(|k| (o.pos[k] - self.scope_pos[k]).powi(2))
                .sum::<f32>()
                .sqrt(),
        )
    }

    fn dependents(&self, obj: ObjId, out: &mut Vec<ObjId>) {
        if let Some(o) = self.objs.get(&obj) {
            out.extend(o.deps.iter().copied());
        }
    }

    fn ghost_always(&self) -> &[ObjId] {
        &self.always
    }
}

/// What the toy client keeps of a ghost.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Toy {
    pub class: Option<ClassForm>,
    pub fields: [Option<u8>; 8],
    pub bulk: Option<usize>,
}

#[derive(Debug, Default)]
pub struct ToyCodec;

fn read_record(t: &mut Toy, r: &mut BitReader) -> Result<(), NetError> {
    for b in 0..8 {
        if r.read_flag()? {
            t.fields[b] = Some(r.read_bits(8)? as u8);
        }
    }
    if r.read_flag()? {
        let n = r.read_bits(16)? as usize;
        if n.saturating_mul(8) > r.bits_left() {
            return Err(NetError::Overrun);
        }
        for k in 0..n {
            if r.read_bits(8)? != (k & 0xff) as u32 {
                return Err(NetError::Invalid("bulk content"));
            }
        }
        t.bulk = Some(n);
    }
    Ok(())
}

impl GhostCodec for ToyCodec {
    type Object = Toy;

    fn create(&mut self, class: ClassForm, r: &mut BitReader) -> Result<Toy, NetError> {
        let mut t = Toy {
            class: Some(class),
            ..Toy::default()
        };
        read_record(&mut t, r)?;
        Ok(t)
    }

    fn placeholder(&mut self) -> Toy {
        Toy::default()
    }

    fn update(&mut self, object: &mut Toy, r: &mut BitReader) -> Result<(), NetError> {
        read_record(object, r)
    }
}
