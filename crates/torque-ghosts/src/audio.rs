//! The `AudioEmitter` ghost (`AudioEmitter::packData` / `unpackData`): a ScopeAlways object that keeps its own dirty word `D`
//! (`mDirty`) and sends a group only when its bit is set.
//!
//! ```text
//! flag(mask & 1)                       (InitialUpdateMask)
//! flag(mask & 2)                       affine transform           (TransformUpdateMask; an initial update sets it itself)
//! flag(D & 1)   flag(profile != 0) [DB]            flag(D & 2)   flag(description != 0) [DB]
//! flag(D & 4)   string255 (file name)              flag(D & 8)   flag(use profile description)
//! flag(D & 0x10) F32 volume        flag(D & 0x20) flag(looping)    flag(D & 0x40) flag(3D)
//! flag(D & 0x80) F32 reference (min) distance      flag(D & 0x100) F32 max distance
//! flag(D & 0x200) raw 32 (cone inside)   flag(D & 0x400) raw 32 (cone outside)   flag(D & 0x800) F32 (cone outside volume)
//! flag(D & 0x1000) F32 x3 (cone vector)  flag(D & 0x2000) raw 32 (loop count)    flag(D & 0x4000) raw 32 (min gap)
//! flag(D & 0x8000) raw 32 (max gap)      flag(D & 0x20000) raw 32 (type)         flag(D & 0x40000) flag (outside ambient)
//! raw byte (enable visual feedback)
//! ```
//! (bit 16, `Transform`, is never written: the transform travels under the update mask). The packer derives `D` on an initial
//! update (`AllDirtyMask` less the groups the object leaves at their defaults), changes it as it goes (the `use profile
//! description` and looping/3D flags switch groups on and off, and an initial update drops the groups equal to the default
//! description's) and clears it at the end: the host zeroes its word after every pack and, when
//! [`AudioEffects::clear_use_profile_description`] is set, also clears the object's `useProfileDescription`, as `packData` does
//! when the flag is set without a profile.
//!
//! A worked example, a default emitter with only a profile (`outsideAmbient` is true by default): 1 + 1 + 193 for the two flags
//! and the transform, 1 + 1 + DB for the profile group, 3 flags for the description, file-name and `use profile description`
//! groups, 12 flags for the groups of volume to max gap, 2 for the type and outside ambient, and the final byte: 222 bits plus
//! the datablock id's width.
//!
//! **View fields:** `dirty` (the word D), `profile`, `description` (ids), `file_name` (string), `use_profile_description`, `volume`,
//! `looping`, `is_3d`, `min_distance`, `max_distance`, `cone_inside`, `cone_outside` (integers), `cone_outside_volume`,
//! `cone_vector` 0..3, `loop_count`, `min_gap`, `max_gap`, `kind`, `outside_ambient`, `enable_visual_feedback`, `transform`.
//!
//! Credit: this follows TGE 1.4's `game/audioEmitter.cc`, whose emitter was carried into the MIT-licensed Torque3D as
//! `T3D/sfx/sfxEmitter.cpp` (Copyright (c) 2012 GarageGames, LLC); the code here is this crate's own.

use crate::codec::{Codec, F32, Layout, R, Xf, datablock_id, opt};
use crate::{ObjectView, PackCtx, boolean, fld, id, raw, raws, text, word, xform};

/// What the host must do to the object after the pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AudioEffects {
    /// Clear the object's `useProfileDescription` (the flag was set but there is no profile).
    pub clear_use_profile_description: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AudioEmitter {
    pub initial: bool,
    pub transform: Option<Xf>,
    /// `flag(D & 1)`: `None` is no group, `Some(None)` a null profile, `Some(Some(id))` the datablock.
    pub profile: Option<Option<u32>>,
    pub description: Option<Option<u32>>,
    pub file_name: Option<Vec<u8>>,
    pub use_profile_description: Option<bool>,
    pub volume: Option<F32>,
    pub looping: Option<bool>,
    pub is_3d: Option<bool>,
    pub min_distance: Option<F32>,
    pub max_distance: Option<F32>,
    pub cone_inside: Option<u32>,
    pub cone_outside: Option<u32>,
    pub cone_outside_volume: Option<F32>,
    pub cone_vector: Option<[F32; 3]>,
    pub loop_count: Option<u32>,
    pub min_gap: Option<u32>,
    pub max_gap: Option<u32>,
    pub kind: Option<u32>,
    pub outside_ambient: Option<bool>,
    pub enable_visual_feedback: u8,
}

fn datablock_link<C: Codec>(c: &mut C, name: &'static str, l: &mut Option<Option<u32>>) -> R {
    opt(c, name, l, |c, inner| {
        let mut present = inner.is_some();
        c.flag("audio.has", &mut present)?;
        if present {
            let mut d = inner.unwrap_or(3);
            datablock_id(c, "audio.datablock", &mut d)?;
            *inner = Some(d);
        } else {
            *inner = None;
        }
        Ok(())
    })
}

impl Layout for AudioEmitter {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        c.flag("audio.initial", &mut self.initial)?;
        opt(c, "audio.transform?", &mut self.transform, |c, t| {
            c.xform("audio.transform", t)
        })?;
        datablock_link(c, "audio.profile?", &mut self.profile)?;
        datablock_link(c, "audio.description?", &mut self.description)?;
        opt(c, "audio.fileName?", &mut self.file_name, |c, s| {
            c.string("audio.fileName", s)
        })?;
        opt(
            c,
            "audio.useDescription?",
            &mut self.use_profile_description,
            |c, b| c.flag("audio.useDescription", b),
        )?;
        opt(c, "audio.volume?", &mut self.volume, |c, f| {
            c.raw32("audio.volume", &mut f.0)
        })?;
        opt(c, "audio.looping?", &mut self.looping, |c, b| {
            c.flag("audio.looping", b)
        })?;
        opt(c, "audio.is3d?", &mut self.is_3d, |c, b| {
            c.flag("audio.is3d", b)
        })?;
        opt(c, "audio.minDistance?", &mut self.min_distance, |c, f| {
            c.raw32("audio.minDistance", &mut f.0)
        })?;
        opt(c, "audio.maxDistance?", &mut self.max_distance, |c, f| {
            c.raw32("audio.maxDistance", &mut f.0)
        })?;
        opt(c, "audio.coneInside?", &mut self.cone_inside, |c, v| {
            c.raw32("audio.coneInside", v)
        })?;
        opt(c, "audio.coneOutside?", &mut self.cone_outside, |c, v| {
            c.raw32("audio.coneOutside", v)
        })?;
        opt(
            c,
            "audio.coneOutsideVolume?",
            &mut self.cone_outside_volume,
            |c, f| c.raw32("audio.coneOutsideVolume", &mut f.0),
        )?;
        opt(c, "audio.coneVector?", &mut self.cone_vector, |c, v| {
            c.floats("audio.coneVector", v)
        })?;
        opt(c, "audio.loopCount?", &mut self.loop_count, |c, v| {
            c.raw32("audio.loopCount", v)
        })?;
        opt(c, "audio.minGap?", &mut self.min_gap, |c, v| {
            c.raw32("audio.minGap", v)
        })?;
        opt(c, "audio.maxGap?", &mut self.max_gap, |c, v| {
            c.raw32("audio.maxGap", v)
        })?;
        opt(c, "audio.kind?", &mut self.kind, |c, v| {
            c.raw32("audio.kind", v)
        })?;
        opt(
            c,
            "audio.outsideAmbient?",
            &mut self.outside_ambient,
            |c, b| c.flag("audio.outsideAmbient", b),
        )?;
        c.raw8(
            "audio.enableVisualFeedback",
            &mut self.enable_visual_feedback,
        )
    }
}

/// Packs an emitter, running `packData`'s dirty-word logic in its order.
pub fn pack(view: &dyn ObjectView, mask: u32, ctx: &PackCtx) -> (AudioEmitter, AudioEffects) {
    let mut effects = AudioEffects::default();
    let mut mask = mask;
    let profile = id(view, "profile", 0);
    let description = id(view, "description", 0);
    let file_name = text(view, "file_name", 0);
    let mut use_desc = boolean(view, "use_profile_description", 0);
    let mut d = word(view, "dirty", 0);
    // The initial update: every group, less those the object leaves at their defaults.
    if mask & 1 != 0 {
        mask |= 2;
        d = 0x7_ffff;
        if profile.is_none() {
            d = 0x7_fffe;
        }
        if description.is_none() {
            d &= !2;
        }
        if file_name.is_empty() {
            d &= !4;
        }
        if !use_desc {
            d &= !8;
        }
        if boolean(view, "outside_ambient", 0) {
            d &= !0x4_0000;
        }
    }
    let mut rec = AudioEmitter {
        initial: mask & 1 != 0,
        transform: (mask & 2 != 0).then(|| xform(view, "transform")),
        ..AudioEmitter::default()
    };
    if d & 1 != 0 {
        rec.profile = Some(profile.map(|p| ctx.wire_db(Some(p))));
    }
    if d & 2 != 0 {
        rec.description = Some(description.map(|p| ctx.wire_db(Some(p))));
    }
    if d & 4 != 0 {
        rec.file_name = Some(file_name);
    }
    if d & 8 != 0 {
        let mut b = use_desc;
        if b {
            if profile.is_none() {
                use_desc = false;
                b = false;
                effects.clear_use_profile_description = true;
                d |= 0x2_fff0;
            } else {
                d &= 0xfffd_000f;
            }
        } else {
            d |= 0x2_fff0;
        }
        rec.use_profile_description = Some(b);
    }
    // With a description datablock (and not the profile's description), the description's own groups are not sent.
    if description.is_some() && !use_desc {
        d &= 0xfffd_000f;
    }
    // An initial update drops the groups equal to the default description's (looping and 3D are on by default).
    let default_looping = true;
    let default_3d = true;
    let looping = boolean(view, "looping", 0);
    let is_3d = boolean(view, "is_3d", 0);
    if mask & 1 != 0 && !use_desc {
        if fld(view, "volume", 0) == 1.0 {
            d &= !0x10;
        }
        if looping != default_looping {
            d &= !0xe000;
        } else {
            d &= !0x20;
            if word(view, "loop_count", 0) as i32 == -1 {
                d &= !0x2000;
            }
            if word(view, "min_gap", 0) == 0 {
                d &= !0x4000;
            }
            if word(view, "max_gap", 0) == 0 {
                d &= !0x8000;
            }
        }
        if is_3d != default_3d {
            d &= !0x1f80;
        } else {
            d &= !0x40;
            if fld(view, "min_distance", 0) == 1.0 {
                d &= !0x80;
            }
            if fld(view, "max_distance", 0) == 100.0 {
                d &= !0x100;
            }
            if word(view, "cone_inside", 0) as i32 == 360 {
                d &= !0x200;
            }
            if word(view, "cone_outside", 0) as i32 == 360 {
                d &= !0x400;
            }
            if fld(view, "cone_outside_volume", 0) == 1.0 {
                d &= !0x800;
            }
            if fld(view, "cone_vector", 0) == 0.0
                && fld(view, "cone_vector", 1) == 0.0
                && fld(view, "cone_vector", 2) == 1.0
            {
                d &= !0x1000;
            }
        }
        if word(view, "kind", 0) == 0 {
            d &= !0x2_0000;
        }
    }
    if d & 0x10 != 0 {
        rec.volume = Some(raw(view, "volume", 0));
    }
    if d & 0x20 != 0 {
        rec.looping = Some(looping);
        if looping {
            d |= 0xe000;
        } else {
            d &= 0xffff_1fff;
        }
    }
    if d & 0x40 != 0 {
        rec.is_3d = Some(is_3d);
        if is_3d {
            d |= 0x1f80;
        } else {
            d &= 0xffff_e07f;
        }
    }
    if d & 0x80 != 0 {
        rec.min_distance = Some(raw(view, "min_distance", 0));
    }
    if d & 0x100 != 0 {
        rec.max_distance = Some(raw(view, "max_distance", 0));
    }
    if d & 0x200 != 0 {
        rec.cone_inside = Some(word(view, "cone_inside", 0));
    }
    if d & 0x400 != 0 {
        rec.cone_outside = Some(word(view, "cone_outside", 0));
    }
    if d & 0x800 != 0 {
        rec.cone_outside_volume = Some(raw(view, "cone_outside_volume", 0));
    }
    if d & 0x1000 != 0 {
        rec.cone_vector = Some(raws::<3>(view, "cone_vector", 0));
    }
    if d & 0x2000 != 0 {
        rec.loop_count = Some(word(view, "loop_count", 0));
    }
    if d & 0x4000 != 0 {
        rec.min_gap = Some(word(view, "min_gap", 0));
    }
    if d & 0x8000 != 0 {
        rec.max_gap = Some(word(view, "max_gap", 0));
    }
    if d & 0x2_0000 != 0 {
        rec.kind = Some(word(view, "kind", 0));
    }
    if d & 0x4_0000 != 0 {
        rec.outside_ambient = Some(boolean(view, "outside_ambient", 0));
    }
    rec.enable_visual_feedback = u8::from(boolean(view, "enable_visual_feedback", 0));
    (rec, effects)
}
