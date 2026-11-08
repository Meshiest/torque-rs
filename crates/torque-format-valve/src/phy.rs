//! Physics models (`.phy`): the collision pieces of a model, one convex hull per piece, and the settings of its solids.
//!
//! **Provenance.** The frame of the file (header, a size before each solid, key-values text at the end) is described in public
//! documentation of the format. The binary data inside a solid is the layout of the physics library's "compact surface"; its
//! byte layout here follows the structure definitions of the MIT-licensed readers of it, VPhysics-Jolt (`vjolt_collide.cpp`:
//! `compactsurfaceheader_t`, `compactsurface_t`, `compactledgenode_t`, `compactledge_t`, `compacttriangle_t`) and
//! HL2UE4AssetImporter (`physdata.h`), with credit to their authors, and was written again here and checked against synthetic
//! files and the oracles in the tests. No other reader's code was looked at and no game executable or library was opened.
//!
//! **Layout.**
//!
//! ```text
//! header    16 bytes: its own size (16), an id, the solid count, the checksum of the model it belongs to
//! solids    each an `i32` size and that many bytes
//! text      the rest of the file: key-values
//! ```
//!
//! A solid (modern) is `VPHY`, a version, a model type (0 convex pieces, 1 MOPP tree, 2 ball, 3 virtual), a 20-byte surface header
//! (size, three drag areas, the axis map's size) and a 48-byte compact surface (mass center, inertia, radius, size, the offset of
//! the ledge tree's root, a tag); an older solid is the 48-byte compact surface alone. Offsets inside it are signed, relative to
//! the structure that holds them. The ledge tree is 28-byte nodes: a node with no right offset is a leaf naming a 16-byte ledge
//! (a count of triangles, the offset of its points); any other node's left child is the next node in memory and its right child is
//! at the offset. One ledge is one convex piece. Its triangles (16 bytes each) name points by index (16 bytes each: x, y, z and
//! padding); the corners of a piece are the points its triangles use.
//!
//! **Space.** Points are meters in the physics library's axes. A point `(x, y, z)` is `(x, z, -y) / 0.0254` in the model's inches
//! and axes (the space of the model's own bounds), and that is what [`Ledge::points`], [`Hull`] and the solid's mass center are in.
//!
//! **Hostile input.** The file is capped ([`MAX_FILE_BYTES`]); every count and offset is checked against the bytes of the solid it
//! is in; the tree is walked with an explicit stack and a visited set, to [`MAX_TREE_DEPTH`], with nodes, ledges, triangles and
//! hull work each paid from a budget for the whole file (overlapping offsets cannot make it read more than the budget); the text is
//! capped ([`MAX_TEXT_BYTES`]). A solid that does not read is left out with a note, and the file with it still reads.

use std::collections::HashSet;

use torque_engine::assets::{LoadCx, Spec};
use torque_engine::{EngineBuilder, Plugin, PluginId};

use crate::hull::{Hull, bounds, convex_hull};
use crate::kv;
use crate::read::{
    Error, Result, cap, f32_at, i32_at, invalid, rel, slice, u16_at, u32_at, vec3_at,
};

/// The largest `.phy` file read.
pub const MAX_FILE_BYTES: u64 = 16 << 20;
/// The most solids one file holds.
pub const MAX_SOLIDS: usize = 1024;
/// The most ledges (convex pieces) one file holds, all solids together.
pub const MAX_LEDGES: usize = 1 << 14;
/// The most triangles one ledge names.
pub const MAX_LEDGE_TRIANGLES: usize = 1 << 12;
/// The most triangles a file's ledges name together (the points read are three times this).
pub const MAX_TRIANGLES: usize = 1 << 18;
/// The most ledge tree nodes a file's solids visit together.
pub const MAX_TREE_NODES: usize = 1 << 17;
/// The deepest ledge tree walked.
pub const MAX_TREE_DEPTH: usize = 256;
/// The most bytes of key-values text read.
pub const MAX_TEXT_BYTES: usize = 1 << 20;
/// Hull work (face examinations) one file may spend.
pub const MAX_HULL_WORK: u64 = 1 << 26;
/// The most solid, constraint and rule blocks one text keeps of each kind.
pub const MAX_BLOCKS: usize = 4096;

/// The largest coordinate (inches) a point may have; a ledge with a point past it is left out.
pub const MAX_COORDINATE: f32 = 1e8;

/// Meters per inch.
const METERS_PER_INCH: f32 = 0.0254;
const HEADER_BYTES: usize = 16;
const SURFACE_HEADER_BYTES: usize = 20;
const COMPACT_SURFACE_BYTES: usize = 48;
const NODE_BYTES: usize = 28;
const LEDGE_BYTES: usize = 16;
const TRIANGLE_BYTES: usize = 16;
const POINT_BYTES: usize = 16;

/// A point of the physics library's space in the model's inches and axes (module docs).
pub fn to_model(p: [f32; 3]) -> [f32; 3] {
    [
        p[0] / METERS_PER_INCH,
        p[2] / METERS_PER_INCH,
        -p[1] / METERS_PER_INCH,
    ]
}

/// What kind of collision data a solid holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SolidKind {
    /// Convex pieces (read).
    Poly,
    /// A bounding-volume tree over a triangle soup (not read).
    Mopp,
    /// A ball (not read).
    Ball,
    /// Built by the engine from the model (not read).
    Virtual,
    /// Anything else (not read).
    Other(u16),
}

/// One convex piece as the file holds it: its distinct corner points, in the model's inches.
#[derive(Debug, Clone, PartialEq)]
pub struct Ledge {
    pub points: Vec<[f32; 3]>,
    /// The hull of `points` (a box of them when they have no volume to hull and a box has some, or when the file's hull work
    /// ran out).
    pub hull: Hull,
    /// Whether `hull` is the box of the points, not their hull.
    pub boxed: bool,
}

/// One solid: a model's body or one bone's body of a ragdoll.
#[derive(Debug, Clone, PartialEq)]
pub struct Solid {
    pub kind: SolidKind,
    /// Whether it is in the older layout (no `VPHY` header).
    pub legacy: bool,
    pub mass_center: [f32; 3],
    pub rotation_inertia: [f32; 3],
    pub upper_limit_radius: f32,
    /// The areas a drag is figured from along each axis (modern layout; zeros in the older).
    pub drag_axis_areas: [f32; 3],
    /// Its convex pieces (none when it is not [`SolidKind::Poly`] or did not read).
    pub ledges: Vec<Ledge>,
}

impl Solid {
    /// The hulls of its pieces.
    pub fn hulls(&self) -> impl Iterator<Item = &Hull> {
        self.ledges.iter().map(|l| &l.hull)
    }
}

/// What a `solid` block of the text says about one solid.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SolidProps {
    pub index: usize,
    /// The bone it moves with (models).
    pub name: String,
    /// The parent bone's name.
    pub parent: String,
    pub surface_prop: String,
    pub mass: f32,
    pub mass_center_override: Option<[f32; 3]>,
    pub damping: f32,
    pub rot_damping: f32,
    pub drag: f32,
    pub inertia: f32,
    pub rot_inertia_limit: f32,
    pub volume: f32,
}

/// A `ragdollconstraint` block: the joint between two solids, its limits in degrees on each axis of the joint's own frame
/// (read, not used).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Constraint {
    pub parent: usize,
    pub child: usize,
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub friction: [f32; 3],
}

/// The key-values text of a file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PhyText {
    pub solids: Vec<SolidProps>,
    pub constraints: Vec<Constraint>,
    /// The pairs of the `collisionrules` block, as written.
    pub collision_rules: Vec<(String, String)>,
    /// The pairs of the `editparams` block, as written.
    pub edit_params: Vec<(String, String)>,
}

/// A physics file.
#[derive(Debug, Clone, PartialEq)]
pub struct Phy {
    /// The checksum of the model it was made with.
    pub checksum: i32,
    /// The solids, in the file's order (one per solid the header counts, whether or not it read).
    pub solids: Vec<Solid>,
    pub text: PhyText,
    /// What did not read, and why.
    pub notes: Vec<String>,
}

impl Phy {
    /// Every convex piece of every solid.
    pub fn hulls(&self) -> impl Iterator<Item = &Hull> {
        self.solids.iter().flat_map(Solid::hulls)
    }

    /// The settings the text gives solid `index`.
    pub fn props(&self, index: usize) -> Option<&SolidProps> {
        self.text.solids.iter().find(|s| s.index == index)
    }

    /// Reads a physics file.
    pub fn parse(b: &[u8]) -> Result<Phy> {
        cap("the PHY file's size", b.len() as u64, MAX_FILE_BYTES)?;
        let size = i32_at(b, 0, "the PHY header")?;
        let count = i32_at(b, 8, "the PHY header")?;
        let checksum = i32_at(b, 12, "the PHY header")?;
        let first = usize::try_from(size)
            .ok()
            .filter(|&s| s >= HEADER_BYTES && s <= b.len())
            .ok_or_else(|| Error::Invalid(format!("a PHY header of {size} bytes")))?;
        let count = usize::try_from(count)
            .map_err(|_| Error::Invalid(format!("a negative solid count {count}")))?;
        cap("the PHY solids", count as u64, MAX_SOLIDS as u64)?;
        let mut budget = Budget::default();
        let mut notes = Vec::new();
        let mut solids = Vec::with_capacity(count);
        let mut at = first;
        for i in 0..count {
            let len = i32_at(b, at, "a solid's size")?;
            let len = usize::try_from(len)
                .map_err(|_| Error::Invalid(format!("solid {i} has a negative size")))?;
            let data = slice(b, at + 4, len, "a solid")?;
            at += 4 + len;
            match read_solid(data, &mut budget) {
                Ok(s) => solids.push(s),
                Err(e @ Error::TooLarge { .. }) => return Err(e),
                Err(e) => {
                    notes.push(format!("solid {i}: {e}"));
                    solids.push(Solid::empty(SolidKind::Other(u16::MAX)));
                }
            }
        }
        for (i, s) in solids.iter().enumerate() {
            match s.kind {
                SolidKind::Poly => {}
                other => notes.push(format!("solid {i}: {other:?} data is not read")),
            }
        }
        let text_bytes = &b[at.min(b.len())..];
        let text_bytes = &text_bytes[..text_bytes.len().min(MAX_TEXT_BYTES)];
        let text = match read_text(text_bytes, solids.len()) {
            Ok(t) => t,
            Err(e) => {
                notes.push(format!("the key-values text: {e}"));
                PhyText::default()
            }
        };
        Ok(Phy {
            checksum,
            solids,
            text,
            notes,
        })
    }
}

impl Solid {
    fn empty(kind: SolidKind) -> Solid {
        Solid {
            kind,
            legacy: false,
            mass_center: [0.0; 3],
            rotation_inertia: [0.0; 3],
            upper_limit_radius: 0.0,
            drag_axis_areas: [0.0; 3],
            ledges: Vec::new(),
        }
    }
}

/// What a whole file may spend.
struct Budget {
    nodes: usize,
    ledges: usize,
    triangles: usize,
    hull_work: u64,
}

impl Default for Budget {
    fn default() -> Budget {
        Budget {
            nodes: MAX_TREE_NODES,
            ledges: MAX_LEDGES,
            triangles: MAX_TRIANGLES,
            hull_work: MAX_HULL_WORK,
        }
    }
}

fn spend(left: &mut usize, n: usize, what: &'static str, cap: usize) -> Result<()> {
    match left.checked_sub(n) {
        Some(rest) => {
            *left = rest;
            Ok(())
        }
        None => Err(Error::TooLarge {
            what,
            asked: (cap - *left + n) as u64,
            cap: cap as u64,
        }),
    }
}

/// A solid's bytes (after its size) as a solid.
fn read_solid(data: &[u8], budget: &mut Budget) -> Result<Solid> {
    let (kind, legacy, drag, surf) = if slice(data, 0, 4, "a solid")? == b"VPHY" {
        let model_type = u16_at(data, 6, "a solid's header")?;
        let kind = match model_type {
            0 => SolidKind::Poly,
            1 => SolidKind::Mopp,
            2 => SolidKind::Ball,
            3 => SolidKind::Virtual,
            n => SolidKind::Other(n),
        };
        let drag = vec3_at(data, 12, "a solid's header")?;
        let start = 8 + SURFACE_HEADER_BYTES;
        (kind, false, drag, data.get(start..).unwrap_or(&[]))
    } else {
        let tag = slice(data, 44, 4, "a solid's tag")?;
        let kind = if tag == b"MOPP" {
            SolidKind::Mopp
        } else {
            SolidKind::Poly
        };
        (kind, true, [0.0; 3], data)
    };
    if kind != SolidKind::Poly {
        return Ok(Solid::empty(kind));
    }
    if surf.len() < COMPACT_SURFACE_BYTES {
        return Err(Error::Truncated {
            what: "a compact surface",
        });
    }
    let mass_center = to_model(vec3_at(surf, 0, "a compact surface")?);
    let rotation_inertia = vec3_at(surf, 12, "a compact surface")?;
    let upper_limit_radius = f32_at(surf, 24, "a compact surface")? / METERS_PER_INCH;
    let root = i32_at(surf, 32, "a compact surface")?;
    let root = usize::try_from(root)
        .ok()
        .filter(|&r| r >= COMPACT_SURFACE_BYTES)
        .ok_or_else(|| Error::Invalid(format!("a ledge tree root at {root}")))?;
    let ledges = walk_tree(surf, root, budget)?;
    Ok(Solid {
        kind,
        legacy,
        mass_center,
        rotation_inertia,
        upper_limit_radius,
        drag_axis_areas: drag,
        ledges,
    })
}

/// The pieces of the ledge tree rooted at `root` in `surf`.
fn walk_tree(surf: &[u8], root: usize, budget: &mut Budget) -> Result<Vec<Ledge>> {
    let mut nodes: HashSet<usize> = HashSet::new();
    let mut seen_ledges: HashSet<usize> = HashSet::new();
    let mut ledges = Vec::new();
    let mut stack = vec![(root, 0usize)];
    while let Some((node, depth)) = stack.pop() {
        if depth > MAX_TREE_DEPTH {
            return invalid("a ledge tree is too deep");
        }
        if !nodes.insert(node) {
            continue;
        }
        spend(&mut budget.nodes, 1, "the ledge tree nodes", MAX_TREE_NODES)?;
        let body = slice(surf, node, NODE_BYTES, "a ledge tree node")?;
        let right = i32_at(body, 0, "a ledge tree node")?;
        if right == 0 {
            let at = rel(node, i32_at(body, 4, "a ledge tree node")?, "a ledge")?;
            if !seen_ledges.insert(at) {
                continue;
            }
            spend(&mut budget.ledges, 1, "the ledges", MAX_LEDGES)?;
            ledges.extend(read_ledge(surf, at, budget)?);
        } else {
            let right = rel(node, right, "a ledge tree node's right child")?;
            stack.push((right, depth + 1));
            stack.push((node + NODE_BYTES, depth + 1));
        }
    }
    Ok(ledges)
}

/// The piece of the ledge at `at` (None when its points cannot be read or it has none).
fn read_ledge(surf: &[u8], at: usize, budget: &mut Budget) -> Result<Option<Ledge>> {
    let head = slice(surf, at, LEDGE_BYTES, "a ledge")?;
    let point_offset = i32_at(head, 0, "a ledge")?;
    let tris = usize::from(u16_at(head, 12, "a ledge")?);
    if tris == 0 {
        return Ok(None);
    }
    cap(
        "a ledge's triangles",
        tris as u64,
        MAX_LEDGE_TRIANGLES as u64,
    )?;
    spend(&mut budget.triangles, tris, "the triangles", MAX_TRIANGLES)?;
    let points_at = rel(at, point_offset, "a ledge's points")?;
    let tri_at = at + LEDGE_BYTES;
    let tri_bytes = slice(surf, tri_at, tris * TRIANGLE_BYTES, "a ledge's triangles")?;
    let mut used: Vec<u16> = Vec::with_capacity(tris * 3);
    for t in 0..tris {
        for e in 0..3 {
            let edge = u32_at(tri_bytes, t * TRIANGLE_BYTES + 4 + e * 4, "a triangle")?;
            used.push((edge & 0xffff) as u16);
        }
    }
    used.sort_unstable();
    used.dedup();
    let mut points = Vec::with_capacity(used.len());
    for &i in &used {
        let off = points_at
            .checked_add(usize::from(i) * POINT_BYTES)
            .ok_or(Error::Truncated {
                what: "a ledge's points",
            })?;
        let p = match vec3_at(surf, off, "a ledge's points") {
            Ok(p) => p,
            // A ledge naming a point the solid does not hold is left out whole.
            Err(_) => return Ok(None),
        };
        let p = to_model(p);
        if p.iter().any(|v| !v.is_finite() || v.abs() > MAX_COORDINATE) {
            return Ok(None);
        }
        points.push(p);
    }
    let mut work = budget.hull_work;
    let hulled = convex_hull(&points, &mut work);
    budget.hull_work = work;
    let (hull, boxed) = match hulled {
        Some(h) => (h, false),
        None => {
            let (lo, hi) = bounds(&points);
            if (0..3).all(|k| hi[k] - lo[k] > 1e-3) {
                (Hull::of_box(lo, hi), true)
            } else {
                return Ok(None);
            }
        }
    };
    Ok(Some(Ledge {
        points,
        hull,
        boxed,
    }))
}

fn number(s: Option<&str>) -> f32 {
    s.and_then(|v| v.trim().parse::<f32>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(0.0)
}

fn pairs(block: &[kv::Pair]) -> Vec<(String, String)> {
    block
        .iter()
        .filter_map(|p| p.str().map(|v| (p.key.clone(), v.to_string())))
        .collect()
}

/// The key-values tail: `solid`, `ragdollconstraint`, `collisionrules` and `editparams` blocks (others are left out). Indices past
/// `solids` are dropped.
fn read_text(bytes: &[u8], solids: usize) -> Result<PhyText> {
    // The text ends at its first NUL (the file may pad it).
    let end = bytes.iter().position(|&c| c == 0).unwrap_or(bytes.len());
    let top = kv::parse(&bytes[..end])?;
    let mut out = PhyText::default();
    for p in &top {
        let Some(block) = p.block() else { continue };
        let key = p.key.to_ascii_lowercase();
        let index_ok = |v: f32| v >= 0.0 && (v as usize) < solids;
        match key.as_str() {
            "solid" if out.solids.len() < MAX_BLOCKS => {
                let index = number(kv::find_str(block, "index"));
                if !index_ok(index) {
                    continue;
                }
                let text = |k: &str| kv::find_str(block, k).unwrap_or("").to_string();
                out.solids.push(SolidProps {
                    index: index as usize,
                    name: text("name"),
                    parent: text("parent"),
                    surface_prop: text("surfaceprop"),
                    mass: number(kv::find_str(block, "mass")),
                    mass_center_override: kv::find_str(block, "massCenterOverride")
                        .and_then(crate::bsp::vec3)
                        .filter(|v| v.iter().all(|c| c.is_finite())),
                    damping: number(kv::find_str(block, "damping")),
                    rot_damping: number(kv::find_str(block, "rotdamping")),
                    drag: number(kv::find_str(block, "drag")),
                    inertia: number(kv::find_str(block, "inertia")),
                    rot_inertia_limit: number(kv::find_str(block, "rotInertiaLimit")),
                    volume: number(kv::find_str(block, "volume")),
                });
            }
            "ragdollconstraint" if out.constraints.len() < MAX_BLOCKS => {
                let parent = number(kv::find_str(block, "parent"));
                let child = number(kv::find_str(block, "child"));
                if !index_ok(parent) || !index_ok(child) {
                    continue;
                }
                let axis = |a: &str| {
                    (
                        number(kv::find_str(block, &format!("{a}min"))),
                        number(kv::find_str(block, &format!("{a}max"))),
                        number(kv::find_str(block, &format!("{a}friction"))),
                    )
                };
                let (x, y, z) = (axis("x"), axis("y"), axis("z"));
                out.constraints.push(Constraint {
                    parent: parent as usize,
                    child: child as usize,
                    min: [x.0, y.0, z.0],
                    max: [x.1, y.1, z.1],
                    friction: [x.2, y.2, z.2],
                });
            }
            "collisionrules" => out.collision_rules.extend(pairs(block)),
            "editparams" => out.edit_params.extend(pairs(block)),
            _ => {}
        }
    }
    out.collision_rules.truncate(MAX_BLOCKS);
    out.edit_params.truncate(MAX_BLOCKS);
    Ok(out)
}

/// Physics models: `.phy` as [`Phy`], from any source.
#[derive(Clone, Copy, Debug, Default, PluginId)]
#[plugin(id = "torque.format-phy")]
pub struct PhyFormatPlugin;

impl PhyFormatPlugin {
    /// What the loader takes.
    pub const SPEC: Spec = Spec::untrusted(MAX_FILE_BYTES);
}

impl Plugin for PhyFormatPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine
            .assets()
            .loader(&["phy"], Self::SPEC, |bytes, _: &LoadCx<'_>| {
                Phy::parse(bytes).map_err(|e| e.to_string())
            });
    }
}
