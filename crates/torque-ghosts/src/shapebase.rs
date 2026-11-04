//! TGE's ShapeBase part ([`TgeShapeBase`]), the part every ShapeBase-derived class's record starts with: the GameBase part,
//! then one flag over the part's groups and, when it is set, damage, script threads, sound threads, mounted images, the
//! cloak/name/shield/invincible/skin group and the mount.
//!
//! ```text
//! GameBase part
//! flag(mask & (Name|Damage|Sound|Thread|Image|Cloak|Mounted|Invincible|Shield|Skin))      (0: the part ends here)
//!   flag(Damage): int(6) level (writeFloat of damage / maxDamage clamped to 0..1), int(2) state, normal vector (8)
//!   flag(Thread): per thread (4) flag(sequence != -1 && ThreadN): int(6) sequence, int(2) state, flag forward, flag atEnd
//!   flag(Sound):  per sound (4) flag(SoundN): flag(play): ranged(profile id) over the datablock range
//!   flag(Image):  per image (4) flag(ImageN): flag(datablock): int(DB) id - 3; net string (skin); flags wet, ammo, loaded,
//!                 target, triggerDown; int(3) fireCount; on an initial update flag(firing)
//!   flag(Name|Shield|Cloak|Invincible|Skin):
//!     flag(Cloak): flag cloaked, flag controlled (any controlling client), flag(fading && elapsed >= delay):
//!                  flag fadeOut, F32 fadeTime; else flag(fadeVal == 1)
//!     flag(Name): net string;  flag(Shield): normal vector (8), int(5) energy;  flag(Invincible): F32 time, F32 speed
//!     flag(Skin): net string
//!   Mounted and mounted on a ghosted object: flag 1, flag 1, int(GB) index, int(5) node
//!   Mounted on an object not ghosted yet: flag 0, and the packer keeps Mounted in its return mask
//!   Not mounted: an initial update flag 0; a later one flag 1, flag 0 (unmount).  Mounted bit clear: flag 0
//! ```
//! An initial update first clears the bits of the sounds that are not playing, the threads without a sequence and the images
//! without a datablock.
//!
//! **Mask bits** ([`mask`]): GameBase's 0..4, then Name 5, Damage 6, NoWarp 7, Mounted 8, Cloak 9, Shield 10, Invincible 11,
//! Skin 12, sounds 13..16, threads 17..20, images 21..24; the class that follows starts at 25.
//!
//! **View fields:** the GameBase part's (`scale` 0..3, `datablock`); `damage`, `max_damage`, `damage_state`, `damage_dir` 0..3;
//! per index 0..4 `thread_sequence` (-1 for none), `thread_state`, `thread_forward`, `thread_at_end`, `sound_play`,
//! `sound_profile`, `image_datablock`, the net string `image_skin`, `image_wet`, `image_ammo`, `image_loaded`, `image_target`,
//! `image_trigger_down`, `image_fire_count`, `image_firing`; `cloaked`, `controlled`, `fading`, `fade_elapsed_time`,
//! `fade_delay`, `fade_out`, `fade_time`, `fade_value`; the net string `shape_name`; `shield_normal` 0..3, `energy_value`
//! (energy over its maximum); `invincible_time`, `invincible_speed`; the net string `skin`; the link `mount` and `mount_node`.
//!
//! Credit: this follows TGE 1.4's `game/shapeBase.cc` (`ShapeBase::packUpdate` / `unpackUpdate`; Copyright (C)
//! GarageGames.com, Inc.), whose MIT-licensed successor is Torque3D's `T3D/shapeBase.cpp` (Copyright (c) 2012 GarageGames, LLC;
//! Copyright (C) 2015 Faust Logic, Inc.). Torque3D dropped the shield and invincible groups and moved the mount out of this
//! part; the layout here is TGE's. The code is this crate's own.

use crate::codec::{Codec, F32, NetStr, R, datablock_id, mask_bits, opt};
use crate::motion::{Normal, float_bits};
use crate::parts::{GameBase, ShapeBasePart, pack_gamebase};
use crate::{Link, ObjectView, PackCtx, boolean, fld, id, int, raw, raws};

/// ShapeBase's update-mask bits (GameBase's are in [`crate::parts::mask`]).
pub mod mask {
    pub const NAME: u32 = 1 << 5;
    pub const DAMAGE: u32 = 1 << 6;
    pub const NO_WARP: u32 = 1 << 7;
    pub const MOUNTED: u32 = 1 << 8;
    pub const CLOAK: u32 = 1 << 9;
    pub const SHIELD: u32 = 1 << 10;
    pub const INVINCIBLE: u32 = 1 << 11;
    pub const SKIN: u32 = 1 << 12;
    /// Sound thread `i` is `SOUND_N << i`.
    pub const SOUND_N: u32 = 1 << 13;
    /// Script thread `i` is `THREAD_N << i`.
    pub const THREAD_N: u32 = 1 << 17;
    /// Mounted image `i` is `IMAGE_N << i`.
    pub const IMAGE_N: u32 = 1 << 21;
    pub const SOUND: u32 = 0xf * SOUND_N;
    pub const THREAD: u32 = 0xf * THREAD_N;
    pub const IMAGE: u32 = 0xf * IMAGE_N;
    /// The first bit of the class that follows the part (`NextFreeMask`).
    pub const NEXT_FREE: u32 = 1 << 25;
    /// The bits whose groups the part's flag covers.
    pub const GROUPS: u32 =
        NAME | DAMAGE | SOUND | THREAD | IMAGE | CLOAK | MOUNTED | INVINCIBLE | SHIELD | SKIN;
}

/// Script threads, sound threads and mounted images (`MaxScriptThreads`, `MaxSoundThreads`, `MaxMountedImages`).
pub const MAX_SCRIPT_THREADS: usize = 4;
pub const MAX_SOUND_THREADS: usize = 4;
pub const MAX_MOUNTED_IMAGES: usize = 4;

/// The widths of the part's quantised fields.
pub const DAMAGE_LEVEL_BITS: u8 = 6;
pub const DAMAGE_STATE_BITS: u8 = 2;
pub const DAMAGE_NORMAL_BITS: u8 = 8;
pub const THREAD_SEQUENCE_BITS: u8 = 6;
pub const SHIELD_NORMAL_BITS: u8 = 8;
pub const ENERGY_LEVEL_BITS: u8 = 5;
pub const MOUNT_POINT_BITS: u8 = 5;
pub const FIRE_COUNT_BITS: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Damage {
    /// `writeFloat(damage / maxDamage, 6)`.
    pub level: u32,
    pub state: u32,
    pub direction: Normal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScriptThread {
    pub sequence: u32,
    pub state: u32,
    pub forward: bool,
    pub at_end: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Image {
    /// The image datablock's id, `None` for no image.
    pub datablock: Option<u32>,
    pub skin: NetStr,
    pub wet: bool,
    pub ammo: bool,
    pub loaded: bool,
    pub target: bool,
    pub trigger_down: bool,
    pub fire_count: u32,
    /// `isImageFiring`, sent on an initial update only.
    pub firing: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Fading {
    pub fade_out: bool,
    pub time: F32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cloak {
    pub cloaked: bool,
    /// The object has a controlling client (any connection's).
    pub controlled: bool,
    /// A fade under way (`mFading && mFadeElapsedTime >= mFadeDelay`).
    pub fading: Option<Fading>,
    /// With no fade: `mFadeVal == 1`. Not sent (false) during a fade.
    pub visible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Shield {
    pub normal: Normal,
    /// `writeFloat(energy value, 5)`.
    pub energy: u32,
}

/// The group of the less common blocks.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Rare {
    pub cloak: Option<Cloak>,
    pub name: Option<NetStr>,
    pub shield: Option<Shield>,
    /// The invincible effect's time and speed.
    pub invincible: Option<[F32; 2]>,
    pub skin: Option<NetStr>,
}

/// A mount on a ghosted object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mount {
    pub object: u32,
    pub node: u32,
}

/// The blocks under the part's flag.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShapeBody {
    pub damage: Option<Damage>,
    pub threads: Option<[Option<ScriptThread>; MAX_SCRIPT_THREADS]>,
    /// Per sound: `None` no update, `Some(None)` stopped, `Some(Some(id))` playing the profile.
    pub sounds: Option<[Option<Option<u32>>; MAX_SOUND_THREADS]>,
    pub images: Option<[Option<Image>; MAX_MOUNTED_IMAGES]>,
    pub rare: Option<Rare>,
    /// `None`: flag 0 (no change, or a mount still waiting for its object's ghost); `Some(None)`: unmount; `Some(Some(m))`: mount.
    pub mount: Option<Option<Mount>>,
}

/// TGE's ShapeBase part.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TgeShapeBase {
    pub game: GameBase,
    pub body: Option<ShapeBody>,
}

fn layout_image<C: Codec>(c: &mut C, img: &mut Image) -> R {
    opt(c, "sb.image.datablock?", &mut img.datablock, |c, d| {
        datablock_id(c, "sb.image.datablock", d)
    })?;
    c.net_string(&mut img.skin)?;
    c.flag("sb.image.wet", &mut img.wet)?;
    c.flag("sb.image.ammo", &mut img.ammo)?;
    c.flag("sb.image.loaded", &mut img.loaded)?;
    c.flag("sb.image.target", &mut img.target)?;
    c.flag("sb.image.triggerDown", &mut img.trigger_down)?;
    c.int("sb.image.fireCount", &mut img.fire_count, FIRE_COUNT_BITS)?;
    // The firing flag goes out with an initial update; the reader reads it while the object is not yet added.
    let initial = c.initial_update().unwrap_or(img.firing.is_some());
    if initial {
        let mut firing = img.firing.unwrap_or(false);
        c.flag("sb.image.firing", &mut firing)?;
        img.firing = Some(firing);
    } else {
        img.firing = None;
    }
    Ok(())
}

fn layout_rare<C: Codec>(c: &mut C, rare: &mut Rare) -> R {
    opt(c, "sb.cloak?", &mut rare.cloak, |c, k| {
        c.flag("sb.cloaked", &mut k.cloaked)?;
        c.flag("sb.controlled", &mut k.controlled)?;
        opt(c, "sb.fading?", &mut k.fading, |c, f| {
            c.flag("sb.fadeOut", &mut f.fade_out)?;
            c.raw32("sb.fadeTime", &mut f.time.0)
        })?;
        if k.fading.is_none() {
            c.flag("sb.visible", &mut k.visible)
        } else {
            k.visible = false;
            Ok(())
        }
    })?;
    opt(c, "sb.name?", &mut rare.name, |c, s| c.net_string(s))?;
    opt(c, "sb.shield?", &mut rare.shield, |c, s| {
        s.normal.layout(c, SHIELD_NORMAL_BITS)?;
        c.int("sb.shield.energy", &mut s.energy, ENERGY_LEVEL_BITS)
    })?;
    opt(c, "sb.invincible?", &mut rare.invincible, |c, v| {
        c.floats("sb.invincible", v)
    })?;
    opt(c, "sb.skin?", &mut rare.skin, |c, s| c.net_string(s))
}

impl TgeShapeBase {
    /// The part's layout, in wire order.
    pub fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.game.layout(c)?;
        opt(c, "sb.body?", &mut self.body, |c, b| {
            opt(c, "sb.damage?", &mut b.damage, |c, d| {
                c.int("sb.damage.level", &mut d.level, DAMAGE_LEVEL_BITS)?;
                c.int("sb.damage.state", &mut d.state, DAMAGE_STATE_BITS)?;
                d.direction.layout(c, DAMAGE_NORMAL_BITS)
            })?;
            opt(c, "sb.threads?", &mut b.threads, |c, ts| {
                for t in ts.iter_mut() {
                    opt(c, "sb.thread?", t, |c, t| {
                        c.int("sb.thread.sequence", &mut t.sequence, THREAD_SEQUENCE_BITS)?;
                        c.int("sb.thread.state", &mut t.state, 2)?;
                        c.flag("sb.thread.forward", &mut t.forward)?;
                        c.flag("sb.thread.atEnd", &mut t.at_end)
                    })?;
                }
                Ok(())
            })?;
            opt(c, "sb.sounds?", &mut b.sounds, |c, ss| {
                for s in ss.iter_mut() {
                    opt(c, "sb.sound?", s, |c, play| {
                        opt(c, "sb.sound.play", play, |c, d| {
                            datablock_id(c, "sb.sound.profile", d)
                        })
                    })?;
                }
                Ok(())
            })?;
            opt(c, "sb.images?", &mut b.images, |c, is| {
                for i in is.iter_mut() {
                    opt(c, "sb.image?", i, layout_image)?;
                }
                Ok(())
            })?;
            opt(c, "sb.rare?", &mut b.rare, layout_rare)?;
            let bits = c.ghost_bits();
            opt(c, "sb.mount?", &mut b.mount, |c, m| {
                opt(c, "sb.mounted", m, |c, m| {
                    c.int("sb.mount.object", &mut m.object, bits)?;
                    m.object = mask_bits(m.object, bits);
                    c.int("sb.mount.node", &mut m.node, MOUNT_POINT_BITS)
                })
            })
        })
    }
}

fn net_string(view: &dyn ObjectView, field: &str, i: usize) -> NetStr {
    view.net_string(field, i)
}

/// Packs the part: the record and the return mask (Mounted when the mount's object has no ghost index yet).
pub fn pack_shapebase(view: &dyn ObjectView, mask: u32, ctx: &PackCtx) -> (TgeShapeBase, u32) {
    let game = pack_gamebase(view, mask, ctx);
    let mut mask = mask;
    if mask & crate::parts::mask::INITIAL != 0 {
        for i in 0..MAX_SOUND_THREADS {
            if !boolean(view, "sound_play", i) {
                mask &= !(mask::SOUND_N << i);
            }
        }
        for i in 0..MAX_SCRIPT_THREADS {
            if int(view, "thread_sequence", i) == -1 {
                mask &= !(mask::THREAD_N << i);
            }
        }
        for i in 0..MAX_MOUNTED_IMAGES {
            if id(view, "image_datablock", i).is_none() {
                mask &= !(mask::IMAGE_N << i);
            }
        }
    }
    if mask & mask::GROUPS == 0 {
        return (TgeShapeBase { game, body: None }, 0);
    }
    let mut ret = 0;
    let initial = mask & crate::parts::mask::INITIAL != 0;
    let damage = (mask & mask::DAMAGE != 0).then(|| {
        // `mClampF` keeps a NaN, as `clamp` does.
        let level = (fld(view, "damage", 0) / fld(view, "max_damage", 0)).clamp(0.0, 1.0);
        Damage {
            level: float_bits(level, DAMAGE_LEVEL_BITS),
            state: mask_bits(int(view, "damage_state", 0) as u32, DAMAGE_STATE_BITS),
            direction: Normal::pack(
                raws::<3>(view, "damage_dir", 0).map(F32::get),
                DAMAGE_NORMAL_BITS,
            ),
        }
    });
    let threads = (mask & mask::THREAD != 0).then(|| {
        std::array::from_fn(|i| {
            let seq = int(view, "thread_sequence", i);
            (seq != -1 && mask & (mask::THREAD_N << i) != 0).then(|| ScriptThread {
                sequence: mask_bits(seq as u32, THREAD_SEQUENCE_BITS),
                state: mask_bits(int(view, "thread_state", i) as u32, 2),
                forward: boolean(view, "thread_forward", i),
                at_end: boolean(view, "thread_at_end", i),
            })
        })
    });
    let sounds = (mask & mask::SOUND != 0).then(|| {
        std::array::from_fn(|i| {
            (mask & (mask::SOUND_N << i) != 0).then(|| {
                boolean(view, "sound_play", i).then(|| ctx.wire_db(id(view, "sound_profile", i)))
            })
        })
    });
    let images = (mask & mask::IMAGE != 0).then(|| {
        std::array::from_fn(|i| {
            (mask & (mask::IMAGE_N << i) != 0).then(|| Image {
                datablock: id(view, "image_datablock", i).map(|d| ctx.wire_db(Some(d))),
                skin: net_string(view, "image_skin", i),
                wet: boolean(view, "image_wet", i),
                ammo: boolean(view, "image_ammo", i),
                loaded: boolean(view, "image_loaded", i),
                target: boolean(view, "image_target", i),
                trigger_down: boolean(view, "image_trigger_down", i),
                fire_count: mask_bits(int(view, "image_fire_count", i) as u32, FIRE_COUNT_BITS),
                firing: initial.then(|| boolean(view, "image_firing", i)),
            })
        })
    });
    let rare_bits = mask::NAME | mask::SHIELD | mask::CLOAK | mask::INVINCIBLE | mask::SKIN;
    let rare = (mask & rare_bits != 0).then(|| Rare {
        cloak: (mask & mask::CLOAK != 0).then(|| {
            let fading = boolean(view, "fading", 0)
                && fld(view, "fade_elapsed_time", 0) >= fld(view, "fade_delay", 0);
            Cloak {
                cloaked: boolean(view, "cloaked", 0),
                controlled: boolean(view, "controlled", 0),
                fading: fading.then(|| Fading {
                    fade_out: boolean(view, "fade_out", 0),
                    time: raw(view, "fade_time", 0),
                }),
                visible: !fading && fld(view, "fade_value", 0) == 1.0,
            }
        }),
        name: (mask & mask::NAME != 0).then(|| net_string(view, "shape_name", 0)),
        shield: (mask & mask::SHIELD != 0).then(|| Shield {
            normal: Normal::pack(
                raws::<3>(view, "shield_normal", 0).map(F32::get),
                SHIELD_NORMAL_BITS,
            ),
            energy: float_bits(fld(view, "energy_value", 0), ENERGY_LEVEL_BITS),
        }),
        invincible: (mask & mask::INVINCIBLE != 0).then(|| {
            [
                raw(view, "invincible_time", 0),
                raw(view, "invincible_speed", 0),
            ]
        }),
        skin: (mask & mask::SKIN != 0).then(|| net_string(view, "skin", 0)),
    });
    let mount = if mask & mask::MOUNTED != 0 {
        match view.link("mount") {
            Link::Ghost(index) => Some(Some(Mount {
                object: mask_bits(index, ctx.ghost_bits()),
                node: mask_bits(int(view, "mount_node", 0) as u32, MOUNT_POINT_BITS),
            })),
            Link::NotGhosted => {
                ret |= mask::MOUNTED;
                None
            }
            Link::Null => (!initial).then_some(None),
        }
    } else {
        None
    };
    let body = ShapeBody {
        damage,
        threads,
        sounds,
        images,
        rare,
        mount,
    };
    (
        TgeShapeBase {
            game,
            body: Some(body),
        },
        ret,
    )
}

impl ShapeBasePart for TgeShapeBase {
    const NEXT_FREE_MASK: u32 = mask::NEXT_FREE;

    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        TgeShapeBase::layout(self, c)
    }

    fn pack(view: &dyn ObjectView, mask: u32, ctx: &PackCtx) -> (TgeShapeBase, u32) {
        pack_shapebase(view, mask, ctx)
    }

    fn game(&self) -> &GameBase {
        &self.game
    }
}
