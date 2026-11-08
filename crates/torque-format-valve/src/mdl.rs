//! MDL studio models (versions 44 to 49): the header file of a model, holding its bones, its body parts and their meshes (the
//! vertices themselves are in the `.vvd`, the triangles in the `.vtx`), its texture names and the folders to find them in, its
//! skin families, its animations and its sequences.
//!
//! **Layout.** A 408-byte header (`IDST`, the version, a checksum the `.vvd` and `.vtx` repeat, the name, bounds, then counts
//! and offsets of every table); each table's entries hold offsets relative to the entry itself. Bones are 216 bytes (name,
//! parent, rest position and rotation as a quaternion and as Euler angles, the scales of their compressed animation values, the
//! pose-to-bone matrix). An animation (100 bytes) names its frame rate, frames and where its data is: in the model, in an `.ani`
//! file's block, or split in sections of frames. A sequence (212 bytes) names its animations in a grid of blend weights.
//!
//! **Animation data** is a list of per-bone records (bone, flags, offset to the next): a constant rotation (48- or 64-bit
//! quaternion) and position (three half floats), or per-axis run-length streams of 16-bit values scaled by the bone's scales
//! and added to its rest pose (Euler angles for rotations). [`Mdl::frame`] decodes one frame into every bone's pose.
//!
//! Every offset and count is checked: a hostile file is an error, never a panic.

use crate::read::{
    Error, Result, cap, count, cstr_at, f32_at, fixed_str, i16_at, i32_at, invalid, rel, slice,
    u8_at, u16_at, vec3_at,
};

/// The largest model file read.
pub const MAX_FILE_BYTES: u64 = 64 << 20;
/// The most bones read (the format's own limit is 256 in a mesh's bone indices).
pub const MAX_BONES: usize = 1024;
/// The most texture folders a model lists.
pub const MAX_TEXTURE_DIRS: usize = 64;
/// The most skin families a model lists.
pub const MAX_SKIN_FAMILIES: usize = 1024;
/// The most models a model includes.
pub const MAX_INCLUDES: usize = 32;
/// The longest string read from a model.
const MAX_STRING: usize = 512;
/// The most frames times bones one animation decodes at once.
pub const MAX_FRAME_SAMPLES: u64 = 1 << 22;
/// The most animation sections one model lists, all animations together.
pub const MAX_SECTIONS: usize = 1 << 16;

const HEADER_BYTES: usize = 408;
const BONE_BYTES: usize = 216;
const TEXTURE_BYTES: usize = 64;
const BODYPART_BYTES: usize = 16;
const MODEL_BYTES: usize = 148;
const MESH_BYTES: usize = 116;
const ANIMDESC_BYTES: usize = 100;
const SEQDESC_BYTES: usize = 212;
/// The bytes of one `.vvd` vertex (`vertexindex` of a model is in bytes of these).
pub const VERTEX_BYTES: usize = 48;

/// Animation flags.
pub mod anim_flags {
    pub const LOOPING: i32 = 0x0001;
    pub const DELTA: i32 = 0x0004;
    pub const ALL_ZEROS: i32 = 0x0020;
}

/// A bone at rest, with its animation scales.
#[derive(Debug, Clone, PartialEq)]
pub struct Bone {
    pub name: String,
    /// The parent bone, or None for a root.
    pub parent: Option<usize>,
    pub pos: [f32; 3],
    /// `[x, y, z, w]`.
    pub quat: [f32; 4],
    pub rot: [f32; 3],
    pub pos_scale: [f32; 3],
    pub rot_scale: [f32; 3],
    /// The model space to the bone's (3 x 4, row-major, translation in the last column).
    pub pose_to_bone: [f32; 12],
    pub flags: i32,
}

/// A mesh of a body part's model: its material slot and its vertices' place in the model's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mesh {
    /// The skin reference (a column of the skin table).
    pub material: i32,
    pub vertex_offset: i32,
    pub vertex_count: i32,
}

/// One choice of a body part (a body group's model).
#[derive(Debug, Clone, PartialEq)]
pub struct SubModel {
    pub name: String,
    /// Its first vertex in the `.vvd`'s list.
    pub vertex_start: usize,
    pub vertex_count: i32,
    pub meshes: Vec<Mesh>,
}

/// A body part: a set of models, one drawn at a time.
#[derive(Debug, Clone, PartialEq)]
pub struct BodyPart {
    pub name: String,
    pub models: Vec<SubModel>,
}

/// An animation: where its data is.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimDesc {
    pub name: String,
    pub fps: f32,
    pub flags: i32,
    pub frames: u32,
    /// Its data: `(block, offset)`: block 0 an offset into the model, another an offset into that `.ani` block.
    data: (i32, usize),
    /// Frames per section, and each section's `(block, offset)` (empty when not split).
    section_frames: u32,
    sections: Vec<(i32, usize)>,
}

/// A sequence: a named grid of animations blended by pose parameters (the first one is what plays with none set).
#[derive(Debug, Clone, PartialEq)]
pub struct Sequence {
    pub label: String,
    pub activity: String,
    pub flags: i32,
    /// Animation indices, `group[0] x group[1]`.
    pub anims: Vec<i16>,
    pub group: [usize; 2],
    pub fade_in: f32,
    pub fade_out: f32,
    /// What happens at moments of the sequence (sounds, effects).
    pub events: Vec<SeqEvent>,
}

/// An event of a sequence: when (0..1 of it), its number, its name (models made later name their events), and its options (a
/// sound event's soundscript entry).
#[derive(Debug, Clone, PartialEq)]
pub struct SeqEvent {
    pub cycle: f32,
    pub event: i32,
    pub name: String,
    pub options: String,
}

impl SeqEvent {
    /// Whether the event plays a sound (the client's and the server's sound events: numbers 5004 and 1004, or named
    /// `AE_CL_PLAYSOUND` / `AE_SV_PLAYSOUND`): its options name the sound.
    pub fn is_sound(&self) -> bool {
        matches!(self.event, 5004 | 1004)
            || ["AE_CL_PLAYSOUND", "AE_SV_PLAYSOUND"]
                .iter()
                .any(|n| self.name.eq_ignore_ascii_case(n))
    }
}

/// The bytes of one event.
const EVENT_BYTES: usize = 80;

/// A bone's pose: position and rotation `[x, y, z, w]` relative to its parent.
pub type Pose = ([f32; 3], [f32; 4]);

/// A model's header file (module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct Mdl {
    pub version: i32,
    pub checksum: i32,
    pub name: String,
    pub flags: i32,
    pub eye_position: [f32; 3],
    pub hull: [[f32; 3]; 2],
    pub bones: Vec<Bone>,
    pub textures: Vec<String>,
    pub texture_dirs: Vec<String>,
    /// Skin families: each a texture index per skin reference.
    pub skins: Vec<Vec<u16>>,
    pub body_parts: Vec<BodyPart>,
    pub anims: Vec<AnimDesc>,
    pub sequences: Vec<Sequence>,
    /// The models whose animations this one includes (`$includemodel`), as paths.
    pub include_models: Vec<String>,
    /// The `.ani` file of the animations stored in blocks, and each block's `(start, end)` in it.
    pub anim_block_file: String,
    pub anim_blocks: Vec<(usize, usize)>,
    pub surface_prop: String,
    bytes: Vec<u8>,
}

fn quat_from_euler(a: [f32; 3]) -> [f32; 4] {
    let (sr, cr) = (a[0] * 0.5).sin_cos();
    let (sp, cp) = (a[1] * 0.5).sin_cos();
    let (sy, cy) = (a[2] * 0.5).sin_cos();
    let (srcp, crsp) = (sr * cp, cr * sp);
    let (crcp, srsp) = (cr * cp, sr * sp);
    [
        srcp * cy - crsp * sy,
        crsp * cy + srcp * sy,
        crcp * sy - srsp * cy,
        crcp * cy + srsp * sy,
    ]
}

/// What one file may make: every entry of every table and every string read is paid from it, in the bytes it stands for. Each
/// table fits in the file on its own; the budget (twice the file's bytes) keeps tables whose offsets overlap from making much more
/// than the file holds.
struct Budget {
    left: usize,
}

impl Budget {
    fn of(b: &[u8]) -> Budget {
        Budget {
            left: b.len().saturating_mul(2).saturating_add(4096),
        }
    }

    fn take(&mut self, bytes: usize, what: &'static str) -> Result<()> {
        match self.left.checked_sub(bytes) {
            Some(rest) => {
                self.left = rest;
                Ok(())
            }
            None => Err(Error::TooLarge {
                what,
                asked: bytes as u64,
                cap: self.left as u64,
            }),
        }
    }
}

/// The NUL-terminated string at `off`, paid from `budget`.
fn string(b: &[u8], off: usize, budget: &mut Budget, what: &'static str) -> Result<String> {
    let s = cstr_at(b, off, MAX_STRING, what)?;
    budget.take(s.len() + 1, what)?;
    Ok(s)
}

/// A table's entries: `(count, offset)` read at `at` in the header, each `stride` bytes, checked to fit in the file and paid
/// from `budget`.
fn table(
    b: &[u8],
    at: usize,
    stride: usize,
    what: &'static str,
    budget: &mut Budget,
) -> Result<(usize, usize)> {
    let n = i32_at(b, at, what)?;
    let off = i32_at(b, at + 4, what)?;
    if n == 0 {
        return Ok((0, 0));
    }
    let off =
        usize::try_from(off).map_err(|_| Error::Invalid(format!("{what}: negative offset")))?;
    let room = b.len().saturating_sub(off);
    let n = count(n, stride, room, what)?;
    budget.take(n * stride, what)?;
    Ok((n, off))
}

impl Mdl {
    /// Reads a model's header file.
    pub fn parse(b: &[u8]) -> Result<Mdl> {
        cap("the MDL file's size", b.len() as u64, MAX_FILE_BYTES)?;
        if slice(b, 0, 4, "the MDL signature")? != b"IDST" {
            return invalid("not a studio model (bad signature)");
        }
        let version = i32_at(b, 4, "the MDL version")?;
        if !(44..=49).contains(&version) {
            return invalid(format!("studio model version {version} is not read"));
        }
        if b.len() < HEADER_BYTES {
            return Err(Error::Truncated {
                what: "the MDL header",
            });
        }
        let mut budget = Budget::of(b);
        let checksum = i32_at(b, 8, "the MDL header")?;
        let name = fixed_str(b, 12, 64, "the MDL name")?;
        let eye_position = vec3_at(b, 80, "the MDL header")?;
        let hull = [
            vec3_at(b, 104, "the MDL header")?,
            vec3_at(b, 116, "the MDL header")?,
        ];
        let flags = i32_at(b, 152, "the MDL header")?;

        let (nbones, boff) = table(b, 156, BONE_BYTES, "the MDL bones", &mut budget)?;
        cap("the MDL's bones", nbones as u64, MAX_BONES as u64)?;
        let mut bones = Vec::with_capacity(nbones);
        for i in 0..nbones {
            let at = boff + i * BONE_BYTES;
            let name = string(
                b,
                rel(at, i32_at(b, at, "a bone")?, "a bone name")?,
                &mut budget,
                "a bone name",
            )?;
            let parent = i32_at(b, at + 4, "a bone")?;
            let parent = usize::try_from(parent).ok().filter(|&p| p < i);
            let mut pose_to_bone = [0f32; 12];
            for (k, v) in pose_to_bone.iter_mut().enumerate() {
                *v = f32_at(b, at + 96 + k * 4, "a bone")?;
            }
            bones.push(Bone {
                name,
                parent,
                pos: vec3_at(b, at + 32, "a bone")?,
                quat: [
                    f32_at(b, at + 44, "a bone")?,
                    f32_at(b, at + 48, "a bone")?,
                    f32_at(b, at + 52, "a bone")?,
                    f32_at(b, at + 56, "a bone")?,
                ],
                rot: vec3_at(b, at + 60, "a bone")?,
                pos_scale: vec3_at(b, at + 72, "a bone")?,
                rot_scale: vec3_at(b, at + 84, "a bone")?,
                pose_to_bone,
                flags: i32_at(b, at + 160, "a bone")?,
            });
        }

        let (ntex, toff) = table(b, 204, TEXTURE_BYTES, "the MDL textures", &mut budget)?;
        let mut textures = Vec::with_capacity(ntex);
        for i in 0..ntex {
            let at = toff + i * TEXTURE_BYTES;
            let s = rel(at, i32_at(b, at, "a texture")?, "a texture name")?;
            textures.push(string(b, s, &mut budget, "a texture name")?);
        }
        let (ncd, cdoff) = table(b, 212, 4, "the MDL texture folders", &mut budget)?;
        cap(
            "the MDL's texture folders",
            ncd as u64,
            MAX_TEXTURE_DIRS as u64,
        )?;
        let mut texture_dirs = Vec::with_capacity(ncd);
        for i in 0..ncd {
            let s = i32_at(b, cdoff + i * 4, "a texture folder")?;
            let s = usize::try_from(s)
                .map_err(|_| Error::Invalid("a negative texture folder".into()))?;
            texture_dirs.push(string(b, s, &mut budget, "a texture folder")?);
        }

        let skin_refs = i32_at(b, 220, "the MDL skins")?;
        let families = i32_at(b, 224, "the MDL skins")?;
        let skin_off = i32_at(b, 228, "the MDL skins")?;
        let mut skins = Vec::new();
        if skin_refs > 0 && families > 0 {
            let refs = count(skin_refs, 2, b.len(), "the MDL skins")?;
            let fams = count(families, refs * 2, b.len(), "the MDL skins")?;
            cap(
                "the MDL's skin families",
                fams as u64,
                MAX_SKIN_FAMILIES as u64,
            )?;
            let off = usize::try_from(skin_off)
                .map_err(|_| Error::Invalid("a negative skin table".into()))?;
            slice(b, off, fams * refs * 2, "the MDL skins")?;
            budget.take(fams * refs * 2, "the MDL skins")?;
            for f in 0..fams {
                let mut row = Vec::with_capacity(refs);
                for r in 0..refs {
                    row.push(u16_at(b, off + (f * refs + r) * 2, "the MDL skins")?);
                }
                skins.push(row);
            }
        }

        let (nparts, poff) = table(b, 232, BODYPART_BYTES, "the MDL body parts", &mut budget)?;
        let mut body_parts = Vec::with_capacity(nparts);
        for i in 0..nparts {
            let at = poff + i * BODYPART_BYTES;
            let name = string(
                b,
                rel(at, i32_at(b, at, "a body part")?, "a body part")?,
                &mut budget,
                "a body part",
            )?;
            let nmodels = i32_at(b, at + 4, "a body part")?;
            let moff = rel(
                at,
                i32_at(b, at + 12, "a body part")?,
                "a body part's models",
            )?;
            let nmodels = count(
                nmodels,
                MODEL_BYTES,
                b.len().saturating_sub(moff),
                "a body part's models",
            )?;
            budget.take(nmodels * MODEL_BYTES, "a body part's models")?;
            let mut models = Vec::with_capacity(nmodels);
            for m in 0..nmodels {
                let at = moff + m * MODEL_BYTES;
                let name = fixed_str(b, at, 64, "a model")?;
                let nmeshes = i32_at(b, at + 72, "a model")?;
                let meshoff = rel(at, i32_at(b, at + 76, "a model")?, "a model's meshes")?;
                let vertex_count = i32_at(b, at + 80, "a model")?;
                let vertex_index = i32_at(b, at + 84, "a model")?;
                let vertex_start = usize::try_from(vertex_index).unwrap_or(0) / VERTEX_BYTES;
                let nmeshes = count(
                    nmeshes,
                    MESH_BYTES,
                    b.len().saturating_sub(meshoff),
                    "a model's meshes",
                )?;
                budget.take(nmeshes * MESH_BYTES, "a model's meshes")?;
                let mut meshes = Vec::with_capacity(nmeshes);
                for k in 0..nmeshes {
                    let at = meshoff + k * MESH_BYTES;
                    meshes.push(Mesh {
                        material: i32_at(b, at, "a mesh")?,
                        vertex_count: i32_at(b, at + 8, "a mesh")?,
                        vertex_offset: i32_at(b, at + 12, "a mesh")?,
                    });
                }
                models.push(SubModel {
                    name,
                    vertex_start,
                    vertex_count,
                    meshes,
                });
            }
            body_parts.push(BodyPart { name, models });
        }

        let (nanims, aoff) = table(b, 180, ANIMDESC_BYTES, "the MDL animations", &mut budget)?;
        let mut anims = Vec::with_capacity(nanims);
        let mut sections_total = 0usize;
        for i in 0..nanims {
            let at = aoff + i * ANIMDESC_BYTES;
            let name = string(
                b,
                rel(at, i32_at(b, at + 4, "an animation")?, "an animation")?,
                &mut budget,
                "an animation",
            )?;
            let fps = f32_at(b, at + 8, "an animation")?;
            let flags = i32_at(b, at + 12, "an animation")?;
            let frames = u32::try_from(i32_at(b, at + 16, "an animation")?).unwrap_or(0);
            let block = i32_at(b, at + 52, "an animation")?;
            let index = i32_at(b, at + 56, "an animation")?;
            let data = (
                block,
                if block == 0 {
                    rel(at, index, "an animation's data")?
                } else {
                    usize::try_from(index).unwrap_or(usize::MAX)
                },
            );
            let section_index = i32_at(b, at + 80, "an animation")?;
            let section_frames = u32::try_from(i32_at(b, at + 84, "an animation")?).unwrap_or(0);
            let mut sections = Vec::new();
            if section_frames > 0 && section_index != 0 {
                let soff = rel(at, section_index, "an animation's sections")?;
                // At most the entries that fit after the offset, and the file's total of sections.
                let n = ((frames / section_frames) as usize + 2)
                    .min(b.len().saturating_sub(soff) / 8)
                    .min(MAX_SECTIONS - sections_total);
                sections_total += n;
                budget.take(n * 8, "an animation's sections")?;
                for s in 0..n {
                    // The last entries may lie past the file's end in some files: the list stops there.
                    let block = i32_at(b, soff + s * 8, "a section")?;
                    let index = i32_at(b, soff + s * 8 + 4, "a section")?;
                    let off = if block == 0 {
                        rel(at, index, "a section")?
                    } else {
                        usize::try_from(index).unwrap_or(usize::MAX)
                    };
                    sections.push((block, off));
                }
            }
            anims.push(AnimDesc {
                name,
                fps,
                flags,
                frames,
                data,
                section_frames: if sections.is_empty() {
                    0
                } else {
                    section_frames
                },
                sections,
            });
        }

        let (nseq, soff) = table(b, 188, SEQDESC_BYTES, "the MDL sequences", &mut budget)?;
        let mut sequences = Vec::with_capacity(nseq);
        for i in 0..nseq {
            let at = soff + i * SEQDESC_BYTES;
            let label = string(
                b,
                rel(at, i32_at(b, at + 4, "a sequence")?, "a sequence")?,
                &mut budget,
                "a sequence",
            )?;
            let activity = string(
                b,
                rel(at, i32_at(b, at + 8, "a sequence")?, "a sequence")?,
                &mut budget,
                "a sequence",
            )?;
            let flags = i32_at(b, at + 12, "a sequence")?;
            let g0 = usize::try_from(i32_at(b, at + 68, "a sequence")?)
                .unwrap_or(0)
                .max(1);
            let g1 = usize::try_from(i32_at(b, at + 72, "a sequence")?)
                .unwrap_or(0)
                .max(1);
            let cells = g0
                .checked_mul(g1)
                .filter(|&c| c <= 1024)
                .ok_or(Error::Invalid(
                    "a sequence's blend grid is too large".into(),
                ))?;
            let ioff = rel(
                at,
                i32_at(b, at + 60, "a sequence")?,
                "a sequence's animations",
            )?;
            budget.take(cells * 2, "a sequence's animations")?;
            let mut list = Vec::with_capacity(cells);
            for k in 0..cells {
                list.push(i16_at(b, ioff + k * 2, "a sequence's animations")?);
            }
            // Its events.
            let nevents = i32_at(b, at + 24, "a sequence")?;
            let mut events = Vec::new();
            if nevents > 0 {
                let eoff = rel(at, i32_at(b, at + 28, "a sequence")?, "a sequence's events")?;
                let n = count(
                    nevents,
                    EVENT_BYTES,
                    b.len().saturating_sub(eoff),
                    "a sequence's events",
                )?;
                budget.take(n * EVENT_BYTES, "a sequence's events")?;
                for k in 0..n {
                    let e = eoff + k * EVENT_BYTES;
                    let name_at = i32_at(b, e + 76, "an event")?;
                    let name = if name_at == 0 {
                        String::new()
                    } else {
                        string(
                            b,
                            rel(e, name_at, "an event's name")?,
                            &mut budget,
                            "an event's name",
                        )?
                    };
                    events.push(SeqEvent {
                        cycle: f32_at(b, e, "an event")?,
                        event: i32_at(b, e + 4, "an event")?,
                        name,
                        options: fixed_str(b, e + 12, 64, "an event's options")?,
                    });
                }
            }
            sequences.push(Sequence {
                label,
                activity,
                flags,
                anims: list,
                group: [g0, g1],
                fade_in: f32_at(b, at + 104, "a sequence")?,
                fade_out: f32_at(b, at + 108, "a sequence")?,
                events,
            });
        }

        let surface_prop = string(
            b,
            usize::try_from(i32_at(b, 308, "the MDL header")?).unwrap_or(0),
            &mut budget,
            "the surface property",
        )
        .unwrap_or_default();
        let (ninc, ioff) = table(b, 336, 8, "the MDL include models", &mut budget)?;
        cap("the MDL's include models", ninc as u64, MAX_INCLUDES as u64)?;
        let mut include_models = Vec::with_capacity(ninc);
        for i in 0..ninc {
            let at = ioff + i * 8;
            let s = rel(
                at,
                i32_at(b, at + 4, "an include model")?,
                "an include model",
            )?;
            include_models.push(string(b, s, &mut budget, "an include model")?);
        }
        let anim_block_file = match i32_at(b, 348, "the MDL header")? {
            0 => String::new(),
            s => string(
                b,
                usize::try_from(s).unwrap_or(usize::MAX),
                &mut budget,
                "the animation block file",
            )?,
        };
        let (nblocks, blockoff) = table(b, 352, 8, "the MDL animation blocks", &mut budget)?;
        let mut anim_blocks = Vec::with_capacity(nblocks);
        for i in 0..nblocks {
            let s = i32_at(b, blockoff + i * 8, "an animation block")?;
            let e = i32_at(b, blockoff + i * 8 + 4, "an animation block")?;
            anim_blocks.push((
                usize::try_from(s).unwrap_or(0),
                usize::try_from(e).unwrap_or(0),
            ));
        }
        Ok(Mdl {
            version,
            checksum,
            name,
            flags,
            eye_position,
            hull,
            bones,
            textures,
            texture_dirs,
            skins,
            body_parts,
            anims,
            sequences,
            include_models,
            anim_block_file,
            anim_blocks,
            surface_prop,
            bytes: b.to_vec(),
        })
    }

    /// The paths a texture slot's material may be at (`materials/<folder><name>.vmt` for each folder), in the engine's order.
    pub fn material_paths(&self, texture: usize) -> Vec<String> {
        let Some(name) = self.textures.get(texture) else {
            return Vec::new();
        };
        let mut dirs = self.texture_dirs.clone();
        if dirs.is_empty() {
            dirs.push(String::new());
        }
        dirs.iter()
            .map(|d| {
                let d = d.replace('\\', "/");
                let d = d.trim_start_matches('/');
                let sep = if d.is_empty() || d.ends_with('/') {
                    ""
                } else {
                    "/"
                };
                crate::vmt::material_path(&format!("{d}{sep}{name}"))
            })
            .collect()
    }

    /// The index of the sequence labeled `label` (any case).
    pub fn sequence(&self, label: &str) -> Option<usize> {
        self.sequences
            .iter()
            .position(|s| s.label.eq_ignore_ascii_case(label))
    }

    /// Every bone's rest pose.
    pub fn rest_pose(&self) -> Vec<Pose> {
        self.bones.iter().map(|b| (b.pos, b.quat)).collect()
    }

    /// The poses of animation `anim`'s frames `0..count` (at most its frame count), every bone's in each; `ani` is the `.ani`
    /// file's bytes when the animation is stored in a block of it (without it such an animation is the rest pose). A delta
    /// animation's poses are moves from the identity (not added to the rest pose). Each run-length stream is decoded once in
    /// order, so the work is linear in the frames; frames times bones is capped ([`MAX_FRAME_SAMPLES`]).
    pub fn frames(&self, anim: usize, ani: Option<&[u8]>, count: u32) -> Result<Vec<Vec<Pose>>> {
        let a = self
            .anims
            .get(anim)
            .ok_or(Error::Invalid(format!("no animation {anim}")))?;
        let delta = a.flags & anim_flags::DELTA != 0;
        let base: Vec<Pose> = if delta {
            vec![([0.0; 3], [0.0, 0.0, 0.0, 1.0]); self.bones.len()]
        } else {
            self.rest_pose()
        };
        let n = count.min(a.frames).max(1) as usize;
        cap(
            "an animation's frames times bones",
            (n as u64) * (self.bones.len().max(1) as u64),
            MAX_FRAME_SAMPLES,
        )?;
        let mut out = vec![base; n];
        if a.flags & anim_flags::ALL_ZEROS != 0 || a.frames == 0 {
            return Ok(out);
        }
        // The runs of frames and where each one's records are: one run, or one a section.
        let mut segments = Vec::new();
        if a.section_frames > 0 {
            let sf = a.section_frames as usize;
            for (s, &(block, off)) in a.sections.iter().enumerate() {
                let first = s * sf;
                if first >= n {
                    break;
                }
                segments.push((first, (first + sf).min(n), block, off));
            }
            if segments.last().is_none_or(|l| l.1 < n) {
                return invalid("an animation's sections end before its frames");
            }
        } else {
            segments.push((0, n, a.data.0, a.data.1));
        }
        for (first, last, block, off) in segments {
            let (data, start): (&[u8], usize) = if block == 0 {
                (&self.bytes, off)
            } else {
                let Some(ani) = ani else {
                    continue;
                };
                let (s, _) = *usize::try_from(block)
                    .ok()
                    .and_then(|b| self.anim_blocks.get(b))
                    .ok_or(Error::Invalid(format!(
                        "animation block {block} is not listed"
                    )))?;
                (
                    ani,
                    s.checked_add(off).ok_or(Error::Truncated {
                        what: "an animation block",
                    })?,
                )
            };
            decode_frames(&self.bones, data, start, delta, &mut out[first..last])?;
        }
        Ok(out)
    }

    /// One frame's poses ([`Mdl::frames`]; the frames before it are decoded too).
    pub fn frame(&self, anim: usize, frame: u32, ani: Option<&[u8]>) -> Result<Vec<Pose>> {
        let mut all = self.frames(anim, ani, frame.saturating_add(1))?;
        Ok(all.pop().unwrap_or_default())
    }
}

/// A run-length stream's values for frames `0..count` (module docs), decoded in one pass: each run is stepped over once, so the
/// work is linear in `count`. A run of no frames ends the stream (the rest of its values are 0).
fn stream_values(b: &[u8], at: usize, count: usize) -> Result<Vec<f32>> {
    let mut out = Vec::with_capacity(count);
    let mut at = at;
    // The first frame of the run at `at`.
    let mut base = 0usize;
    for k in 0..count {
        loop {
            let total = usize::from(u8_at(b, at + 1, "an animation value")?);
            if total == 0 {
                out.resize(count, 0.0);
                return Ok(out);
            }
            if k - base < total {
                break;
            }
            let valid = usize::from(u8_at(b, at, "an animation value")?);
            base += total;
            at += (valid + 1) * 2;
        }
        let valid = usize::from(u8_at(b, at, "an animation value")?);
        let j = k - base;
        let slot = if valid > j { j + 1 } else { valid };
        out.push(f32::from(i16_at(b, at + slot * 2, "an animation value")?));
    }
    Ok(out)
}

/// One stream of a record's three (`ptr` the offsets' place; an offset of 0 or less: no stream, every value 0).
fn axis_values(
    b: &[u8],
    ptr: usize,
    axis: usize,
    count: usize,
    what: &'static str,
) -> Result<Vec<f32>> {
    let o = i16_at(b, ptr + axis * 2, what)?;
    if o <= 0 {
        return Ok(vec![0.0; count]);
    }
    stream_values(b, ptr + o as usize, count)
}

/// Decodes the records at `start` for `out.len()` frames (the first local frame 0), over the poses already in `out`.
fn decode_frames(
    bones: &[Bone],
    b: &[u8],
    start: usize,
    delta: bool,
    out: &mut [Vec<Pose>],
) -> Result<()> {
    let n = out.len();
    let mut at = start;
    for _ in 0..=bones.len() {
        let bone = usize::from(u8_at(b, at, "an animation record")?);
        let flags = u8_at(b, at + 1, "an animation record")?;
        let next = i16_at(b, at + 2, "an animation record")?;
        if bone == 255 {
            break;
        }
        let data = at + 4;
        if let Some(bn) = bones.get(bone) {
            let mut pos_at = data;
            if flags & 0x02 != 0 {
                pos_at = data + 6;
                let x = u16_at(b, data, "a rotation")?;
                let y = u16_at(b, data + 2, "a rotation")?;
                let zw = u16_at(b, data + 4, "a rotation")?;
                let qx = (f32::from(x) - 32768.0) / 32768.0;
                let qy = (f32::from(y) - 32768.0) / 32768.0;
                let qz = (f32::from(zw & 0x7fff) - 16384.0) / 16384.0;
                let w = (1.0 - qx * qx - qy * qy - qz * qz).max(0.0).sqrt();
                let q = [qx, qy, qz, if zw & 0x8000 != 0 { -w } else { w }];
                out.iter_mut().for_each(|f| f[bone].1 = q);
            } else if flags & 0x20 != 0 {
                pos_at = data + 8;
                let v = u64::from_le_bytes(crate::read::array::<8>(b, data, "a rotation")?);
                let c = |s: u32| ((v >> s) & 0x1f_ffff) as f32;
                let qx = (c(0) - 1_048_576.0) / 1_048_576.5;
                let qy = (c(21) - 1_048_576.0) / 1_048_576.5;
                let qz = (c(42) - 1_048_576.0) / 1_048_576.5;
                let w = (1.0 - qx * qx - qy * qy - qz * qz).max(0.0).sqrt();
                let q = [qx, qy, qz, if v >> 63 != 0 { -w } else { w }];
                out.iter_mut().for_each(|f| f[bone].1 = q);
            } else if flags & 0x08 != 0 {
                let axes = [
                    axis_values(b, data, 0, n, "a rotation stream")?,
                    axis_values(b, data, 1, n, "a rotation stream")?,
                    axis_values(b, data, 2, n, "a rotation stream")?,
                ];
                for (k, f) in out.iter_mut().enumerate() {
                    let e = [0, 1, 2].map(|i| {
                        axes[i][k] * bn.rot_scale[i] + if delta { 0.0 } else { bn.rot[i] }
                    });
                    f[bone].1 = quat_from_euler(e);
                }
            }
            if flags & 0x01 != 0 {
                let h = |k: usize| u16_at(b, pos_at + k * 2, "a position").map(crate::read::f16);
                let p = [h(0)?, h(1)?, h(2)?];
                out.iter_mut().for_each(|f| f[bone].0 = p);
            } else if flags & 0x04 != 0 {
                let pv = data + if flags & 0x08 != 0 { 6 } else { 0 };
                let axes = [
                    axis_values(b, pv, 0, n, "a position stream")?,
                    axis_values(b, pv, 1, n, "a position stream")?,
                    axis_values(b, pv, 2, n, "a position stream")?,
                ];
                for (k, f) in out.iter_mut().enumerate() {
                    f[bone].0 = [0, 1, 2].map(|i| {
                        axes[i][k] * bn.pos_scale[i] + if delta { 0.0 } else { bn.pos[i] }
                    });
                }
            }
        }
        if next == 0 {
            break;
        }
        at = rel(at, i32::from(next), "the next animation record")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn euler_quaternions() {
        let q = quat_from_euler([0.0, 0.0, std::f32::consts::FRAC_PI_2]);
        assert!((q[2] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert!((q[3] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
    }

    #[test]
    fn run_length_values() {
        // valid 2, total 4: values 10, 20, then 20 held; then valid 1 total 2: 30.
        let mut b = Vec::new();
        for v in [0x0402i16, 10, 20, 0x0201, 30] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        let vals = stream_values(&b, 0, 6).unwrap();
        assert_eq!(vals, vec![10.0, 20.0, 20.0, 20.0, 30.0, 30.0]);
        assert!(stream_values(&b, 0, 7).is_err());
    }

    #[test]
    fn refuses_bad_headers() {
        assert!(Mdl::parse(b"IDST").is_err());
        let mut b = vec![0u8; HEADER_BYTES];
        b[..4].copy_from_slice(b"IDST");
        b[4..8].copy_from_slice(&48i32.to_le_bytes());
        assert!(Mdl::parse(&b).is_ok());
        b[156..160].copy_from_slice(&1000i32.to_le_bytes());
        b[160..164].copy_from_slice(&10i32.to_le_bytes());
        assert!(Mdl::parse(&b).is_err());
    }
}
