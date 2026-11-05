//! The reader of a `.dif` interior resource.
//!
//! The layout follows Torque3D 2.0's MIT-licensed `InteriorResource::read` (`Engine/source/interior/interiorRes.cpp`) and
//! `Interior::read` (`interiorIO.cpp`), `Copyright (c) 2012 GarageGames, LLC`, and the binary form of `MaterialList::read`
//! (`Engine/source/materials/materialList.cpp`, `Copyright (c) 2012 GarageGames, LLC` and `Copyright (C) 2015 Faust Logic,
//! Inc.`); the code here is this crate's own.
//!
//! ```text
//! u32 resource version (44)
//! bool preview, then a PNG when set
//! u32 detail levels, each an interior            u32 sub-objects, each an interior
//! u32 triggers   u32 path followers   u32 force fields   u32 AI special nodes
//! u32 1 when vehicle collision for detail 0 follows
//! u32 2 when game entities follow (u32 count, the entities, u32 reserved)
//! ```
//!
//! An interior is a version word (0 to 14) and its vectors: planes, points, point visibility, texture planes, BSP nodes and solid
//! leaves, the material list, windings and fans, edges (12 on), zones and portals, surfaces, the lightmaps (PNGs), animated
//! lights, sub-objects (mirrors), convex hulls and their index lists, coordinate bins, the ambient colours, and from 10 on static
//! meshes, normals and texture matrices. Some fields change width with the version (`Interior::read`'s tests are kept here).
//!
//! **Version 0.** A version-0 interior has two forms: TGE's, and Torque3D's, which adds a byte to each surface and a second
//! PNG to each lightmap. As Torque3D's `Interior::read` does, the surfaces are read in Torque3D's form first with checks on
//! every index; when a check fails the reader goes back and reads them in TGE's form ([`Interior::plain_surfaces`]).
//!
//! Beyond Torque3D the reader checks every index it reads against the vector it names where Torque3D trusts the file (a plane's
//! normal, a mirror's key, a reserved word), and [`Interior::check`] checks the cross-references the collision and drawing code
//! follow, so that code can index without a bound check failing at run time.

use crate::png::{self, Png};
use crate::read::Reader;

#[path = "dif_write.rs"]
mod writer;
use crate::{MAX_DIF_BYTES, MAX_ENTRIES, MAX_INTERIORS, MapError, size_ok};
pub use writer::write;

/// The only resource version read (`InteriorResource::smFileVersion`).
pub const RESOURCE_VERSION: u32 = 44;
/// The newest interior version read (`Interior::smFileVersion`).
pub const NEWEST_INTERIOR_VERSION: u32 = 14;
/// A force field's only version (`ForceField::smFileVersion`).
pub const FORCE_FIELD_VERSION: u32 = 0;
/// The coordinate bins of an interior: 16 by 16.
pub const COORD_BINS: usize = 16 * 16;
/// The fewest bytes an interior can be (its coordinate bins alone are 2,048).
const MIN_INTERIOR_BYTES: usize = 2_048;
/// A material list's binary version.
pub const MATERIAL_LIST_VERSION: u8 = 1;
/// The key of a mirror sub-object (the only kind `InteriorSubObject::readISO` makes).
pub const MIRROR_KEY: u32 = 1;
/// The flag of a plane index whose plane is the stored one negated.
pub const PLANE_FLIPPED: u16 = 0x8000;
/// The flag of a surface index that names a null surface.
pub const NULL_SURFACE: u32 = 0x8000_0000;

/// Surface flags (`Interior::SurfaceFlags`).
pub mod surface_flags {
    pub const DETAIL: u8 = 1;
    pub const AMBIGUOUS: u8 = 2;
    pub const ORPHAN: u8 = 4;
    pub const SHARED_LMAPS: u8 = 8;
    pub const OUTSIDE_VISIBLE: u8 = 16;
    pub const STATIC_MESH: u8 = 32;
    /// This engine's own bits, above TGE's: TGE's map2dif masks every surface's flags with `SurfaceFlagMask` (bits 0 to 5) and
    /// its reader keeps the byte as it is, so a file those tools made never has them and TGE ignores them. Every surface of a
    /// material carries the same ones.
    ///
    /// Drawn see-through (alpha blended, in the translucent pass); it still collides as its hulls do.
    pub const TRANSLUCENT: u8 = 64;
    /// Its texture sampled with nearest-neighbor filtering (pixel art).
    pub const NEAREST: u8 = 128;
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane {
    pub normal: [f32; 3],
    pub d: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// A texture's two planes: `u = x · p + x[3]`, `v = y · p + y[3]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TexGen {
    pub x: [f32; 4],
    pub y: [f32; 4],
}

/// Where a BSP node's side leads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BspChild {
    Node(u32),
    /// A solid leaf: an index into [`Interior::bsp_solid_leaves`].
    Solid(u32),
    /// An empty leaf and its zone.
    Empty(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BspNode {
    /// A plane index, [`PLANE_FLIPPED`] set when the plane is negated.
    pub plane: u16,
    pub front: BspChild,
    pub back: BspChild,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SolidLeaf {
    pub surface_start: u32,
    pub surface_count: u16,
}

/// A run of windings (`TriFan`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fan {
    pub start: u32,
    pub count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    pub vertices: [i32; 2],
    pub faces: [i32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Zone {
    pub portal_start: u16,
    pub portal_count: u16,
    pub surface_start: u32,
    pub surface_count: u16,
    pub static_mesh_start: u32,
    pub static_mesh_count: u32,
    pub flags: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Portal {
    pub plane: u16,
    pub fan_count: u16,
    pub fan_start: u32,
    pub zone_front: u16,
    pub zone_back: u16,
}

/// A drawn surface: a triangle strip of `winding_count` windings from `winding_start`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Surface {
    pub winding_start: u32,
    pub winding_count: u32,
    /// A plane index, [`PLANE_FLIPPED`] set when the plane is negated.
    pub plane: u16,
    pub texture: u16,
    pub texgen: u32,
    pub flags: u8,
    pub fan_mask: u32,
    /// The lightmap's texture planes, unpacked from the file's packed word.
    pub lightmap_texgen: TexGen,
    pub light_count: u16,
    pub light_state_start: u32,
    pub map_offset: [u32; 2],
    pub map_size: [u32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NullSurface {
    pub winding_start: u32,
    pub plane: u16,
    pub flags: u8,
    pub winding_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lightmap {
    pub png: Png,
    /// The light direction map of Torque3D's form (version 0, and 5 and 12 on), kept but unused.
    pub direction: Option<Png>,
    pub keep: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnimatedLight {
    pub name: u32,
    pub state_start: u32,
    pub state_count: u16,
    pub flags: u16,
    pub duration_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LightState {
    pub rgb: [u8; 3],
    pub active_ms: u32,
    pub data_start: u32,
    pub data_count: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LightStateData {
    pub surface: u32,
    pub map: u32,
    pub light_state: u16,
}

/// A mirror sub-object.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mirror {
    pub detail_level: u32,
    pub zone: u32,
    pub alpha: f32,
    pub surface_count: u32,
    pub surface_start: u32,
    pub centroid: [f32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConvexHull {
    pub hull_start: u32,
    pub hull_count: u16,
    pub bounds: Aabb,
    pub surface_start: u32,
    pub surface_count: u16,
    pub plane_start: u32,
    pub poly_list_plane_start: u32,
    pub poly_list_point_start: u32,
    pub poly_list_string_start: u32,
    pub static_mesh: bool,
}

/// The convex hulls and the lists they index (an interior's, or its vehicle collision's).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Hulls {
    pub hulls: Vec<ConvexHull>,
    pub emit_strings: Vec<u8>,
    /// Point indices.
    pub hull_indices: Vec<u32>,
    pub hull_plane_indices: Vec<u16>,
    pub hull_emit_string_indices: Vec<u32>,
    /// Surface indices, [`NULL_SURFACE`] set for a null surface.
    pub hull_surface_indices: Vec<u32>,
    pub poly_list_planes: Vec<u16>,
    pub poly_list_points: Vec<u32>,
    pub poly_list_strings: Vec<u8>,
}

/// The vehicle collision of detail 0 (`Interior::readVehicleCollision`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VehicleCollision {
    pub version: u32,
    pub hulls: Hulls,
    pub null_surfaces: Vec<NullSurface>,
    pub points: Vec<[f32; 3]>,
    pub planes: Vec<Plane>,
    pub windings: Vec<u32>,
    pub winding_indices: Vec<Fan>,
}

/// A static mesh primitive (`InteriorSimpleMesh::primitive`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshPrimitive {
    pub alpha: bool,
    pub tex_s: u32,
    pub tex_t: u32,
    pub diffuse: i32,
    pub lightmap: i32,
    pub start: u32,
    pub count: u32,
    pub lightmap_x: [f32; 4],
    pub lightmap_y: [f32; 4],
    pub lightmap_offset: [i32; 2],
    pub lightmap_size: [i32; 2],
}

/// A TS material list of version 25 (a static mesh's).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TsMaterials {
    pub names: Vec<String>,
    pub flags: Vec<u32>,
    pub reflectance: Vec<u32>,
    pub bump: Vec<u32>,
    pub detail: Vec<u32>,
    pub detail_scales: Vec<f32>,
    pub reflection_amounts: Vec<f32>,
}

/// A static mesh (interior versions 10 on).
#[derive(Debug, Clone, PartialEq)]
pub struct StaticMesh {
    pub primitives: Vec<MeshPrimitive>,
    pub indices: Vec<u16>,
    pub verts: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub diffuse_uvs: Vec<[f32; 2]>,
    pub lightmap_uvs: Vec<[f32; 2]>,
    pub materials: Option<TsMaterials>,
    pub diffuse_bitmaps: Vec<Option<Png>>,
    pub has_solid: bool,
    pub has_translucency: bool,
    pub bounds: Aabb,
    pub transform: [f32; 16],
    pub scale: [f32; 3],
}

/// One interior: a detail level or a sub-object of a resource.
#[derive(Debug, Clone, PartialEq)]
pub struct Interior {
    pub version: u32,
    /// The surfaces and lightmaps were read the plain way (a version-0 file without the extra fields).
    pub plain_surfaces: bool,
    pub detail_level: u32,
    pub min_pixels: u32,
    pub bounds: Aabb,
    /// Centre and radius.
    pub sphere: [f32; 4],
    pub has_alarm_state: bool,
    pub light_state_entries: u32,
    pub planes: Vec<Plane>,
    pub points: Vec<[f32; 3]>,
    pub point_visibility: Vec<u8>,
    pub texgens: Vec<TexGen>,
    pub bsp_nodes: Vec<BspNode>,
    pub bsp_solid_leaves: Vec<SolidLeaf>,
    /// The material names, each cut after its last `/` or `\` as `MaterialList::read` does.
    pub materials: Vec<String>,
    /// Point indices.
    pub windings: Vec<u32>,
    pub winding_indices: Vec<Fan>,
    pub edges: Vec<Edge>,
    pub zones: Vec<Zone>,
    pub zone_surfaces: Vec<u16>,
    pub zone_static_meshes: Vec<u32>,
    pub zone_portals: Vec<u16>,
    pub portals: Vec<Portal>,
    pub surfaces: Vec<Surface>,
    pub normal_lightmaps: Vec<u32>,
    pub alarm_lightmaps: Vec<u32>,
    pub null_surfaces: Vec<NullSurface>,
    pub lightmaps: Vec<Lightmap>,
    /// Surface indices, [`NULL_SURFACE`] set for a null surface.
    pub solid_leaf_surfaces: Vec<u32>,
    pub animated_lights: Vec<AnimatedLight>,
    pub light_states: Vec<LightState>,
    pub state_data: Vec<LightStateData>,
    pub state_data_flags: u32,
    pub state_data_buffer: Vec<u8>,
    pub name_buffer: Vec<u8>,
    pub mirrors: Vec<Mirror>,
    pub hulls: Hulls,
    /// 256 (start, count) runs of [`Interior::coord_bin_indices`].
    pub coord_bins: Vec<[u32; 2]>,
    /// Hull indices.
    pub coord_bin_indices: Vec<u16>,
    pub coord_bin_mode: u32,
    pub base_ambient: [u8; 4],
    pub alarm_ambient: [u8; 4],
    pub static_meshes: Vec<StaticMesh>,
    pub normals: Vec<[f32; 3]>,
    /// T, N and B indices into [`Interior::normals`].
    pub tex_matrices: Vec<[i32; 3]>,
    pub tex_mat_indices: Vec<u32>,
    pub lightmap_border: Option<u32>,
    pub vehicle: Option<VehicleCollision>,
}

/// A trigger's name, datablock and fields (`InteriorDict`).
pub type Dict = Vec<(String, String)>;

#[derive(Debug, Clone, PartialEq)]
pub struct Trigger {
    pub name: String,
    pub datablock: String,
    pub dict: Dict,
    pub points: Vec<[f32; 3]>,
    pub planes: Vec<[f32; 4]>,
    /// Face 0, face 1, vertex 0, vertex 1.
    pub edges: Vec<[u32; 4]>,
    pub offset: [f32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WayPoint {
    pub pos: [f32; 3],
    pub rot: [f32; 4],
    pub ms_to_next: u32,
    pub smoothing: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PathFollower {
    pub name: String,
    pub datablock: String,
    pub interior: u32,
    pub offset: [f32; 3],
    pub dict: Dict,
    pub triggers: Vec<u32>,
    pub way_points: Vec<WayPoint>,
    pub total_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForceFieldSurface {
    pub winding_start: u32,
    pub winding_count: u8,
    pub plane: u16,
    pub flags: u8,
    pub fan_mask: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ForceField {
    pub name: String,
    pub triggers: Vec<String>,
    pub bounds: Aabb,
    pub sphere: [f32; 4],
    pub planes: Vec<Plane>,
    pub points: Vec<[f32; 3]>,
    /// Plane, front, back.
    pub bsp_nodes: Vec<[u16; 3]>,
    pub bsp_solid_leaves: Vec<SolidLeaf>,
    pub windings: Vec<u32>,
    pub surfaces: Vec<ForceFieldSurface>,
    pub solid_leaf_surfaces: Vec<u32>,
    pub color: [u8; 4],
}

#[derive(Debug, Clone, PartialEq)]
pub struct AiNode {
    pub name: String,
    pub pos: [f32; 3],
}

#[derive(Debug, Clone, PartialEq)]
pub struct GameEntity {
    pub datablock: String,
    pub class: String,
    pub pos: [f32; 3],
    pub dict: Dict,
}

/// A whole `.dif`.
#[derive(Debug, Clone, PartialEq)]
pub struct Dif {
    pub preview: Option<Png>,
    pub details: Vec<Interior>,
    pub sub_objects: Vec<Interior>,
    pub triggers: Vec<Trigger>,
    pub path_followers: Vec<PathFollower>,
    pub force_fields: Vec<ForceField>,
    pub ai_nodes: Vec<AiNode>,
    pub game_entities: Vec<GameEntity>,
    /// The interiors are in the variant layout with a zero word after a zero lightmap-border flag (read by
    /// [`parse_with_border_pad`] only; TGE's and Torque3D's readers have no such word).
    pub border_pad: bool,
    /// Bytes after the last field (Torque ignores them).
    pub trailing: usize,
}

/// The header of a `.dif`, without reading the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub resource_version: u32,
    pub preview: bool,
    pub details: u32,
    /// The first interior's version (when there is no preview).
    pub interior_version: Option<u32>,
}

/// The first words of a `.dif`.
pub fn header(bytes: &[u8]) -> Result<Header, MapError> {
    let mut r = Reader::new(bytes);
    let resource_version = r.u32("resource version")?;
    let preview = r.bool("preview flag")?;
    let details = r.u32("detail count")?;
    let interior_version = if preview {
        None
    } else {
        r.u32("interior version").ok()
    };
    Ok(Header {
        resource_version,
        preview,
        details,
        interior_version,
    })
}

/// Reads a whole `.dif` as TGE and Torque3D lay it out.
pub fn parse(bytes: &[u8]) -> Result<Dif, MapError> {
    size_ok(bytes.len(), MAX_DIF_BYTES)?;
    parse_as(bytes, false)
}

/// [`parse`], and for a file whose end does not read that way (a flag that is not one, or bytes left over) a second reading
/// in the variant layout with a zero word after each zero lightmap-border flag ([`Dif::border_pad`]), which TGE's and Torque3D's
/// readers do not have. The second reading is kept only when it reads to the end.
pub fn parse_with_border_pad(bytes: &[u8]) -> Result<Dif, MapError> {
    size_ok(bytes.len(), MAX_DIF_BYTES)?;
    let first = parse_as(bytes, false);
    if matches!(&first, Ok(d) if d.trailing == 0) {
        return first;
    }
    match parse_as(bytes, true) {
        Ok(d) if d.trailing == 0 => Ok(d),
        _ => first,
    }
}

fn parse_as(bytes: &[u8], pad: bool) -> Result<Dif, MapError> {
    let mut r = Reader::new(bytes);
    let mut read_interior = |r: &mut Reader| read_interior(r, pad);
    let version = r.u32("resource version")?;
    if version != RESOURCE_VERSION {
        return Err(MapError::Version {
            what: "resource",
            found: version,
        });
    }
    let preview = if r.bool("preview flag")? {
        Some(png::read(&mut r, "preview")?)
    } else {
        None
    };
    let n = r.count("detail levels", MIN_INTERIOR_BYTES, MAX_INTERIORS)?;
    let mut details = r.items(n, MIN_INTERIOR_BYTES, &mut read_interior)?;
    let n = r.count("sub-objects", MIN_INTERIOR_BYTES, MAX_INTERIORS)?;
    let sub_objects = r.items(n, MIN_INTERIOR_BYTES, &mut read_interior)?;
    let n = r.count("triggers", 30, MAX_ENTRIES)?;
    let triggers = r.items(n, 30, &mut read_trigger)?;
    let n = r.count("path followers", 34, MAX_ENTRIES)?;
    let path_followers = r.items(n, 34, &mut read_path_follower)?;
    let n = r.count("force fields", 89, MAX_ENTRIES)?;
    let force_fields = r.items(n, 89, &mut read_force_field)?;
    let n = r.count("AI special nodes", 13, MAX_ENTRIES)?;
    let ai_nodes = r.items(n, 13, &mut |r| {
        Ok(AiNode {
            name: r.string("AI node name")?,
            pos: r.f32s("AI node position")?,
        })
    })?;
    match r.u32("vehicle collision flag")? {
        0 => {}
        1 => {
            let Some(first) = details.first_mut() else {
                return Err(r.bad("vehicle collision with no detail level", 1));
            };
            first.vehicle = Some(read_vehicle(&mut r)?);
        }
        v => return Err(r.bad("vehicle collision flag", v as u64)),
    }
    let mut game_entities = Vec::new();
    match r.u32("game entity flag")? {
        0 => {}
        2 => {
            let n = r.count("game entities", 18, MAX_ENTRIES)?;
            game_entities = r.items(n, 18, &mut |r| {
                Ok(GameEntity {
                    datablock: r.string("game entity datablock")?,
                    class: r.string("game entity class")?,
                    pos: r.f32s("game entity position")?,
                    dict: read_dict(r)?,
                })
            })?;
            r.u32("game entity reserved word")?;
        }
        v => return Err(r.bad("game entity flag", v as u64)),
    }
    Ok(Dif {
        preview,
        details,
        sub_objects,
        triggers,
        path_followers,
        force_fields,
        ai_nodes,
        game_entities,
        border_pad: pad,
        trailing: r.left(),
    })
}

fn read_dict(r: &mut Reader) -> Result<Dict, MapError> {
    r.vec("dictionary", 2, |r| {
        Ok((r.string("dictionary name")?, r.string("dictionary value")?))
    })
}

fn read_trigger(r: &mut Reader) -> Result<Trigger, MapError> {
    Ok(Trigger {
        name: r.string("trigger name")?,
        datablock: r.string("trigger datablock")?,
        dict: read_dict(r)?,
        points: r.vec("trigger points", 12, |r| r.f32s("trigger point"))?,
        planes: r.vec("trigger planes", 16, |r| r.f32s("trigger plane"))?,
        edges: r.vec("trigger edges", 16, |r| {
            Ok([
                r.u32("trigger edge")?,
                r.u32("trigger edge")?,
                r.u32("trigger edge")?,
                r.u32("trigger edge")?,
            ])
        })?,
        offset: r.f32s("trigger offset")?,
    })
}

fn read_path_follower(r: &mut Reader) -> Result<PathFollower, MapError> {
    Ok(PathFollower {
        name: r.string("path follower name")?,
        datablock: r.string("path follower datablock")?,
        interior: r.u32("path follower interior")?,
        offset: r.f32s("path follower offset")?,
        dict: read_dict(r)?,
        triggers: r.u32_vec("path follower triggers")?,
        way_points: r.vec("way points", 36, |r| {
            Ok(WayPoint {
                pos: r.f32s("way point position")?,
                rot: r.f32s("way point rotation")?,
                ms_to_next: r.u32("way point time")?,
                smoothing: r.u32("way point smoothing")?,
            })
        })?,
        total_ms: r.u32("path follower time")?,
    })
}

/// `readPlaneVector`: the unique normals, then each plane as a normal index and a distance.
fn read_planes(r: &mut Reader) -> Result<Vec<Plane>, MapError> {
    let normals = r.vec("plane normals", 12, |r| r.f32s::<3>("plane normal"))?;
    r.vec("planes", 6, |r| {
        let index = r.u16("plane normal index")?;
        let d = r.f32("plane distance")?;
        let normal = *normals.get(index as usize).ok_or(MapError::Index {
            what: "plane normal index",
            value: index as u64,
            len: normals.len() as u64,
        })?;
        Ok(Plane { normal, d })
    })
}

fn read_force_field(r: &mut Reader) -> Result<ForceField, MapError> {
    let version = r.u32("force field version")?;
    if version != FORCE_FIELD_VERSION {
        return Err(MapError::Version {
            what: "force field",
            found: version,
        });
    }
    Ok(ForceField {
        name: r.string("force field name")?,
        triggers: r.vec("force field triggers", 1, |r| {
            r.string("force field trigger")
        })?,
        bounds: read_box(r)?,
        sphere: r.f32s("force field sphere")?,
        planes: read_planes(r)?,
        points: r.vec("force field points", 12, |r| r.f32s("force field point"))?,
        bsp_nodes: r.vec("force field BSP nodes", 6, |r| {
            Ok([r.u16("BSP node")?, r.u16("BSP node")?, r.u16("BSP node")?])
        })?,
        bsp_solid_leaves: r.vec("force field solid leaves", 6, read_solid_leaf)?,
        windings: r.u32_vec("force field windings")?,
        surfaces: r.vec("force field surfaces", 12, |r| {
            Ok(ForceFieldSurface {
                winding_start: r.u32("force field surface")?,
                winding_count: r.u8("force field surface")?,
                plane: r.u16("force field surface")?,
                flags: r.u8("force field surface")?,
                fan_mask: r.u32("force field surface")?,
            })
        })?,
        solid_leaf_surfaces: r.u32_vec("force field solid leaf surfaces")?,
        color: r.rgba("force field colour")?,
    })
}

fn read_box(r: &mut Reader) -> Result<Aabb, MapError> {
    Ok(Aabb {
        min: r.f32s("box")?,
        max: r.f32s("box")?,
    })
}

fn read_solid_leaf(r: &mut Reader) -> Result<SolidLeaf, MapError> {
    Ok(SolidLeaf {
        surface_start: r.u32("solid leaf surface start")?,
        surface_count: r.u16("solid leaf surface count")?,
    })
}

fn read_fan(r: &mut Reader) -> Result<Fan, MapError> {
    Ok(Fan {
        start: r.u32("fan start")?,
        count: r.u32("fan count")?,
    })
}

/// `MaterialList::read` in its binary form: a version byte, a count and the names (a text list is not valid inside a `.dif`).
fn read_material_names(r: &mut Reader) -> Result<Vec<String>, MapError> {
    let version = r.u8("material list version")?;
    if version != MATERIAL_LIST_VERSION {
        return Err(MapError::Version {
            what: "material list",
            found: version as u32,
        });
    }
    r.vec("materials", 2, |r| {
        let at = r.pos();
        let name = r.string("material name")?;
        if name.is_empty() {
            return Err(MapError::Bad {
                what: "empty material name",
                value: 0,
                at,
            });
        }
        let cut = name.rfind(['/', '\\']).map_or(0, |i| i + 1);
        Ok(name[cut..].to_string())
    })
}

fn bsp_child(index: u32, version: u32) -> BspChild {
    let (leaf, solid, mask) = if version >= 14 {
        (0x8_0000, 0x4_0000, !0xc_0000u32)
    } else {
        (0x8000, 0x4000, !0xc000u32)
    };
    if index & leaf == 0 {
        BspChild::Node(index)
    } else if index & solid != 0 {
        BspChild::Solid(index & mask)
    } else {
        BspChild::Empty(index & mask)
    }
}

/// `readLMapTexGen`: a packed word (two log scales and which axes) and the two offsets. None for an axis code above 5 (which
/// Torque3D's reader takes as a sign of a version-0 surface in TGE's form).
fn read_lightmap_texgen(r: &mut Reader) -> Result<Option<TexGen>, MapError> {
    let word = r.u16("lightmap texgen")?;
    let dx = r.f32("lightmap texgen")?;
    let dy = r.f32("lightmap texgen")?;
    let log_y = (word & 0x3f) as i32;
    let log_x = ((word >> 6) & 0x3f) as i32;
    let (s, t) = match (word >> 13) & 7 {
        0 => (0, 1),
        1 => (0, 2),
        2 => (1, 0),
        3 => (1, 2),
        4 => (2, 0),
        5 => (2, 1),
        _ => return Ok(None),
    };
    let mut x = [0.0, 0.0, 0.0, dx];
    let mut y = [0.0, 0.0, 0.0, dy];
    x[s] = 0.5f64.powi(log_x) as f32;
    y[t] = 0.5f64.powi(log_y) as f32;
    Ok(Some(TexGen { x, y }))
}

/// The vectors a surface's checks need while the surfaces are read.
struct Sizes {
    windings: usize,
    planes: usize,
    materials: usize,
    texgens: usize,
}

/// `Interior::readSurface`: None when a check fails (for version 0, the sign of the plain form).
fn read_surface(
    r: &mut Reader,
    version: u32,
    plain_surfaces: bool,
    s: &Sizes,
) -> Result<Option<Surface>, MapError> {
    let winding_start = r.u32("surface winding start")?;
    let winding_count = if version >= 13 {
        r.u32("surface winding count")?
    } else {
        r.u8("surface winding count")? as u32
    };
    if winding_start as u64 + winding_count as u64 > s.windings as u64
        || winding_start as usize >= s.windings
    {
        return Ok(None);
    }
    let plane = r.u16("surface plane")?;
    if (plane & !PLANE_FLIPPED) as usize >= s.planes {
        return Ok(None);
    }
    let texture = r.u16("surface texture")?;
    if texture as usize >= s.materials {
        return Ok(None);
    }
    let texgen = r.u32("surface texgen")?;
    if texgen as usize >= s.texgens {
        return Ok(None);
    }
    let flags = r.u8("surface flags")?;
    let fan_mask = r.u32("surface fan mask")?;
    let Some(lightmap_texgen) = read_lightmap_texgen(r)? else {
        return Ok(None);
    };
    let light_count = r.u16("surface light count")?;
    let light_state_start = r.u32("surface light state start")?;
    let (map_offset, map_size) = if version >= 13 {
        (
            [r.u32("surface map offset")?, r.u32("surface map offset")?],
            [r.u32("surface map size")?, r.u32("surface map size")?],
        )
    } else {
        (
            [
                r.u8("surface map offset")? as u32,
                r.u8("surface map offset")? as u32,
            ],
            [
                r.u8("surface map size")? as u32,
                r.u8("surface map size")? as u32,
            ],
        )
    };
    if !plain_surfaces && (version == 0 || version == 5 || version >= 12) {
        r.u8("surface unused byte")?;
    }
    if version == 5 {
        r.u32("surface brush id")?;
    }
    Ok(Some(Surface {
        winding_start,
        winding_count,
        plane,
        texture,
        texgen,
        flags,
        fan_mask,
        lightmap_texgen,
        light_count,
        light_state_start,
        map_offset,
        map_size,
    }))
}

/// The surfaces read one way; None when a check fails or the file ends.
fn read_surfaces_as(
    r: &mut Reader,
    n: usize,
    version: u32,
    plain_surfaces: bool,
    s: &Sizes,
) -> Option<Vec<Surface>> {
    let mut out = crate::reserved(n, r.left(), 22);
    for _ in 0..n {
        out.push(read_surface(r, version, plain_surfaces, s).ok()??);
    }
    Some(out)
}

fn read_null_surface(r: &mut Reader, wide: bool) -> Result<NullSurface, MapError> {
    Ok(NullSurface {
        winding_start: r.u32("null surface winding start")?,
        plane: r.u16("null surface plane")?,
        flags: r.u8("null surface flags")?,
        winding_count: if wide {
            r.u32("null surface winding count")?
        } else {
            r.u8("null surface winding count")? as u32
        },
    })
}

fn read_hull(r: &mut Reader, with_static: bool) -> Result<ConvexHull, MapError> {
    let hull_start = r.u32("hull start")?;
    let hull_count = r.u16("hull count")?;
    let mut f = [0f32; 6];
    for v in &mut f {
        *v = r.f32("hull bounds")?;
    }
    Ok(ConvexHull {
        hull_start,
        hull_count,
        bounds: Aabb {
            min: [f[0], f[2], f[4]],
            max: [f[1], f[3], f[5]],
        },
        surface_start: r.u32("hull surface start")?,
        surface_count: r.u16("hull surface count")?,
        plane_start: r.u32("hull plane start")?,
        poly_list_plane_start: r.u32("hull poly list plane start")?,
        poly_list_point_start: r.u32("hull poly list point start")?,
        poly_list_string_start: r.u32("hull poly list string start")?,
        static_mesh: with_static && r.bool("hull static mesh flag")?,
    })
}

fn read_hulls(r: &mut Reader, with_static: bool) -> Result<Hulls, MapError> {
    let min = 44 + with_static as usize;
    Ok(Hulls {
        hulls: r.vec("convex hulls", min, |r| read_hull(r, with_static))?,
        emit_strings: r.byte_vec("hull emit strings")?,
        hull_indices: r.u32_vec("hull indices")?,
        hull_plane_indices: r.u16_vec("hull plane indices")?,
        hull_emit_string_indices: r.u32_vec("hull emit string indices")?,
        hull_surface_indices: r.u32_vec("hull surface indices")?,
        poly_list_planes: r.u16_vec("poly list planes")?,
        poly_list_points: r.u32_vec("poly list points")?,
        poly_list_strings: r.byte_vec("poly list strings")?,
    })
}

fn read_vehicle(r: &mut Reader) -> Result<VehicleCollision, MapError> {
    let version = r.u32("vehicle collision version")?;
    if version > NEWEST_INTERIOR_VERSION {
        return Err(MapError::Version {
            what: "vehicle collision",
            found: version,
        });
    }
    Ok(VehicleCollision {
        version,
        hulls: read_hulls(r, false)?,
        null_surfaces: r.vec("vehicle null surfaces", 11, |r| read_null_surface(r, true))?,
        points: r.vec("vehicle points", 12, |r| r.f32s("vehicle point"))?,
        planes: r.vec("vehicle planes", 16, |r| {
            let p: [f32; 4] = r.f32s("vehicle plane")?;
            Ok(Plane {
                normal: [p[0], p[1], p[2]],
                d: p[3],
            })
        })?,
        windings: r.u32_vec("vehicle windings")?,
        winding_indices: r.vec("vehicle winding indices", 8, read_fan)?,
    })
}

fn read_ts_materials(r: &mut Reader) -> Result<TsMaterials, MapError> {
    let names = read_material_names(r)?;
    let n = names.len();
    let mut words = |what: &'static str| -> Result<Vec<u32>, MapError> {
        r.fits(what, n as u64, 4, n)?;
        r.items(n, 4, &mut |r| r.u32(what))
    };
    let flags = words("TS material flags")?;
    let reflectance = words("TS material reflectance maps")?;
    let bump = words("TS material bump maps")?;
    let detail = words("TS material detail maps")?;
    words("TS material reserved words")?;
    let detail_scales = words("TS material detail scales")?;
    let reflection_amounts = words("TS material reflection amounts")?;
    Ok(TsMaterials {
        names,
        flags,
        reflectance,
        bump,
        detail,
        detail_scales: detail_scales.into_iter().map(f32::from_bits).collect(),
        reflection_amounts: reflection_amounts.into_iter().map(f32::from_bits).collect(),
    })
}

fn read_static_mesh(r: &mut Reader) -> Result<StaticMesh, MapError> {
    Ok(StaticMesh {
        primitives: r.vec("mesh primitives", 73, |r| {
            Ok(MeshPrimitive {
                alpha: r.bool("mesh primitive")?,
                tex_s: r.u32("mesh primitive")?,
                tex_t: r.u32("mesh primitive")?,
                diffuse: r.i32("mesh primitive")?,
                lightmap: r.i32("mesh primitive")?,
                start: r.u32("mesh primitive")?,
                count: r.u32("mesh primitive")?,
                lightmap_x: r.f32s("mesh primitive")?,
                lightmap_y: r.f32s("mesh primitive")?,
                lightmap_offset: [r.i32("mesh primitive")?, r.i32("mesh primitive")?],
                lightmap_size: [r.i32("mesh primitive")?, r.i32("mesh primitive")?],
            })
        })?,
        indices: r.u16_vec("mesh indices")?,
        verts: r.vec("mesh vertices", 12, |r| r.f32s("mesh vertex"))?,
        normals: r.vec("mesh normals", 12, |r| r.f32s("mesh normal"))?,
        diffuse_uvs: r.vec("mesh diffuse uvs", 8, |r| r.f32s("mesh uv"))?,
        lightmap_uvs: r.vec("mesh lightmap uvs", 8, |r| r.f32s("mesh uv"))?,
        materials: if r.bool("mesh material list flag")? {
            Some(read_ts_materials(r)?)
        } else {
            None
        },
        diffuse_bitmaps: r.vec("mesh diffuse bitmaps", 1, |r| {
            Ok(if r.bool("mesh bitmap flag")? {
                Some(png::read(r, "mesh bitmap")?)
            } else {
                None
            })
        })?,
        has_solid: r.bool("mesh solid flag")?,
        has_translucency: r.bool("mesh translucency flag")?,
        bounds: read_box(r)?,
        transform: r.f32s("mesh transform")?,
        scale: r.f32s("mesh scale")?,
    })
}

fn zero_word(r: &mut Reader, what: &'static str) -> Result<(), MapError> {
    match r.u32(what)? {
        0 => Ok(()),
        v => Err(r.bad(what, v as u64)),
    }
}

/// An interior; a version-0 one whose surfaces passed the Torque3D form's checks but whose later fields do not read is read
/// again in TGE's form from its start (Torque3D's reader decides at the surfaces alone).
fn read_interior(r: &mut Reader, pad: bool) -> Result<Interior, MapError> {
    let start = r.pos();
    match read_interior_as(r, false, pad) {
        Ok(i) => Ok(i),
        Err(e) => {
            r.seek(start);
            if r.u32("interior version") != Ok(0) {
                return Err(e);
            }
            r.seek(start);
            read_interior_as(r, true, pad).map_err(|_| e)
        }
    }
}

fn read_interior_as(r: &mut Reader, force_tge: bool, pad: bool) -> Result<Interior, MapError> {
    let version = r.u32("interior version")?;
    if version > NEWEST_INTERIOR_VERSION {
        return Err(MapError::Version {
            what: "interior",
            found: version,
        });
    }
    let detail_level = r.u32("detail level")?;
    let min_pixels = r.u32("min pixels")?;
    let bounds = read_box(r)?;
    let sphere = r.f32s("bounding sphere")?;
    let has_alarm_state = r.bool("alarm state flag")?;
    let light_state_entries = r.u32("light state entries")?;
    let planes = read_planes(r)?;
    let points = r.vec("points", 12, |r| r.f32s("point"))?;
    let point_visibility = r.byte_vec("point visibility")?;
    let texgens = r.vec("texgens", 32, |r| {
        Ok(TexGen {
            x: r.f32s("texgen")?,
            y: r.f32s("texgen")?,
        })
    })?;
    let wide_bsp = version >= 14;
    let bsp_nodes = r.vec("BSP nodes", if wide_bsp { 10 } else { 6 }, |r| {
        let plane = r.u16("BSP node plane")?;
        let (front, back) = if wide_bsp {
            (r.u32("BSP node front")?, r.u32("BSP node back")?)
        } else {
            (
                r.u16("BSP node front")? as u32,
                r.u16("BSP node back")? as u32,
            )
        };
        Ok(BspNode {
            plane,
            front: bsp_child(front, version),
            back: bsp_child(back, version),
        })
    })?;
    let bsp_solid_leaves = r.vec("BSP solid leaves", 6, read_solid_leaf)?;
    let materials = read_material_names(r)?;
    let windings = r.u32_vec("windings")?;
    let winding_indices = r.vec("winding indices", 8, read_fan)?;
    let read_edge = |r: &mut Reader| -> Result<Edge, MapError> {
        Ok(Edge {
            vertices: [r.i32("edge vertex")?, r.i32("edge vertex")?],
            faces: [r.i32("edge face")?, r.i32("edge face")?],
        })
    };
    let mut edges = if version >= 12 {
        r.vec("edges", 16, read_edge)?
    } else {
        Vec::new()
    };
    let zones = r.vec("zones", if version >= 12 { 20 } else { 12 }, |r| {
        let portal_start = r.u16("zone")?;
        let portal_count = r.u16("zone")?;
        let surface_start = r.u32("zone")?;
        let surface_count = r.u16("zone")?;
        let (static_mesh_start, static_mesh_count) = if version >= 12 {
            (r.u32("zone")?, r.u32("zone")?)
        } else {
            (0, 0)
        };
        Ok(Zone {
            portal_start,
            portal_count,
            surface_start,
            surface_count,
            static_mesh_start,
            static_mesh_count,
            flags: r.u16("zone")?,
        })
    })?;
    let zone_surfaces = r.u16_vec("zone surfaces")?;
    let zone_static_meshes = if version >= 12 {
        r.u32_vec("zone static meshes")?
    } else {
        Vec::new()
    };
    let zone_portals = r.u16_vec("zone portals")?;
    let portals = r.vec("portals", 12, |r| {
        Ok(Portal {
            plane: r.u16("portal")?,
            fan_count: r.u16("portal")?,
            fan_start: r.u32("portal")?,
            zone_front: r.u16("portal")?,
            zone_back: r.u16("portal")?,
        })
    })?;

    let n = r.count("surfaces", 22, crate::MAX_ITEMS)?;
    let sizes = Sizes {
        windings: windings.len(),
        planes: planes.len(),
        materials: materials.len(),
        texgens: texgens.len(),
    };
    let start = r.pos();
    let first = if force_tge {
        None
    } else {
        read_surfaces_as(r, n, version, false, &sizes)
    };
    let (surfaces, plain_surfaces) = match first {
        Some(s) => (s, false),
        None if version == 0 => {
            r.seek(start);
            match read_surfaces_as(r, n, version, true, &sizes) {
                Some(s) => (s, true),
                None => return Err(r.bad("surface", start as u64)),
            }
        }
        None => return Err(r.bad("surface", start as u64)),
    };

    if version == 5 {
        edges = r.vec("edges", 24, |r| {
            let vertices = [r.i32("edge vertex")?, r.i32("edge vertex")?];
            r.u32("edge normal")?;
            r.u32("edge normal")?;
            Ok(Edge {
                vertices,
                faces: [r.i32("edge face")?, r.i32("edge face")?],
            })
        })?;
        r.vec("old normals", 12, |r| r.f32s::<3>("old normal"))?;
        r.u16_vec("old normal indices")?;
    }
    let wide = version >= 13;
    let lightmap_index = |r: &mut Reader| -> Result<u32, MapError> {
        if wide {
            r.u32("lightmap index")
        } else {
            Ok(r.u8("lightmap index")? as u32)
        }
    };
    let normal_lightmaps = r.vec(
        "normal lightmap indices",
        if wide { 4 } else { 1 },
        lightmap_index,
    )?;
    let alarm_lightmaps = r.vec(
        "alarm lightmap indices",
        if wide { 4 } else { 1 },
        lightmap_index,
    )?;
    let null_surfaces = r.vec("null surfaces", if wide { 11 } else { 8 }, |r| {
        read_null_surface(r, wide)
    })?;
    let with_direction = !plain_surfaces && (version == 0 || version == 5 || version >= 12);
    let lightmaps = r.vec("lightmaps", 46, |r| {
        let png = png::read(r, "lightmap")?;
        let direction = if with_direction {
            Some(png::read(r, "light direction map")?)
        } else {
            None
        };
        Ok(Lightmap {
            png,
            direction,
            keep: r.bool("lightmap keep flag")?,
        })
    })?;
    let solid_leaf_surfaces = r.u32_vec("solid leaf surfaces")?;
    let animated_lights = r.vec("animated lights", 16, |r| {
        Ok(AnimatedLight {
            name: r.u32("animated light")?,
            state_start: r.u32("animated light")?,
            state_count: r.u16("animated light")?,
            flags: r.u16("animated light")?,
            duration_ms: r.u32("animated light")?,
        })
    })?;
    let light_states = r.vec("light states", 13, |r| {
        Ok(LightState {
            rgb: [
                r.u8("light state")?,
                r.u8("light state")?,
                r.u8("light state")?,
            ],
            active_ms: r.u32("light state")?,
            data_start: r.u32("light state")?,
            data_count: r.u16("light state")?,
        })
    })?;
    let state_data = r.vec("light state data", 10, |r| {
        Ok(LightStateData {
            surface: r.u32("light state data")?,
            map: r.u32("light state data")?,
            light_state: r.u16("light state data")?,
        })
    })?;
    let n = r.count("state data buffer", 1, crate::MAX_ITEMS)?;
    let state_data_flags = r.u32("state data flags")?;
    let state_data_buffer = r.bytes_copy(n, "state data buffer")?;
    let name_buffer = r.byte_vec("name buffer")?;
    let mirrors = r.vec("sub-objects", 32, |r| {
        let at = r.pos();
        let key = r.u32("sub-object key")?;
        if key != MIRROR_KEY {
            return Err(MapError::Bad {
                what: "sub-object key",
                value: key as u64,
                at,
            });
        }
        Ok(Mirror {
            detail_level: r.u32("mirror")?,
            zone: r.u32("mirror")?,
            alpha: r.f32("mirror")?,
            surface_count: r.u32("mirror")?,
            surface_start: r.u32("mirror")?,
            centroid: r.f32s("mirror")?,
        })
    })?;
    let hulls = read_hulls(r, version >= 12)?;
    let mut coord_bins = crate::reserved(COORD_BINS, r.left(), 8);
    for _ in 0..COORD_BINS {
        coord_bins.push([r.u32("coord bin")?, r.u32("coord bin")?]);
    }
    let coord_bin_indices = r.u16_vec("coord bin indices")?;
    let coord_bin_mode = r.u32("coord bin mode")?;
    let base_ambient = r.rgba("base ambient")?;
    let alarm_ambient = r.rgba("alarm ambient")?;
    let static_meshes = if version >= 10 {
        let n = r.count("static meshes", 131, MAX_ENTRIES)?;
        r.items(n, 131, &mut read_static_mesh)?
    } else {
        Vec::new()
    };
    let (normals, tex_matrices, tex_mat_indices) = if version >= 11 {
        (
            r.vec("normals", 12, |r| r.f32s("normal"))?,
            r.vec("texture matrices", 12, |r| {
                Ok([
                    r.i32("texture matrix")?,
                    r.i32("texture matrix")?,
                    r.i32("texture matrix")?,
                ])
            })?,
            r.u32_vec("texture matrix indices")?,
        )
    } else {
        (Vec::new(), Vec::new(), Vec::new())
    };
    if version < 10 {
        zero_word(r, "reserved word")?;
    }
    if version < 11 {
        zero_word(r, "reserved word")?;
        zero_word(r, "reserved word")?;
    }
    let lightmap_border = match r.u32("extended lightmap flag")? {
        1 => {
            let border = r.u32("lightmap border")?;
            zero_word(r, "reserved word")?;
            Some(border)
        }
        0 if pad => {
            zero_word(r, "word after the lightmap border flag")?;
            None
        }
        _ => None,
    };
    Ok(Interior {
        version,
        plain_surfaces,
        detail_level,
        min_pixels,
        bounds,
        sphere,
        has_alarm_state,
        light_state_entries,
        planes,
        points,
        point_visibility,
        texgens,
        bsp_nodes,
        bsp_solid_leaves,
        materials,
        windings,
        winding_indices,
        edges,
        zones,
        zone_surfaces,
        zone_static_meshes,
        zone_portals,
        portals,
        surfaces,
        normal_lightmaps,
        alarm_lightmaps,
        null_surfaces,
        lightmaps,
        solid_leaf_surfaces,
        animated_lights,
        light_states,
        state_data,
        state_data_flags,
        state_data_buffer,
        name_buffer,
        mirrors,
        hulls,
        coord_bins,
        coord_bin_indices,
        coord_bin_mode,
        base_ambient,
        alarm_ambient,
        static_meshes,
        normals,
        tex_matrices,
        tex_mat_indices,
        lightmap_border,
        vehicle: None,
    })
}

fn index(what: &'static str, value: u64, len: usize) -> Result<(), MapError> {
    if value < len as u64 {
        Ok(())
    } else {
        Err(MapError::Index {
            what,
            value,
            len: len as u64,
        })
    }
}

fn range(what: &'static str, start: u64, count: u64, len: usize) -> Result<(), MapError> {
    if start + count <= len as u64 {
        Ok(())
    } else {
        Err(MapError::Index {
            what,
            value: start + count,
            len: len as u64,
        })
    }
}

fn plane_index(what: &'static str, plane: u16, len: usize) -> Result<(), MapError> {
    index(what, (plane & !PLANE_FLIPPED) as u64, len)
}

/// A surface index of a solid leaf or a hull: a surface, or with [`NULL_SURFACE`] a null surface.
fn surface_ref(what: &'static str, s: u32, surfaces: usize, nulls: usize) -> Result<(), MapError> {
    if s & NULL_SURFACE != 0 {
        index(what, (s & !NULL_SURFACE) as u64, nulls)
    } else {
        index(what, s as u64, surfaces)
    }
}

impl Interior {
    /// The cross-references the collision and drawing code follow, each inside the vector it names: windings name points,
    /// surfaces, fans and null surfaces name windings and planes, BSP nodes name planes, nodes and solid leaves, solid leaves
    /// and hulls name surfaces, hulls name points, planes and emit strings, coordinate bins name hulls. Linear in the vectors'
    /// sizes (the ranges are tested, not walked).
    pub fn check(&self) -> Result<(), MapError> {
        let points = self.points.len();
        let planes = self.planes.len();
        let windings = self.windings.len();
        let surfaces = self.surfaces.len();
        let nulls = self.null_surfaces.len();
        for &w in &self.windings {
            index("winding point", w as u64, points)?;
        }
        for s in &self.surfaces {
            range(
                "surface windings",
                s.winding_start as u64,
                s.winding_count as u64,
                windings,
            )?;
            plane_index("surface plane", s.plane, planes)?;
            index("surface texture", s.texture as u64, self.materials.len())?;
            index("surface texgen", s.texgen as u64, self.texgens.len())?;
        }
        for f in &self.winding_indices {
            range("fan windings", f.start as u64, f.count as u64, windings)?;
        }
        for s in &self.null_surfaces {
            range(
                "null surface windings",
                s.winding_start as u64,
                s.winding_count as u64,
                windings,
            )?;
            plane_index("null surface plane", s.plane, planes)?;
        }
        for n in &self.bsp_nodes {
            plane_index("BSP node plane", n.plane, planes)?;
            for child in [n.front, n.back] {
                match child {
                    BspChild::Node(i) => index("BSP child node", i as u64, self.bsp_nodes.len())?,
                    BspChild::Solid(i) => {
                        index("BSP solid leaf", i as u64, self.bsp_solid_leaves.len())?
                    }
                    BspChild::Empty(_) => {}
                }
            }
        }
        for l in &self.bsp_solid_leaves {
            range(
                "solid leaf surfaces",
                l.surface_start as u64,
                l.surface_count as u64,
                self.solid_leaf_surfaces.len(),
            )?;
        }
        for &s in &self.solid_leaf_surfaces {
            surface_ref("solid leaf surface", s, surfaces, nulls)?;
        }
        check_hulls(&self.hulls, points, planes, surfaces, nulls)?;
        for b in &self.coord_bins {
            range(
                "coord bin",
                b[0] as u64,
                b[1] as u64,
                self.coord_bin_indices.len(),
            )?;
        }
        for &h in &self.coord_bin_indices {
            index("coord bin hull", h as u64, self.hulls.hulls.len())?;
        }
        Ok(())
    }

    /// The triangles of the drawn surfaces: each surface's windings as a strip (`Interior`'s draw), degenerate triangles
    /// left out. Counted, not built.
    pub fn triangle_count(&self) -> u64 {
        self.surfaces
            .iter()
            .map(|s| (s.winding_count as u64).saturating_sub(2))
            .sum()
    }
}

fn check_hulls(
    h: &Hulls,
    points: usize,
    planes: usize,
    surfaces: usize,
    nulls: usize,
) -> Result<(), MapError> {
    for hull in &h.hulls {
        range(
            "hull points",
            hull.hull_start as u64,
            hull.hull_count as u64,
            h.hull_indices.len(),
        )?;
        range(
            "hull surfaces",
            hull.surface_start as u64,
            hull.surface_count as u64,
            h.hull_surface_indices.len(),
        )?;
        index(
            "hull plane start",
            hull.plane_start as u64,
            h.hull_plane_indices.len() + 1,
        )?;
        index(
            "hull poly list plane start",
            hull.poly_list_plane_start as u64,
            h.poly_list_planes.len() + 1,
        )?;
        index(
            "hull poly list point start",
            hull.poly_list_point_start as u64,
            h.poly_list_points.len() + 1,
        )?;
        index(
            "hull poly list string start",
            hull.poly_list_string_start as u64,
            h.poly_list_strings.len() + 1,
        )?;
    }
    for &p in &h.hull_indices {
        index("hull point", p as u64, points)?;
    }
    for &p in &h.hull_plane_indices {
        plane_index("hull plane", p, planes)?;
    }
    for &s in &h.hull_surface_indices {
        surface_ref("hull surface", s, surfaces, nulls)?;
    }
    for &e in &h.hull_emit_string_indices {
        index("hull emit string", e as u64, h.emit_strings.len())?;
    }
    for &p in &h.poly_list_planes {
        plane_index("poly list plane", p, planes)?;
    }
    for &p in &h.poly_list_points {
        index("poly list point", p as u64, points)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "dif_tests.rs"]
mod tests;
