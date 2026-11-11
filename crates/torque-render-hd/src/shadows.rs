//! The enhanced renderer's shadow maps: the sun's cascades and the point lights' cubes, each kept as a
//! **static** map (the static cells) and a **working** map (the static one plus the moving shapes: Players, vehicles, items), which is the
//! one the forward pass samples.
//!
//! - **Static cascades are cached.** A cascade's cells are drawn again only when its matrix changes (the camera moved far enough to
//!   move the snapped sphere, or the sun moved) or when static cells changed under it: then only the changed cells' texel rectangle is
//!   cleared (a far-depth triangle under a scissor) and the cells that meet it are drawn again. A still camera draws no cell into a
//!   cascade at all ([`ShadowStats::static_redraws`]).
//! - **The dynamic overlay**: a cascade (or a face) with shapes in it is copied from the static map and the shapes are drawn over
//!   the copy; one that had shapes last frame but none now is copied back clean. The overlay is kept while nothing it shows
//!   changed: the same caster, the same shapes with the same vertices and mesh places ([`ObjectCaster::content`]) and a static
//!   map not drawn again leave the working map as it is (a room of lamps around a standing figure copied and drew every face
//!   every frame: most of the frame's passes).
//! - **Point lights** (`assign_slots`): every light whose lit sphere meets the view gets a cube, at a resolution by its importance:
//!   the shadow maps are three cube arrays ([`point_tiers`]), the quality's face size for the few most important lights, half of
//!   it for the next, a quarter for the rest (the first tier's size, its count half the quality's point lights; then as many as the
//!   quality's; then 16), so a room of twenty lamps keeps every lamp's shadow within less memory than equal slots took. A
//!   light's importance is how much of the frame it may light: its brightness times the share of the view its sphere takes
//!   (radius over distance, squared, at most 4) times the share of the sphere its cone lights; a light that holds a cube counts
//!   [`SLOT_HOLD`] times as much, so a turn or a step does not trade tiers for one barely ahead. A light that changes tier
//!   crossfades from its old cube to its new one over [`SLOT_FADE_FRAMES`] frames (the forward pass blends the two lookups), a
//!   light that loses its cube while still in view fades its shadow out, one given a cube while it already lit the view fades
//!   in, and one that comes into view takes its cube whole at once; a light that leaves the view gives its cubes up at once (it
//!   lights nothing seen). A cube's six faces are drawn when it is given to a light and again only for the faces that changed
//!   cells meet. A spot that casts shadows takes a cube as a point light does, and only the faces its cone meets are drawn. The
//!   static geometry's owners a light is mounted in ([`crate::ext::HdExtension::light_hosts`]: a light that sits inside a solid)
//!   are left out of its own cube, else their faces hide it from everything around them.
//! - **Coloured shadows** (High and Ultra): beside each cascade a transmittance map at half its side, drawn again with its
//!   static cascade: the translucent batches of the cells it sees, each fragment nearer the sun than the static cascade's first
//!   opaque surface multiplying in what it lets through ((1 - a) + a x its linear paint). The forward pass multiplies the sun's light
//!   on opaque receivers by it (a receiver is the first opaque surface, so everything multiplied in lies between it and the sun).

use std::collections::BTreeMap;

use glam::Vec3;
use torque_render_core::wgpu;
use torque_render_scene::hd::shadow::{
    Cascade, MAX_CASCADES, MAX_POINT_SHADOWS, ShadowQuality, cascades, casts_into, cone_meets_face,
    cube_face, face_sees, face_sees_sphere, holds_shadow, light_basis, rect_union, rects_meet,
    shadow_range_within, texel_rect,
};
use torque_render_scene::hd::{HdCamera, HdLight, HdSun};

use crate::shaders::Permutation;
use crate::{GpuCell, push_f32, push_u32};

/// The shadow maps' format.
pub const SHADOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// The shadow uniform's bytes (`common.wgsl`'s `Shadows`).
pub const SHADOWS_BYTES: usize = 624;
/// The transmittance maps' format.
pub const TRANS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// One caster uniform's stride (the dynamic offset's alignment) and how many there are (the cascades, then six faces a slot).
const CASTER_STRIDE: u64 = 256;
/// A caster uniform's bytes (`shadow.wgsl`'s `Caster`: the matrix, the light, the owners left out, the extension's block).
const CASTER_BYTES: u64 = 112;
/// The nearest cascades that are also kept without the client's own Player (the first-person images are inside them).
const NO_SELF_CASCADES: usize = 1;
/// The most point-light cubes of all tiers ([`point_tiers`]).
pub const MAX_POINT_SLOTS: usize = 48;
const CASTERS: usize = MAX_CASCADES + 6 * MAX_POINT_SLOTS;
pub use crate::ext::NO_HOST;
/// The most dirty boxes kept; more invalidate every cached map.
pub const MAX_DIRTY: usize = 256;
/// A patch larger than this share of a cascade redraws the whole cascade.
const PATCH_SHARE: f64 = 0.5;

/// What the shadow passes did in a frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShadowStats {
    pub cascades: usize,
    /// Static cascades drawn whole, static cascades patched (only changed cells), static cube faces drawn.
    pub cascade_redraws: usize,
    pub cascade_patches: usize,
    /// Static cascades moved by whole texels (their cache scrolled, the strips it uncovered drawn as patches).
    pub cascade_scrolls: usize,
    pub face_redraws: usize,
    /// Point lights with a cube this frame.
    pub point_lights: usize,
    /// Dynamic overlays drawn (cascades and faces with shapes in them).
    pub overlays: usize,
    /// Cell draws into static maps.
    pub cell_draws: usize,
    /// The cell blocks the sun's range looked into (the rest were passed over as unable to change it).
    pub reach_blocks: usize,
}

impl ShadowStats {
    /// Every static map drawn (whole or patched).
    pub fn static_redraws(&self) -> usize {
        self.cascade_redraws + self.cascade_patches + self.face_redraws
    }
}

/// Which shadows a frame has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShadowSwitches {
    pub sun: bool,
    pub points: bool,
    pub contact: bool,
}

/// What a frame's shadow passes leave for the rest of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Encoded {
    /// A timed shadow pass was encoded (the frame's first; it wrote the frame's start).
    pub timed: bool,
    /// The contact shadows are on (their pass is to be drawn).
    pub contact: bool,
}

/// A moving shape that casts: its bounding sphere and its draws in the loose buffers (first index, count, base vertex).
pub(crate) struct ObjectCaster {
    pub ghost: u32,
    /// The client's own Player in its first-person view (its shadow-only body and held copy): the first-person images do not
    /// receive its shadow.
    pub own: bool,
    pub centre: [f32; 3],
    pub radius: f32,
    pub draws: Vec<(u32, u32, i32)>,
    /// What its shadow depends on besides the caster and `draws`: its vertices and its meshes' places (the renderer's object
    /// content).
    pub content: u64,
}

/// What a frame's shadows are made from.
pub(crate) struct ShadowInput<'a> {
    pub camera: &'a HdCamera,
    pub sun: &'a HdSun,
    pub lights: &'a [HdLight],
    /// Per light, its packed cone (`HdSpot::cone`; zeros: a point light): a spot's cube draws only the faces its cone meets.
    pub cones: &'a [[f32; 4]],
    /// Per light, the static geometry's owners it is mounted in (up to 4, the rest [`NO_HOST`]): left out of its cube.
    pub hosts: &'a [[u32; 4]],
    pub cells: &'a BTreeMap<[i32; 3], GpuCell>,
    /// The cells in blocks (the range's nearest and farthest seen cells).
    pub grid: &'a crate::cull::CellGrid,
    /// The extension's buffers in the casters' group 3 (bindings 0 and 1: [`crate::ExtSlots::shadow`]), and the frame's meshes'
    /// places (binding 2: the shapes' vertices in their meshes' own space).
    pub ext: &'a [wgpu::Buffer; 2],
    /// The extension's shadow generation ([`crate::ext::HdExtension::shadow_generation`]): what its casters' shader reads of
    /// its buffers changed when it changes (a kept overlay is drawn again).
    pub ext_generation: u64,
    pub nodes: &'a wgpu::Buffer,
    /// Each map's casters culled for it: on the GPU into the scratch lists before its pass when the frame's lists are the
    /// GPU's, else by the CPU (the same test, the same order).
    pub gpu: ShadowLists<'a>,
    /// The static arena's vertex and index buffers (the cells' slabs).
    pub arena: &'a crate::arena::Arena,
    pub loose: Option<(&'a wgpu::Buffer, &'a wgpu::Buffer)>,
    pub objects: Vec<ObjectCaster>,
    /// The boxes of the cells that changed since the last frame (old and new), and whether everything is to be drawn again.
    pub dirty: &'a [[[f32; 3]; 2]],
    pub reset: bool,
    pub switches: ShadowSwitches,
    pub debug: u32,
    /// The translucent cells colour the sun's shadows (the quality has it and the frame wants it).
    pub coloured: bool,
    /// The extension's extra casters, when the frame draws them (the fine cascades draw them too): the cells within the near
    /// detail's distance of the eye cast them.
    pub extras: Option<crate::ext::ShadowExtras<'a>>,
    /// The longest shadow the extra casters cast across a face (units). The extras' distance is where their geometry starts to
    /// fade: past it the faces are flat and casters would shadow them whole.
    pub caster_reach: f32,
    /// The farthest the cascades reach (the scene's `shadow_distance`).
    pub shadow_distance: f32,
    /// The boxes of the ground's draws (the scene's planes and water): they receive the sun's shadows, so the range reaches over
    /// them.
    pub ground: &'a [[[f32; 3]; 2]],
    /// The share of the ambient occlusion the sun's light takes (the scene's `ao_sun`).
    pub ao_sun: f32,
    /// The timestamp queries for the passes' start and end, when timed.
    pub stamps: Option<(&'a wgpu::QuerySet, u32, u32)>,
}

/// What a shadow map's casters are culled with: the GPU's cull (None: the CPU culls them) and its lists (one a map culled in a
/// batch), the tables, and how the moving shapes' overlays are drawn.
pub(crate) struct ShadowLists<'a> {
    pub device: &'a wgpu::Device,
    pub cull: Option<&'a crate::cull::Cull>,
    pub table: &'a crate::arena::Table,
    pub cells: &'a crate::cull::CellTable,
    pub lists: &'a [crate::arena::Lists],
    pub first_instance: bool,
    pub way: crate::arena::DrawWay,
    /// The static arena has at most [`crate::cull::MAX_PAGES`] pages: a depth-only map's GPU lists are one list a page
    /// ([`crate::cull::VIEW_WHOLE`]).
    pub whole: bool,
}

/// A map's casters: its view, and whether the GPU culled them into the lists (`list`: which of them).
#[derive(Clone, Copy)]
pub(crate) struct Culled {
    view: crate::cull::ViewPlanes,
    gpu: bool,
    list: usize,
}

impl ShadowLists<'_> {
    /// The casters of each view culled on the GPU into a list of its own, all in one compute pass (when the frame's lists are
    /// its, a view slot is left and a list: the first `lists.len()`), else kept for the CPU's test at the draw.
    fn cull_batch(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        views: &[crate::cull::ViewPlanes],
    ) -> Vec<Culled> {
        let mut batch: Vec<(u32, &crate::arena::Lists, bool)> = Vec::new();
        let out = views
            .iter()
            .map(|view| {
                let slot = self
                    .cull
                    .filter(|_| batch.len() < self.lists.len())
                    .and_then(|c| c.view(queue, view));
                match slot {
                    Some(slot) => {
                        let list = batch.len();
                        let whole = view.kinds & crate::cull::VIEW_WHOLE != 0;
                        batch.push((slot, &self.lists[list], whole));
                        Culled {
                            view: *view,
                            gpu: true,
                            list,
                        }
                    }
                    None => Culled {
                        view: *view,
                        gpu: false,
                        list: 0,
                    },
                }
            })
            .collect();
        if let Some(c) = self.cull {
            c.encode_batch(self.device, encoder, &batch, self.table, self.cells);
        }
        out
    }

    /// Draws the culled groups of one translucency (each group's arena page bound): the draws issued.
    fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        arena: &crate::arena::Arena,
        translucent: bool,
        culled: &Culled,
    ) -> usize {
        if culled.gpu
            && culled.view.kinds & crate::cull::VIEW_WHOLE != 0
            && let Some(c) = self.cull
        {
            let lists = &self.lists[culled.list];
            let mut calls = 0;
            for (page, &(first, len)) in c.page_lists().iter().enumerate() {
                if len == 0 || !arena.bind(pass, page as u32, &[0]) {
                    continue;
                }
                pass.multi_draw_indexed_indirect_count(
                    &lists.args,
                    u64::from(first) * 20,
                    &lists.counts,
                    page as u64 * 4,
                    len,
                );
                calls += 1;
            }
            crate::count_draws(calls);
            return calls;
        }
        let mut page: Option<u32> = None;
        let groups: Vec<crate::arena::GroupKey> = self
            .table
            .groups()
            .map(|(g, _)| *g)
            .filter(|g| g.0 == translucent)
            .collect();
        groups
            .into_iter()
            .map(|g| {
                if page != Some(g.2) {
                    arena.bind(pass, g.2, &[0]);
                    page = Some(g.2);
                }
                if culled.gpu {
                    self.lists[culled.list].draw_group(
                        pass,
                        self.table,
                        g,
                        crate::arena::DrawWay::Count,
                    )
                } else {
                    crate::arena::Lists::draw_group_direct(
                        pass,
                        self.table,
                        g,
                        self.first_instance,
                        |r| crate::cull::cpu_pass(&culled.view, self.cells, r),
                    )
                }
            })
            .sum()
    }
}

/// Off: a static cascade that moved is drawn whole, never scrolled (the checks' reference for the scrolled cache).
pub static SCROLL_CACHE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// A patch's texel rectangle of a map of `size` texels as a clip-space rectangle (x0, y0, x1, y1), `margin` texels wider.
fn patch_clip(r: [u32; 4], size: u32, margin: f32) -> [f32; 4] {
    let s = size as f32;
    let x = |t: f32| 2.0 * t / s - 1.0;
    let y = |t: f32| 1.0 - 2.0 * t / s;
    [
        x(r[0] as f32 - margin),
        y(r[3] as f32 + margin),
        x(r[2] as f32 + margin),
        y(r[1] as f32 - margin),
    ]
}

/// How much more a light that holds a cube counts when the lights are ranked for the tiers.
pub const SLOT_HOLD: f32 = 1.25;
/// The frames a shadow takes to fade in or out, or to crossfade from one tier's cube to another's.
pub const SLOT_FADE_FRAMES: u32 = 12;

/// The point-light cube tiers of `q`: each tier's face size and its number of cubes (the module docs); none without point
/// shadows.
pub fn point_tiers(q: &ShadowQuality) -> [(u32, usize); 3] {
    let n = q.point_lights.min(MAX_POINT_SHADOWS);
    if n == 0 {
        return [(0, 0); 3];
    }
    let s = q.point_size.max(64);
    [((s), (n / 2).max(1)), (s / 2, n), (s / 4, 16)]
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Slot {
    pub light: Option<[u32; 4]>,
    valid: [bool; 6],
    /// What each working face holds ([`Overlay`]).
    overlay: [Overlay; 6],
    /// How much of the light's shadow this cube gives (0 to 1): below 1 while it fades in or out.
    pub strength: f32,
    /// The cube its light is leaving (fading out, or crossfading to a cube of another tier).
    pub leaving: bool,
}

/// Gives this frame's cubes (module docs). `tiers` holds where each tier's slots end (the slots run tier by tier); `ranked` is
/// every light in view (its key, its importance); `seen_last` the lights that were in view the frame before.
pub(crate) fn assign_slots(
    slots: &mut [Slot],
    tiers: &[usize],
    ranked: &[([u32; 4], f32)],
    seen_last: &[[u32; 4]],
) {
    let step = 1.0 / SLOT_FADE_FRAMES as f32;
    let tier_of = |slot: usize| {
        tiers
            .iter()
            .position(|&end| slot < end)
            .unwrap_or(tiers.len())
    };
    let range = |t: usize| (if t == 0 { 0 } else { tiers[t - 1] })..tiers[t];
    // A light out of view lights nothing seen: its cubes go at once.
    for s in slots.iter_mut() {
        if s.light.is_some_and(|k| !ranked.iter().any(|r| r.0 == k)) {
            *s = Slot::default();
        }
    }
    let current =
        |slots: &[Slot], k: [u32; 4]| slots.iter().position(|s| s.light == Some(k) && !s.leaving);
    let old =
        |slots: &[Slot], k: [u32; 4]| slots.iter().position(|s| s.light == Some(k) && s.leaving);
    let mut order: Vec<(f32, usize)> = ranked
        .iter()
        .enumerate()
        .map(|(i, &(k, score))| {
            let held = current(slots, k).is_some();
            (if held { score * SLOT_HOLD } else { score }, i)
        })
        .collect();
    order.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    // Each light's tier by its rank: the first tier's count first, then the next.
    let mut room: Vec<usize> = (0..tiers.len()).map(|t| range(t).len()).collect();
    let targets: Vec<(usize, Option<usize>)> = order
        .iter()
        .map(|&(_, i)| {
            let t = room.iter().position(|&r| r > 0);
            if let Some(t) = t {
                room[t] -= 1;
            }
            (i, t)
        })
        .collect();
    // The tiers to look in for a free cube: the wanted one, then the others nearest it.
    let near_tiers = |t: usize| {
        let mut v: Vec<usize> = (0..tiers.len()).collect();
        v.sort_by_key(|&o| (o.abs_diff(t), o < t));
        v
    };
    let free_in = |slots: &[Slot], t: usize| range(t).find(|&f| slots[f].light.is_none());
    // First every light in view without a cube takes one (its own tier if it has room, else the nearest with room), so a light
    // in view is never left unshadowed while cubes are free; then the lights whose rank moved them change tiers.
    for &(i, target) in &targets {
        let k = ranked[i].0;
        let cur = current(slots, k);
        match (cur, target) {
            (Some(c), None) => slots[c].leaving = true,
            (Some(c), Some(_)) => slots[c].strength = (slots[c].strength + step).min(1.0),
            (None, Some(t)) => {
                let Some(f) = near_tiers(t).into_iter().find_map(|o| free_in(slots, o)) else {
                    continue;
                };
                let fading = old(slots, k).is_some();
                slots[f] = Slot {
                    light: Some(k),
                    strength: if fading || seen_last.contains(&k) {
                        step
                    } else {
                        1.0
                    },
                    ..Slot::default()
                };
            }
            (None, None) => {}
        }
    }
    for &(i, target) in &targets {
        let k = ranked[i].0;
        let (Some(c), Some(t)) = (current(slots, k), target) else {
            continue;
        };
        // A crossfade still running finishes before the next change of tier.
        if tier_of(c) == t || old(slots, k).is_some() {
            continue;
        }
        if let Some(f) = free_in(slots, t) {
            slots[f] = Slot {
                light: Some(k),
                strength: step,
                ..Slot::default()
            };
            slots[c].leaving = true;
        }
    }
    // A cube being left gives way to its light's new cube (the two sum to one) or fades out alone; at 0 it is free.
    for i in 0..slots.len() {
        let Some(k) = slots[i].light.filter(|_| slots[i].leaving) else {
            continue;
        };
        let strength = match current(slots, k) {
            Some(n) => slots[i].strength.min(1.0 - slots[n].strength),
            None => slots[i].strength - step,
        };
        if strength <= 1e-4 {
            slots[i] = Slot::default();
        } else {
            slots[i].strength = strength;
        }
    }
}

enum Job {
    /// A static cascade drawn whole, or only under a texel rectangle.
    Cascade {
        layer: usize,
        patch: Option<[u32; 4]>,
    },
    /// A static cascade's map copied into the work map `to` (its own layer, or the one beyond the cascades).
    CopyCascade {
        layer: usize,
        to: usize,
    },
    CascadeOverlay {
        layer: usize,
        to: usize,
        objects: Vec<usize>,
    },
    Face {
        slot: usize,
        face: usize,
        light: usize,
    },
    CopyFace {
        slot: usize,
        face: usize,
    },
    /// a cascade's transmittance map drawn again (after its static cascade).
    Transmit {
        layer: usize,
    },
    FaceOverlay {
        slot: usize,
        face: usize,
        objects: Vec<usize>,
    },
    /// a static cascade's cache moved by whole texels (x, y; before the patches of the strips it uncovers).
    Scroll {
        layer: usize,
        shift: [i32; 2],
    },
}

/// A static cascade's move from matrix `old` to `new` as a shift of whole texels of a map of `size` (the new map's texel
/// (u, v) shows what the old one's (u - x, v - y) did), when the two differ only in the square's place across the light and by
/// less than the map.
pub(crate) fn scroll_of(old: &[f32; 16], new: &[f32; 16], size: u32) -> Option<[i32; 2]> {
    // Column-major: [12] and [13] are the x and y rows' translations; everything else (the depth's row included) unchanged.
    if (0..16).any(|k| k != 12 && k != 13 && old[k].to_bits() != new[k].to_bits()) {
        return None;
    }
    let half = f64::from(size) / 2.0;
    let sx = f64::from(new[12] - old[12]) * half;
    let sy = -f64::from(new[13] - old[13]) * half;
    let (rx, ry) = (sx.round(), sy.round());
    let whole = (sx - rx).abs() < 1e-3 && (sy - ry).abs() < 1e-3;
    let near = rx.abs() < f64::from(size) && ry.abs() < f64::from(size);
    (whole && near && (rx != 0.0 || ry != 0.0)).then_some([rx as i32, ry as i32])
}

/// The texel rectangles a scroll of `shift` uncovers on a map of `size` (a strip across, a strip down).
pub(crate) fn uncovered(shift: [i32; 2], size: u32) -> Vec<[u32; 4]> {
    let s = size as i32;
    let mut out = Vec::new();
    match shift[0] {
        x if x > 0 => out.push([0, 0, x as u32, size]),
        x if x < 0 => out.push([(s + x) as u32, 0, size, size]),
        _ => {}
    }
    match shift[1] {
        y if y > 0 => out.push([0, 0, size, y as u32]),
        y if y < 0 => out.push([0, (s + y) as u32, size, size]),
        _ => {}
    }
    out
}

impl Job {
    fn is_pass(&self) -> bool {
        !matches!(self, Job::CopyCascade { .. } | Job::CopyFace { .. })
    }
}

/// One tier of point-light cubes: their face size, their static and working cube arrays (six layers a cube), each layer's view and
/// the working array's cube view (the forward pass's).
struct PointTier {
    size: u32,
    count: usize,
    stat: wgpu::Texture,
    work: wgpu::Texture,
    static_layers: Vec<wgpu::TextureView>,
    work_layers: Vec<wgpu::TextureView>,
    view: wgpu::TextureView,
}

struct Maps {
    quality: ShadowQuality,
    /// The static cascades as one array (the transmittance's depth test reads it).
    csm_static_view: wgpu::TextureView,
    csm_static: wgpu::Texture,
    csm_work: wgpu::Texture,
    csm_static_layers: Vec<wgpu::TextureView>,
    csm_work_layers: Vec<wgpu::TextureView>,
    csm_view: wgpu::TextureView,
    /// The point-light cubes, tier by tier ([`point_tiers`]).
    points: Vec<PointTier>,
}

/// The shadow maps, their caches and passes.
pub(crate) struct Shadows {
    maps: Maps,
    casters: wgpu::Buffer,
    /// The last frame's casters in bytes, up to the last one used (its room kept).
    caster_bytes: Vec<u8>,
    caster_group: wgpu::BindGroup,
    sun_pipeline: wgpu::RenderPipeline,
    point_pipeline: wgpu::RenderPipeline,
    clear_pipeline: wgpu::RenderPipeline,
    /// The scroll of a static cascade's cache: its pipeline, its group's layout, a uniform slot a cascade.
    scroll_pipeline: wgpu::RenderPipeline,
    scroll_layout: wgpu::BindGroupLayout,
    /// The casters' group 3 layout (the extension's buffers, the meshes' places), and the group over the buffers last bound
    /// (made again only when one of them is another buffer).
    ext_layout: wgpu::BindGroupLayout,
    ext_group: Option<([wgpu::Buffer; 3], wgpu::BindGroup)>,
    scroll_uniform: wgpu::Buffer,
    pub uniform: wgpu::Buffer,
    pub cmp_sampler: wgpu::Sampler,
    pub gather_sampler: wgpu::Sampler,
    cascade_keys: Vec<Option<[f32; 16]>>,
    /// What each working cascade holds, and each cascade kept without the client's own Player ([`Overlay`]).
    cascade_overlay: Vec<Overlay>,
    cascade_self_overlay: Vec<Overlay>,
    /// Whether a working map is kept while its overlay is unchanged (on; off: copied and drawn over every frame, the check's
    /// reference: [`Shadows::set_keep_overlays`]).
    keep_overlays: bool,
    /// Whether passes from outside the renderer draw into the cascades' work maps this frame ([`Shadows::set_drawn_over`]):
    /// each is copied from its cache every frame.
    drawn_over: bool,
    /// The sun's cascades of the last frame encoded: their light's view and projection.
    cascade_matrices: Vec<[f32; 16]>,
    /// The static cascades whose cells changed too much to patch, waiting for their whole redraw since that frame
    /// ([`Shadows::frames`]): at most one is drawn whole for its cells a frame, the longest waiting first (a load's stream of
    /// cells redrew all four every frame, some 30 ms of the GPU's).
    cascade_stale: Vec<Option<u64>>,
    frames: u64,
    slots: Vec<Slot>,
    /// The lights in view the frame before (their keys): a light given a slot fades its shadow in only if it was.
    seen_last: Vec<[u32; 4]>,
    /// Each light's cubes this frame as the lights' buffer holds them: its cube and that cube's strength, the cube it is leaving
    /// and that one's (-1 for none).
    pub light_slots: Vec<[f32; 4]>,
    pub stats: ShadowStats,
    /// The sun's shadow range the last frame used (None before the first).
    pub range: Option<torque_render_scene::hd::shadow::ShadowRange>,
    /// Bumped when the maps are made again (the frame's bind group must be made again).
    pub maps_generation: u64,
    /// The transmittance maps (coloured shadows), their pipeline and the static cascades' group they read, a white stand-in
    /// for when they are off, the forward pass's sampler, and which layers hold their cascade's present cells.
    trans: Option<TransMaps>,
    trans_dummy: wgpu::TextureView,
    trans_layout: wgpu::BindGroupLayout,
    trans_pipeline: wgpu::RenderPipeline,
    trans_group: wgpu::BindGroup,
    pub trans_sampler: wgpu::Sampler,
    trans_valid: Vec<bool>,
    /// The extension's extra casters' pipeline, and whether the static cascades hold them.
    extra_pipeline: Option<wgpu::RenderPipeline>,
    cascades_hold_extras: bool,
    /// The moving shapes' overlay draws of the frame (indirect arguments, grown as needed).
    overlay_args: Option<wgpu::Buffer>,
}

/// The cascades' transmittance maps: half a cascade's side, a layer a cascade.
struct TransMaps {
    size: u32,
    layers: Vec<wgpu::TextureView>,
    view: wgpu::TextureView,
}

impl TransMaps {
    fn new(device: &wgpu::Device, q: ShadowQuality) -> TransMaps {
        let layers = q.cascades.clamp(1, MAX_CASCADES) as u32;
        let size = (q.cascade_size / 2).max(1);
        let t = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hd transmittance"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TRANS_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        TransMaps {
            size,
            layers: layer_views(&t, layers),
            view: t.create_view(&wgpu::TextureViewDescriptor {
                label: Some("hd transmittance array"),
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..wgpu::TextureViewDescriptor::default()
            }),
        }
    }

    fn bytes(&self) -> u64 {
        u64::from(self.size).pow(2) * self.layers.len() as u64 * 4
    }
}

/// The static cascades' group the transmittance pass reads.
fn trans_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    maps: &Maps,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("hd transmittance depth"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(&maps.csm_static_view),
        }],
    })
}

fn layer_views(t: &wgpu::Texture, layers: u32) -> Vec<wgpu::TextureView> {
    (0..layers)
        .map(|i| {
            t.create_view(&wgpu::TextureViewDescriptor {
                label: Some("hd shadow layer"),
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: i,
                array_layer_count: Some(1),
                ..wgpu::TextureViewDescriptor::default()
            })
        })
        .collect()
}

impl Maps {
    fn new(device: &wgpu::Device, q: ShadowQuality) -> Maps {
        let tex = |label, size: u32, layers: u32, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: layers,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: SHADOW_FORMAT,
                usage,
                view_formats: &[],
            })
        };
        let st = wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::TEXTURE_BINDING;
        let wk = wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::TEXTURE_BINDING;
        let layers = q.cascades.clamp(1, MAX_CASCADES) as u32;
        let csm_static = tex("hd csm static", q.cascade_size, layers, st);
        // Layers beyond the cascades: the nearest ones drawn without the client's own Player (the first-person images' lookups).
        let csm_work = tex(
            "hd csm",
            q.cascade_size,
            layers + NO_SELF_CASCADES as u32,
            wk,
        );
        let points = point_tiers(&q)
            .iter()
            .map(|&(size, count)| {
                // Without point shadows, one small cube a tier for the forward pass's bindings.
                let (size, count) = if count == 0 { (16, 1) } else { (size, count) };
                let layers = 6 * count as u32;
                let stat = tex("hd point static", size, layers, st);
                let work = tex("hd point", size, layers, wk);
                PointTier {
                    size,
                    count,
                    static_layers: layer_views(&stat, layers),
                    work_layers: layer_views(&work, layers),
                    view: work.create_view(&wgpu::TextureViewDescriptor {
                        label: Some("hd point cubes"),
                        dimension: Some(wgpu::TextureViewDimension::CubeArray),
                        array_layer_count: Some(layers),
                        ..wgpu::TextureViewDescriptor::default()
                    }),
                    stat,
                    work,
                }
            })
            .collect();
        Maps {
            quality: q,
            csm_static_layers: layer_views(&csm_static, layers),
            csm_static_view: csm_static.create_view(&wgpu::TextureViewDescriptor {
                label: Some("hd csm static array"),
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..wgpu::TextureViewDescriptor::default()
            }),
            csm_work_layers: layer_views(&csm_work, layers + NO_SELF_CASCADES as u32),
            csm_view: csm_work.create_view(&wgpu::TextureViewDescriptor {
                label: Some("hd csm array"),
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..wgpu::TextureViewDescriptor::default()
            }),
            csm_static,
            csm_work,
            points,
        }
    }

    /// Where each tier's slots end (the slots run tier by tier).
    fn tier_ends(&self) -> Vec<usize> {
        point_tiers(&self.quality)
            .iter()
            .scan(0, |end, &(_, count)| {
                *end += count;
                Some(*end)
            })
            .collect()
    }

    fn slots(&self) -> usize {
        self.tier_ends().last().copied().unwrap_or(0)
    }

    /// Slot `slot`'s tier and its first layer there.
    fn point_layer(&self, slot: usize) -> (&PointTier, usize) {
        let mut at = slot;
        for t in &self.points {
            if at < t.count {
                return (t, at * 6);
            }
            at -= t.count;
        }
        (&self.points[0], 0)
    }

    fn bytes(&self) -> u64 {
        let c = u64::from(self.quality.cascade_size).pow(2) * self.csm_static_layers.len() as u64;
        let p: u64 = self
            .points
            .iter()
            .map(|t| u64::from(t.size).pow(2) * t.static_layers.len() as u64)
            .sum();
        2 * 4 * (c + p)
    }
}

/// A depth-only pipeline's choices: its entry points, whether it reads the cells' vertices, depth clamping, the test and the slope
/// bias.
struct DepthPipe<'a> {
    label: &'a str,
    vs: &'a str,
    fs: Option<&'a str>,
    with_vertices: bool,
    unclipped: bool,
    compare: wgpu::CompareFunction,
    slope_bias: f32,
}

fn depth_pipeline(
    device: &wgpu::Device,
    cache: Option<&wgpu::PipelineCache>,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    p: DepthPipe<'_>,
) -> wgpu::RenderPipeline {
    let DepthPipe {
        label,
        vs,
        fs,
        with_vertices,
        unclipped,
        compare,
        slope_bias,
    } = p;
    let buffers = [Some(wgpu::VertexBufferLayout {
        array_stride: torque_render_scene::hd::HD_VERTEX_BYTES as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        // The position, the material word and the extra word (a point light's own hosts are left out of its cube; a
        // shape's vertex in its mesh's own space names its mesh's place there).
        attributes: &[
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Uint32,
                offset: 28,
                shader_location: 4,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Uint32,
                offset: 32,
                shader_location: 5,
            },
        ],
    })];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some(vs),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: if with_vertices { &buffers } else { &[] },
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            unclipped_depth: unclipped,
            polygon_mode: wgpu::PolygonMode::Fill,
            conservative: false,
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: SHADOW_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(compare),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState {
                constant: 0,
                slope_scale: slope_bias,
                clamp: 0.0,
            },
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: fs.map(|entry| wgpu::FragmentState {
            module,
            entry_point: Some(entry),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[],
        }),
        multiview_mask: None,
        cache,
    })
}

/// Whether a light's face is drawn: every face of a point light's cube, a spot's only where its cone (packed: the direction over
/// the soft edge's width, the outer cosine) meets it.
fn cone_face(cone: [f32; 4], face: usize) -> bool {
    let d = [cone[0], cone[1], cone[2]];
    d == [0.0; 3] || cone_meets_face(d, cone[3].clamp(-1.0, 1.0).acos(), face)
}

/// The bits a light is known by (its position and radius).
fn light_key(l: &HdLight) -> [u32; 4] {
    [
        l.position[0].to_bits(),
        l.position[1].to_bits(),
        l.position[2].to_bits(),
        l.radius.to_bits(),
    ]
}

impl Shadows {
    /// The shadows at `quality`; `extra`: the extension's extra casters' vertex entry and buffers.
    pub fn new(
        device: &wgpu::Device,
        cache: Option<&wgpu::PipelineCache>,
        quality: ShadowQuality,
        extra: Option<(&str, Vec<wgpu::VertexBufferLayout<'_>>)>,
    ) -> Shadows {
        let unclipped = device
            .features()
            .contains(wgpu::Features::DEPTH_CLIP_CONTROL);
        let caster_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd caster"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(CASTER_BYTES),
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hd caster"),
            bind_group_layouts: &[Some(&caster_layout)],
            immediate_size: 0,
        });
        let casters = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hd casters"),
            size: CASTER_STRIDE * CASTERS as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let caster_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd caster"),
            layout: &caster_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &casters,
                    offset: 0,
                    size: wgpu::BufferSize::new(CASTER_BYTES),
                }),
            }],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hd shadow"),
            source: wgpu::ShaderSource::Wgsl(Permutation::Shadow { unclipped }.source().into()),
        });
        use wgpu::CompareFunction as C;
        let ext_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd shadow extension"),
            entries: &[0, 1, 2].map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }),
        });
        let point_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hd point caster"),
            bind_group_layouts: &[Some(&caster_layout), None, None, Some(&ext_layout)],
            immediate_size: 0,
        });
        // (The sun's casters read the meshes' places from group 3 as well.)
        let sun_pipeline = depth_pipeline(
            device,
            cache,
            &point_layout,
            &module,
            DepthPipe {
                label: "hd sun caster",
                vs: "vs_sun",
                fs: None,
                with_vertices: true,
                unclipped,
                compare: C::LessEqual,
                slope_bias: 1.0,
            },
        );
        let point_pipeline = depth_pipeline(
            device,
            cache,
            &point_layout,
            &module,
            DepthPipe {
                label: "hd point caster",
                vs: "vs_point",
                fs: Some("fs_point"),
                with_vertices: true,
                unclipped: false,
                compare: C::Less,
                slope_bias: 0.0,
            },
        );
        let clear_pipeline = depth_pipeline(
            device,
            cache,
            &layout,
            &module,
            DepthPipe {
                label: "hd shadow clear",
                vs: "vs_clear",
                fs: None,
                with_vertices: false,
                unclipped: false,
                compare: C::Always,
                slope_bias: 0.0,
            },
        );
        // The scroll: a full-frame pass from the cascades' copy.
        let scroll_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd shadow scroll"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(16),
                    },
                    count: None,
                },
            ],
        });
        let scroll_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("hd shadow scroll"),
                bind_group_layouts: &[None, None, Some(&scroll_layout)],
                immediate_size: 0,
            });
        let scroll_pipeline = depth_pipeline(
            device,
            cache,
            &scroll_pipeline_layout,
            &module,
            DepthPipe {
                label: "hd shadow scroll",
                vs: "vs_clear",
                fs: Some("fs_scroll"),
                with_vertices: false,
                unclipped: false,
                compare: C::Always,
                slope_bias: 0.0,
            },
        );
        let scroll_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hd shadow scroll"),
            size: SCROLL_STRIDE * MAX_CASCADES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hd shadows"),
            size: SHADOWS_BYTES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let clamp = wgpu::AddressMode::ClampToEdge;
        let cmp_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hd shadow compare"),
            address_mode_u: clamp,
            address_mode_v: clamp,
            address_mode_w: clamp,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..wgpu::SamplerDescriptor::default()
        });
        let gather_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hd shadow gather"),
            address_mode_u: clamp,
            address_mode_v: clamp,
            address_mode_w: clamp,
            ..wgpu::SamplerDescriptor::default()
        });
        let maps = Maps::new(device, quality);
        // The transmittance maps' pass: the casters' group and the static cascades.
        let trans_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd transmittance depth"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let trans_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("hd transmittance"),
                bind_group_layouts: &[Some(&caster_layout), Some(&trans_layout)],
                immediate_size: 0,
            });
        let trans_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hd transmittance"),
            layout: Some(&trans_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_trans"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: torque_render_scene::hd::HD_VERTEX_BYTES as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Unorm8x4,
                            offset: 24,
                            shader_location: 3,
                        },
                    ],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: unclipped,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_trans"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: TRANS_FORMAT,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::Zero,
                            dst_factor: wgpu::BlendFactor::Src,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent::REPLACE,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache,
        });
        let trans_group = trans_group(device, &trans_layout, &maps);
        // The extension's extra casters into the sun's cascades.
        let extra_pipeline = extra.map(|(entry, buffers)| {
            let buffers: Vec<Option<wgpu::VertexBufferLayout<'_>>> =
                buffers.into_iter().map(Some).collect();
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("hd extra caster"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &buffers,
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    unclipped_depth: unclipped,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: SHADOW_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState {
                        constant: 0,
                        slope_scale: 1.0,
                        clamp: 0.0,
                    },
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: None,
                multiview_mask: None,
                cache,
            })
        });
        let trans_dummy = {
            let t = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("hd transmittance off"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: TRANS_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            t.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..wgpu::TextureViewDescriptor::default()
            })
        };
        let trans_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hd transmittance"),
            address_mode_u: clamp,
            address_mode_v: clamp,
            address_mode_w: clamp,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        Shadows {
            trans: None,
            extra_pipeline,
            cascades_hold_extras: false,
            overlay_args: None,
            trans_dummy,
            trans_layout,
            trans_pipeline,
            trans_group,
            trans_sampler,
            trans_valid: vec![false; MAX_CASCADES],
            cascade_keys: vec![None; MAX_CASCADES],
            cascade_overlay: vec![Overlay::Unknown; MAX_CASCADES],
            cascade_self_overlay: vec![Overlay::Unknown; MAX_CASCADES],
            keep_overlays: true,
            drawn_over: false,
            cascade_matrices: Vec::new(),
            cascade_stale: vec![None; MAX_CASCADES],
            frames: 0,
            slots: vec![Slot::default(); maps.slots()],
            maps,
            casters,
            caster_bytes: Vec::new(),
            caster_group,
            sun_pipeline,
            point_pipeline,
            clear_pipeline,
            scroll_pipeline,
            scroll_layout,
            ext_layout,
            ext_group: None,
            scroll_uniform,
            uniform,
            cmp_sampler,
            gather_sampler,
            light_slots: Vec::new(),
            seen_last: Vec::new(),
            stats: ShadowStats::default(),
            range: None,
            maps_generation: 0,
        }
    }

    /// The maps for `quality` (made again, every cache dropped, when it changed), with the transmittance maps when `coloured`.
    pub fn ensure(&mut self, device: &wgpu::Device, quality: ShadowQuality, coloured: bool) {
        if self.maps.quality != quality {
            self.maps = Maps::new(device, quality);
            self.cascade_keys = vec![None; MAX_CASCADES];
            self.cascade_overlay = vec![Overlay::Unknown; MAX_CASCADES];
            self.cascade_self_overlay = vec![Overlay::Unknown; MAX_CASCADES];
            self.cascade_stale = vec![None; MAX_CASCADES];
            self.slots = vec![Slot::default(); self.maps.slots()];
            self.seen_last.clear();
            self.maps_generation += 1;
            self.trans_group = trans_group(device, &self.trans_layout, &self.maps);
            self.trans = None;
        }
        if coloured != self.trans.is_some() {
            self.trans = coloured.then(|| TransMaps::new(device, quality));
            self.trans_valid = vec![false; MAX_CASCADES];
            self.maps_generation += 1;
        }
    }

    /// The transmittance maps, or a white stand-in.
    pub fn trans_view(&self) -> &wgpu::TextureView {
        self.trans.as_ref().map_or(&self.trans_dummy, |t| &t.view)
    }

    pub fn csm_view(&self) -> &wgpu::TextureView {
        &self.maps.csm_view
    }

    /// Whether passes from outside the renderer draw into the cascades after [`Shadows::encode`] (each cascade's work map is then
    /// copied from its cache every frame, so what they drew the frame before is gone).
    pub fn set_drawn_over(&mut self, on: bool) {
        self.drawn_over = on;
    }

    /// Whether a working map is kept while its overlay is unchanged (on by default). Off, every map with shapes in it is copied
    /// from its static map and drawn over each frame: the reference the kept maps are checked against.
    pub fn set_keep_overlays(&mut self, on: bool) {
        self.keep_overlays = on;
    }

    /// The sun's cascades the last [`Shadows::encode`] drew, finest first: each one's light view and projection, its work map
    /// (what the frame reads) and its side in texels.
    pub fn cascades(&self) -> impl Iterator<Item = ([f32; 16], &wgpu::TextureView, u32)> + '_ {
        let size = self.maps.quality.cascade_size;
        self.cascade_matrices
            .iter()
            .zip(&self.maps.csm_work_layers)
            .map(move |(m, v)| (*m, v, size))
    }

    /// A count that changes whenever the maps are made again.
    pub fn maps_made(&self) -> u64 {
        self.maps_generation
    }

    /// Tier `tier`'s cube array (the forward pass's).
    pub fn point_view(&self, tier: usize) -> &wgpu::TextureView {
        &self.maps.points[tier].view
    }

    /// The shadow maps' GPU memory (bytes).
    pub fn bytes(&self) -> u64 {
        self.maps.bytes() + self.trans.as_ref().map_or(0, TransMaps::bytes)
    }

    fn invalidate_cascades(&mut self) {
        self.cascade_keys.iter_mut().for_each(|k| *k = None);
        self.trans_valid.iter_mut().for_each(|v| *v = false);
    }

    fn invalidate_slots(&mut self) {
        self.slots.iter_mut().for_each(|s| *s = Slot::default());
        self.seen_last.clear();
    }

    /// The point lights in view (their spheres meet the view), each with its importance (module docs).
    fn lights_in_view(
        lights: &[HdLight],
        cones: &[[f32; 4]],
        camera: &HdCamera,
    ) -> Vec<(f32, usize)> {
        let planes = camera.planes();
        let e = camera.eye;
        lights
            .iter()
            .enumerate()
            .filter(|(_, l)| {
                l.radius > 0.0
                    && l.color.iter().any(|c| *c > 0.0)
                    && crate::sphere_seen(&planes, l.position, l.radius)
            })
            .map(|(i, l)| {
                let dist = Vec3::from(l.position).distance(e.into()).max(1e-3);
                let lum = 0.2126 * l.color[0] + 0.7152 * l.color[1] + 0.0722 * l.color[2];
                let screen = (l.radius / dist).powi(2).min(4.0);
                // A spot lights the share of its sphere inside its cone.
                let cone = cones.get(i).copied().unwrap_or([0.0; 4]);
                let lit = if cone[..3] == [0.0; 3] {
                    1.0
                } else {
                    ((1.0 - cone[3].clamp(-1.0, 1.0)) * 0.5).max(0.05)
                };
                (lum * screen * lit, i)
            })
            .collect()
    }

    /// Plans and encodes this frame's shadow passes (before everything else of the frame) and writes the shadow uniform: whether a
    /// timed pass was encoded and whether the contact shadows are on.
    pub fn encode(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        input: &ShadowInput<'_>,
        sun_radius_deg: f32,
    ) -> Encoded {
        let q = self.maps.quality;
        let mut stats = ShadowStats::default();
        let mut jobs: Vec<Job> = Vec::new();
        // Written up to the last caster used (the slots past it are not read this frame), zeros between.
        let mut caster_bytes = std::mem::take(&mut self.caster_bytes);
        caster_bytes.clear();
        let mut b = Vec::with_capacity(CASTER_BYTES as usize);
        // A cascade's extra casters cast only within `near` (the eye, the distance their geometry is drawn whole to).
        let near = {
            let e = input.camera.eye;
            [
                e[0],
                e[1],
                e[2],
                input.extras.as_ref().map_or(0.0, |x| x.reach),
            ]
        };
        // Returns the caster's bytes hashed (an overlay's key).
        let mut put_caster = |index: usize, m: &[f32; 16], light: [f32; 4], exclude: [u32; 4]| {
            b.clear();
            push_f32(&mut b, m);
            push_f32(&mut b, &light);
            crate::push_u32(&mut b, &exclude);
            push_f32(&mut b, &near);
            let at = index * CASTER_STRIDE as usize;
            if caster_bytes.len() < at + CASTER_STRIDE as usize {
                caster_bytes.resize(at + CASTER_STRIDE as usize, 0);
            }
            caster_bytes[at..at + b.len()].copy_from_slice(&b);
            let mut h = std::hash::DefaultHasher::new();
            std::hash::Hash::hash(&b, &mut h);
            std::hash::Hasher::finish(&h)
        };
        // The extra casters going on or off changes what the static cascades hold.
        if input.extras.is_some() != self.cascades_hold_extras {
            self.invalidate_cascades();
            self.cascades_hold_extras = input.extras.is_some();
        }
        self.frames += 1;
        if input.reset {
            // (The cascades wait their turn for their whole redraw; the cubes are drawn again.)
            for s in &mut self.cascade_stale {
                s.get_or_insert(self.frames);
            }
            self.invalidate_slots();
        }
        // The one cascade drawn whole for its cells this frame: the longest waiting (else the first that comes to need it).
        let mut whole_turn: Option<usize> = self
            .cascade_stale
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.map(|f| (f, i)))
            .min()
            .map(|(_, i)| i);
        let mut whole_spent = false;
        let sun_on = input.switches.sun
            && input.sun.towards[2] > -0.2
            && input.sun.diffuse.iter().any(|c| *c > 0.0);
        // The nearest and farthest of the geometry about the eye in every direction (not only what the camera sees: the shadows reach
        // as far whichever way it turns): the cells' from their blocks, the shapes', and the ground's (the terrain and the water
        // receive the shadows but are not cells). The shapes' and the ground's first: the cells' walk then looks only at the blocks
        // that could still change the range (with a ground under the eye, almost none).
        let cam = input.camera;
        let others_reach = torque_render_scene::hd::shadow::reach_around(
            cam.eye,
            input
                .objects
                .iter()
                .map(|o| {
                    let (c, r) = (o.centre, o.radius);
                    [
                        [c[0] - r, c[1] - r, c[2] - r],
                        [c[0] + r, c[1] + r, c[2] + r],
                    ]
                })
                .chain(input.ground.iter().copied()),
        );
        let bounds = torque_render_scene::hd::shadow::reach_bounds(
            cam,
            others_reach.map_or(f32::INFINITY, |r| r.0),
            input.shadow_distance,
        );
        let (reach, visited) = input.grid.reach(cam.eye, others_reach, bounds);
        stats.reach_blocks = visited;
        let range = shadow_range_within(cam, reach, input.shadow_distance);
        let layers = self.maps.quality.cascades.clamp(1, MAX_CASCADES);
        let mut no_self = 0usize;
        let cs = if sun_on {
            cascades(input.camera, input.sun.towards, &q, &range)
        } else {
            self.invalidate_cascades();
            Vec::new()
        };
        stats.cascades = cs.len();
        self.cascade_matrices = cs.iter().map(|c| c.view_proj).collect();
        self.range = Some(range);
        let size = q.cascade_size;
        for (i, c) in cs.iter().enumerate() {
            // The cascade's layer rides in the exclusion's first word (the transmittance pass's depth test reads it).
            // The light word carries the sun's direction and the extra casters' shadow reach.
            let t = input.sun.towards;
            let caster = put_caster(
                i,
                &c.view_proj,
                [t[0], t[1], t[2], input.caster_reach],
                [i as u32, NO_HOST, NO_HOST, NO_HOST],
            );
            let mut drawn = false;
            let mut whole = false;
            let scroll = match self.cascade_keys[i] {
                Some(old)
                    if old != c.view_proj
                        && SCROLL_CACHE.load(std::sync::atomic::Ordering::Relaxed) =>
                {
                    scroll_of(&old, &c.view_proj, size)
                }
                _ => None,
            };
            match (self.cascade_keys[i], scroll) {
                (Some(old), _) if old == c.view_proj => {}
                // Moved by whole texels: the cache scrolled, the strips it uncovers drawn.
                (_, Some(shift)) => {
                    jobs.push(Job::Scroll { layer: i, shift });
                    for strip in uncovered(shift, size) {
                        jobs.push(Job::Cascade {
                            layer: i,
                            patch: Some(strip),
                        });
                    }
                    stats.cascade_scrolls += 1;
                    drawn = true;
                }
                _ => {
                    jobs.push(Job::Cascade {
                        layer: i,
                        patch: None,
                    });
                    stats.cascade_redraws += 1;
                    drawn = true;
                    whole = true;
                }
            }
            self.cascade_keys[i] = Some(c.view_proj);
            if whole {
                self.cascade_stale[i] = None;
            }
            if !whole && !input.dirty.is_empty() {
                let mut rect = None;
                for b in input.dirty {
                    if casts_into(&c.view_proj, b[0], b[1])
                        && let Some(r) = texel_rect(&c.view_proj, b[0], b[1], size)
                    {
                        rect = Some(rect_union(rect, r));
                    }
                }
                if let Some(r) = rect {
                    let share = f64::from((r[2] - r[0]) * (r[3] - r[1])) / f64::from(size).powi(2);
                    if share <= PATCH_SHARE && self.cascade_stale[i].is_none() {
                        jobs.push(Job::Cascade {
                            layer: i,
                            patch: Some(r),
                        });
                        stats.cascade_patches += 1;
                        drawn = true;
                    } else {
                        self.cascade_stale[i].get_or_insert(self.frames);
                        whole_turn.get_or_insert(i);
                    }
                }
            }
            if !whole && !whole_spent && whole_turn == Some(i) && self.cascade_stale[i].is_some() {
                jobs.push(Job::Cascade {
                    layer: i,
                    patch: None,
                });
                stats.cascade_redraws += 1;
                self.cascade_stale[i] = None;
                whole_spent = true;
                drawn = true;
            }
            let objects: Vec<usize> = input
                .objects
                .iter()
                .enumerate()
                .filter(|(_, o)| {
                    let r = o.radius;
                    let c3 = o.centre;
                    let (lo, hi) = (
                        [c3[0] - r, c3[1] - r, c3[2] - r],
                        [c3[0] + r, c3[1] + r, c3[2] + r],
                    );
                    // Not into a cascade a finer one already holds its whole shadow for (every one of a moving
                    // Player's four cascades was copied from its static map and drawn over each frame: 256 MB at Ultra).
                    casts_into(&c.view_proj, lo, hi)
                        && !cs[..i].iter().any(|f| holds_shadow(&f.view_proj, lo, hi))
                })
                .map(|(k, _)| k)
                .collect();
            // The working cascade is made again when its static cascade was drawn, when passes from outside draw into it, or when
            // its overlay is not the one it holds (shapes came, went or changed); else it stands as it is.
            let fresh = drawn || self.drawn_over || !self.keep_overlays;
            let key = overlay_key(caster, &objects, input);
            if fresh || key != self.cascade_overlay[i] {
                jobs.push(Job::CopyCascade { layer: i, to: i });
                if !objects.is_empty() {
                    stats.overlays += 1;
                    jobs.push(Job::CascadeOverlay {
                        layer: i,
                        to: i,
                        objects: objects.clone(),
                    });
                }
                // A pass from outside drew into it: what it holds is not known next frame.
                self.cascade_overlay[i] = if self.drawn_over {
                    Overlay::Unknown
                } else {
                    key
                };
            }
            // The first-person images are lit by the nearest cascades with the client's own Player left out: it casts a shadow of
            // the held item's copy at the hand, not onto the image drawn before the eye.
            if i < NO_SELF_CASCADES && input.objects.iter().any(|o| o.own) {
                let rest: Vec<usize> = objects
                    .iter()
                    .copied()
                    .filter(|&k| !input.objects[k].own)
                    .collect();
                no_self = i + 1;
                let key = overlay_key(caster, &rest, input);
                if fresh || key != self.cascade_self_overlay[i] {
                    jobs.push(Job::CopyCascade {
                        layer: i,
                        to: layers + i,
                    });
                    if !rest.is_empty() {
                        stats.overlays += 1;
                        jobs.push(Job::CascadeOverlay {
                            layer: i,
                            to: layers + i,
                            objects: rest,
                        });
                    }
                    self.cascade_self_overlay[i] = if self.drawn_over {
                        Overlay::Unknown
                    } else {
                        key
                    };
                }
            } else {
                // Not kept up while there is no own Player: made again when one is back.
                self.cascade_self_overlay[i] = Overlay::Unknown;
            }
        }
        // The transmittance maps follow their static cascades.
        let coloured = input.coloured && self.trans.is_some() && !cs.is_empty();
        if coloured {
            for i in 0..cs.len() {
                let redrawn = jobs
                    .iter()
                    .any(|j| matches!(j, Job::Cascade { layer, .. } if *layer == i));
                if redrawn || !self.trans_valid[i] {
                    jobs.push(Job::Transmit { layer: i });
                    self.trans_valid[i] = true;
                }
            }
        }
        for k in cs.len()..MAX_CASCADES {
            self.cascade_keys[k] = None;
            self.cascade_stale[k] = None;
            self.cascade_overlay[k] = Overlay::Unknown;
            self.cascade_self_overlay[k] = Overlay::Unknown;
        }

        // Point lights.
        let usable = &input.lights[..input.lights.len().min(crate::MAX_LIGHTS)];
        self.light_slots = vec![[-1.0, 0.0, -1.0, 0.0]; usable.len()];
        let keys: Vec<[u32; 4]> = usable.iter().map(light_key).collect();
        let ranked: Vec<([u32; 4], f32)> = if input.switches.points && !self.slots.is_empty() {
            Self::lights_in_view(usable, input.cones, input.camera)
                .into_iter()
                .map(|(score, i)| (keys[i], score))
                .collect()
        } else {
            self.invalidate_slots();
            Vec::new()
        };
        assign_slots(
            &mut self.slots,
            &self.maps.tier_ends(),
            &ranked,
            &self.seen_last,
        );
        self.seen_last = ranked.iter().map(|r| r.0).collect();
        let held: Vec<(usize, usize)> = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(slot, s)| Some((keys.iter().position(|k| Some(*k) == s.light)?, slot)))
            .collect();
        for (li, slot) in held {
            let st = &self.slots[slot];
            let half = if st.leaving { 2 } else { 0 };
            self.light_slots[li][half] = slot as f32;
            self.light_slots[li][half + 1] = st.strength;
            let l = usable[li];
            stats.point_lights += 1;
            let cone = input.cones.get(li).copied().unwrap_or([0.0; 4]);
            for face in 0..6 {
                if !cone_face(cone, face) {
                    // Drawn again in full should the cone turn to it.
                    let st = &mut self.slots[slot];
                    st.valid[face] = false;
                    st.overlay[face] = Overlay::Unknown;
                    continue;
                }
                let m = cube_face(l.position, l.radius, face);
                let caster = put_caster(
                    MAX_CASCADES + slot * 6 + face,
                    &m,
                    [l.position[0], l.position[1], l.position[2], l.radius],
                    input.hosts.get(li).copied().unwrap_or([NO_HOST; 4]),
                );
                let st = &mut self.slots[slot];
                let mut drawn = false;
                if !st.valid[face]
                    || input
                        .dirty
                        .iter()
                        .any(|b| face_sees(l.position, l.radius, face, b[0], b[1]))
                {
                    jobs.push(Job::Face {
                        slot,
                        face,
                        light: li,
                    });
                    st.valid[face] = true;
                    stats.face_redraws += 1;
                    drawn = true;
                }
                let objects: Vec<usize> = input
                    .objects
                    .iter()
                    .enumerate()
                    // Not the shape the light is attached to (a Player's held light: its own body and arm cast
                    // a wedge of dark from beside the source).
                    .filter(|(_, o)| {
                        l.carrier != Some(o.ghost)
                            && face_sees_sphere(l.position, l.radius, face, o.centre, o.radius)
                    })
                    .map(|(k, _)| k)
                    .collect();
                let key = overlay_key(caster, &objects, input);
                if drawn || !self.keep_overlays || key != st.overlay[face] {
                    jobs.push(Job::CopyFace { slot, face });
                    if !objects.is_empty() {
                        stats.overlays += 1;
                        jobs.push(Job::FaceOverlay {
                            slot,
                            face,
                            objects,
                        });
                    }
                    st.overlay[face] = key;
                }
            }
        }

        // The uniform.
        if !caster_bytes.is_empty() {
            queue.write_buffer(&self.casters, 0, &caster_bytes);
        }
        self.caster_bytes = caster_bytes;
        let mut u = Vec::with_capacity(SHADOWS_BYTES);
        for i in 0..MAX_CASCADES {
            push_f32(&mut u, &cs.get(i).map_or([0.0; 16], |c| c.view_proj));
        }
        for i in 0..MAX_CASCADES {
            let p = cs.get(i).map_or([0.0; 4], |c| {
                [
                    1.0 / size as f32,
                    c.texel,
                    c.depth_range,
                    1.0 / (2.0 * c.radius),
                ]
            });
            push_f32(&mut u, &p);
        }
        for axis in light_basis(input.sun.towards) {
            push_f32(
                &mut u,
                &[axis[0] as f32, axis[1] as f32, axis[2] as f32, 0.0],
            );
        }
        push_u32(
            &mut u,
            &[cs.len() as u32, q.pcf_taps, q.blocker_taps, input.debug],
        );
        push_f32(
            &mut u,
            &[
                sun_radius_deg.to_radians().tan(),
                range.end,
                range.fade,
                size as f32,
            ],
        );
        let t = input.sun.shadow.map(|c| c.clamp(0.0, 1.0));
        let contact_on = input.switches.contact && sun_on && q.contact_steps > 0;
        push_f32(
            &mut u,
            &[t[0], t[1], t[2], if contact_on { 1.0 } else { 0.0 }],
        );
        let points_on = stats.point_lights > 0;
        push_f32(
            &mut u,
            &[
                q.point_taps as f32,
                self.maps.points[0].size as f32,
                if points_on { 1.0 } else { 0.0 },
                q.contact_steps as f32,
            ],
        );
        let ends = self.maps.tier_ends();
        push_f32(
            &mut u,
            &[
                ends[0] as f32,
                ends[1] as f32,
                self.maps.points[1].size as f32,
                self.maps.points[2].size as f32,
            ],
        );
        push_f32(
            &mut u,
            &[
                if coloured { 1.0 } else { 0.0 },
                (layers + 1) as f32,
                no_self as f32,
                input.ao_sun,
            ],
        );
        debug_assert_eq!(u.len(), SHADOWS_BYTES);
        queue.write_buffer(&self.uniform, 0, &u);

        // The passes.
        let ext_buffers = [
            input.ext[0].clone(),
            input.ext[1].clone(),
            input.nodes.clone(),
        ];
        let ext_group = match &self.ext_group {
            Some((buffers, group)) if *buffers == ext_buffers => group.clone(),
            _ => {
                let group = input
                    .gpu
                    .device
                    .create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("hd shadow extension"),
                        layout: &self.ext_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: ext_buffers[0].as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: ext_buffers[1].as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: ext_buffers[2].as_entire_binding(),
                            },
                        ],
                    });
                self.ext_group = Some((ext_buffers, group.clone()));
                group
            }
        };
        // Each map's casters' view (the jobs that draw cells: a cascade, a face, a transmittance map).
        let views: Vec<Option<crate::cull::ViewPlanes>> = jobs
            .iter()
            .map(|job| match job {
                Job::Cascade { layer, patch } => {
                    use crate::cull::{VIEW_NEAR, VIEW_OPAQUE, ViewPlanes};
                    let c = &cs[*layer];
                    // The cells' near clusters cast where the extra casters do.
                    let near = input.extras.as_ref().is_some_and(|x| c.texel < x.texel);
                    let kinds = VIEW_OPAQUE | if near { VIEW_NEAR } else { 0 };
                    let mut v = ViewPlanes::cascade(
                        &c.view_proj,
                        patch.map(|r| patch_clip(r, size, 2.0)),
                        kinds,
                    );
                    // Every cell's near clusters, near the eye or not.
                    v.eye[3] = f32::MAX;
                    if input.gpu.whole {
                        v.kinds |= crate::cull::VIEW_WHOLE;
                    }
                    Some(v)
                }
                Job::Face { face, light, .. } => {
                    let light = usable[*light];
                    let whole = if input.gpu.whole {
                        crate::cull::VIEW_WHOLE
                    } else {
                        0
                    };
                    Some(crate::cull::ViewPlanes::cube_face(
                        &cube_face(light.position, light.radius, *face),
                        light.position,
                        light.radius,
                        crate::cull::VIEW_OPAQUE | whole,
                    ))
                }
                Job::Transmit { layer } => match (&self.trans, cs.get(*layer)) {
                    (Some(_), Some(c)) => Some(crate::cull::ViewPlanes::cascade(
                        &c.view_proj,
                        None,
                        crate::cull::VIEW_TRANSLUCENT,
                    )),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        // The jobs as passes: a cascade's scroll and the patches after it into the same layer in one pass (the same draws in
        // the same order), every other job alone.
        let mut runs: Vec<std::ops::Range<usize>> = Vec::new();
        for (n, job) in jobs.iter().enumerate() {
            let joins = match (runs.last().map(|r| &jobs[r.start]), job) {
                (
                    Some(
                        Job::Scroll { layer: a, .. }
                        | Job::Cascade {
                            layer: a,
                            patch: Some(_),
                        },
                    ),
                    Job::Cascade {
                        layer: b,
                        patch: Some(_),
                    },
                ) => a == b,
                _ => false,
            };
            match runs.last_mut() {
                Some(r) if joins => r.end = n + 1,
                _ => runs.push(n..n + 1),
            }
        }
        // The overlays' draws as indirect arguments, each overlay's in a row (one multi-draw an overlay).
        let mut overlay_at: Vec<(u32, u32)> = vec![(0, 0); jobs.len()];
        let indirect = input.gpu.way != crate::arena::DrawWay::Direct && input.loose.is_some();
        if indirect {
            let mut args: Vec<u8> = Vec::new();
            let mut at = 0u32;
            for (n, job) in jobs.iter().enumerate() {
                let (Job::CascadeOverlay { objects, .. } | Job::FaceOverlay { objects, .. }) = job
                else {
                    continue;
                };
                let first = at;
                for &k in objects {
                    for &(first_index, count, base) in &input.objects[k].draws {
                        if count > 0 {
                            for w in [count, 1, first_index, base as u32, 0] {
                                args.extend_from_slice(&w.to_le_bytes());
                            }
                            at += 1;
                        }
                    }
                }
                overlay_at[n] = (first, at - first);
            }
            if !args.is_empty() {
                let need = args.len() as u64;
                if self.overlay_args.as_ref().is_none_or(|b| b.size() < need) {
                    self.overlay_args =
                        Some(input.gpu.device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some("hd shadow overlay draws"),
                            size: need.next_power_of_two().max(4096),
                            usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
                            mapped_at_creation: false,
                        }));
                }
                if let Some(b) = &self.overlay_args {
                    queue.write_buffer(b, 0, &args);
                }
            }
        }
        let passes = runs.iter().filter(|r| jobs[r.start].is_pass()).count();
        let mut pass_no = 0;
        let kind_before = crate::set_pass_kind(crate::PassKind::Cascade);
        let batch_size = input.gpu.lists.len().max(1);
        let mut culled: Vec<Option<Culled>> = vec![None; jobs.len()];
        for (r, run) in runs.iter().enumerate() {
            // The next batch of culls, whole runs at a time, before the first pass that needs one not yet culled.
            if run
                .clone()
                .any(|n| views[n].is_some() && culled[n].is_none())
            {
                let mut picked: Vec<usize> = Vec::new();
                for later in &runs[r..] {
                    let need: Vec<usize> = later.clone().filter(|&n| views[n].is_some()).collect();
                    if !picked.is_empty() && picked.len() + need.len() > batch_size {
                        break;
                    }
                    picked.extend(need);
                }
                let planes: Vec<crate::cull::ViewPlanes> =
                    picked.iter().filter_map(|&n| views[n]).collect();
                for (n, c) in picked
                    .into_iter()
                    .zip(input.gpu.cull_batch(queue, encoder, &planes))
                {
                    culled[n] = Some(c);
                }
            }
            let stamp = |n: usize| {
                input.stamps.and_then(|(qs, begin, end)| {
                    let b = (n == 0).then_some(begin);
                    let e = (n + 1 == passes).then_some(end);
                    (b.is_some() || e.is_some()).then_some(wgpu::RenderPassTimestampWrites {
                        query_set: qs,
                        beginning_of_pass_write_index: b,
                        end_of_pass_write_index: e,
                    })
                })
            };
            let job = &jobs[run.start];
            let _ = crate::set_pass_kind(match job {
                Job::Face { .. } | Job::CopyFace { .. } => crate::PassKind::Face,
                Job::CascadeOverlay { .. } | Job::FaceOverlay { .. } => crate::PassKind::Overlay,
                _ => crate::PassKind::Cascade,
            });
            match job {
                Job::Scroll { layer, .. } | Job::Cascade { layer, .. } => {
                    let layer = *layer;
                    // The scroll: the cache into its copy, then back moved (its group made before the pass).
                    let scroll_group = if let Job::Scroll { shift, .. } = job {
                        copy_layer(
                            encoder,
                            &self.maps.csm_static,
                            &self.maps.csm_work,
                            layer as u32,
                            layer as u32,
                            size,
                        );
                        Some(self.scroll_group(queue, input.gpu.device, layer, *shift, size))
                    } else {
                        None
                    };
                    let clear =
                        matches!(job, Job::Scroll { .. } | Job::Cascade { patch: None, .. });
                    let mut pass = depth_pass(
                        encoder,
                        &self.maps.csm_static_layers[layer],
                        clear,
                        stamp(pass_no),
                    );
                    if let Some(group) = &scroll_group {
                        pass.set_pipeline(&self.scroll_pipeline);
                        pass.set_bind_group(2, group, &[]);
                        pass.draw(0..3, 0..1);
                        crate::count_draws(1);
                    }
                    for n in run.clone() {
                        if let (Job::Cascade { patch, .. }, Some(culled)) = (&jobs[n], &culled[n]) {
                            self.draw_cascade(
                                &mut pass,
                                input,
                                &ext_group,
                                (&cs[layer], layer, *patch),
                                culled,
                                &mut stats,
                            );
                        }
                    }
                    pass_no += 1;
                }
                Job::CopyCascade { layer, to } => {
                    copy_layer(
                        encoder,
                        &self.maps.csm_static,
                        &self.maps.csm_work,
                        *layer as u32,
                        *to as u32,
                        size,
                    );
                }
                Job::CopyFace { slot, face } => {
                    let (t, first) = self.maps.point_layer(*slot);
                    copy_layer(
                        encoder,
                        &t.stat,
                        &t.work,
                        (first + face) as u32,
                        (first + face) as u32,
                        t.size,
                    );
                }
                Job::CascadeOverlay { layer, to, objects } => {
                    let mut pass = depth_pass(
                        encoder,
                        &self.maps.csm_work_layers[*to],
                        false,
                        stamp(pass_no),
                    );
                    pass.set_bind_group(
                        0,
                        &self.caster_group,
                        &[(*layer as u64 * CASTER_STRIDE) as u32],
                    );
                    pass.set_pipeline(&self.sun_pipeline);
                    pass.set_bind_group(3, &ext_group, &[]);
                    self.draw_overlay(&mut pass, input, objects, indirect, overlay_at[run.start]);
                    pass_no += 1;
                }
                Job::Face { slot, face, .. } => {
                    let index = slot * 6 + face;
                    let Some(culled) = culled[run.start] else {
                        continue;
                    };
                    let (t, first) = self.maps.point_layer(*slot);
                    let mut pass = depth_pass(
                        encoder,
                        &t.static_layers[first + face],
                        true,
                        stamp(pass_no),
                    );
                    pass_no += 1;
                    pass.set_bind_group(
                        0,
                        &self.caster_group,
                        &[((MAX_CASCADES + index) as u64 * CASTER_STRIDE) as u32],
                    );
                    pass.set_pipeline(&self.point_pipeline);
                    pass.set_bind_group(3, &ext_group, &[]);
                    stats.cell_draws += input.gpu.draw(&mut pass, input.arena, false, &culled);
                }
                Job::Transmit { layer } => {
                    let (Some(t), Some(culled)) = (&self.trans, culled[run.start]) else {
                        continue;
                    };
                    crate::count_pass();
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("hd transmittance"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &t.layers[*layer],
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: stamp(pass_no),
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    pass_no += 1;
                    pass.set_pipeline(&self.trans_pipeline);
                    pass.set_bind_group(
                        0,
                        &self.caster_group,
                        &[(*layer as u64 * CASTER_STRIDE) as u32],
                    );
                    pass.set_bind_group(1, &self.trans_group, &[]);
                    input.gpu.draw(&mut pass, input.arena, true, &culled);
                }
                Job::FaceOverlay {
                    slot,
                    face,
                    objects,
                } => {
                    let index = slot * 6 + face;
                    let (t, first) = self.maps.point_layer(*slot);
                    let mut pass =
                        depth_pass(encoder, &t.work_layers[first + face], false, stamp(pass_no));
                    pass.set_bind_group(
                        0,
                        &self.caster_group,
                        &[((MAX_CASCADES + index) as u64 * CASTER_STRIDE) as u32],
                    );
                    pass.set_pipeline(&self.point_pipeline);
                    pass.set_bind_group(3, &ext_group, &[]);
                    self.draw_overlay(&mut pass, input, objects, indirect, overlay_at[run.start]);
                    pass_no += 1;
                }
            }
        }
        crate::restore_pass_kind(kind_before);
        self.stats = stats;
        Encoded {
            timed: passes > 0 && input.stamps.is_some(),
            contact: contact_on,
        }
    }
}

impl Shadows {
    /// A cascade's scroll's group: its shift written into the cascade's uniform slot.
    fn scroll_group(
        &self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        layer: usize,
        shift: [i32; 2],
        size: u32,
    ) -> wgpu::BindGroup {
        let at = layer as u64 * SCROLL_STRIDE;
        let mut b = Vec::with_capacity(16);
        for w in [shift[0], shift[1], layer as i32, size as i32] {
            b.extend_from_slice(&w.to_le_bytes());
        }
        queue.write_buffer(&self.scroll_uniform, at, &b);
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd shadow scroll"),
            layout: &self.scroll_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.maps.csm_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.scroll_uniform,
                        offset: at,
                        size: wgpu::BufferSize::new(16),
                    }),
                },
            ],
        })
    }

    /// A static cascade's draws into its pass (a patch: cleared under its rectangle first): the cells, then the extension's extra
    /// casters in a cascade fine enough to hold their shadow.
    fn draw_cascade(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        input: &ShadowInput<'_>,
        ext_group: &wgpu::BindGroup,
        (c, layer, patch): (&Cascade, usize, Option<[u32; 4]>),
        culled: &Culled,
        stats: &mut ShadowStats,
    ) {
        let size = self.maps.quality.cascade_size;
        pass.set_bind_group(
            0,
            &self.caster_group,
            &[(layer as u64 * CASTER_STRIDE) as u32],
        );
        if let Some(r) = patch {
            pass.set_scissor_rect(r[0], r[1], r[2] - r[0], r[3] - r[1]);
            pass.set_pipeline(&self.clear_pipeline);
            pass.draw(0..3, 0..1);
            crate::count_draws(1);
        }
        pass.set_pipeline(&self.sun_pipeline);
        pass.set_bind_group(3, ext_group, &[]);
        stats.cell_draws += input.gpu.draw(pass, input.arena, false, culled);
        // The extra casters, in a cascade fine enough to hold their shadow (the cells within the near detail's distance of the
        // eye).
        if let Some(x) = &input.extras
            && let Some(pipeline) = &self.extra_pipeline
            && c.texel < x.texel
            && x.casters.bind(pass)
        {
            pass.set_pipeline(pipeline);
            for k in &x.cells {
                let Some(cell) = input.cells.get(k).filter(|_| x.casters.holds(*k)) else {
                    continue;
                };
                let b = cell.bounds;
                if !casts_into(&c.view_proj, b[0], b[1]) {
                    continue;
                }
                if let Some(r) = patch
                    && !texel_rect(&c.view_proj, cell.bounds[0], cell.bounds[1], size)
                        .is_some_and(|cr| rects_meet(cr, r))
                {
                    continue;
                }
                x.casters.cast(pass, *k);
                crate::count_draws(1);
            }
        }
    }

    /// The moving shapes' draws into an overlay: one multi-draw of the frame's indirect arguments (`at`: the overlay's first
    /// and count), or a draw each where the device does not draw indirectly.
    fn draw_overlay(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        input: &ShadowInput<'_>,
        objects: &[usize],
        indirect: bool,
        at: (u32, u32),
    ) {
        match (&self.overlay_args, input.loose) {
            (Some(args), Some((v, i))) if indirect => {
                if at.1 == 0 {
                    return;
                }
                pass.set_vertex_buffer(0, v.slice(..));
                pass.set_index_buffer(i.slice(..), wgpu::IndexFormat::Uint32);
                pass.multi_draw_indexed_indirect(args, u64::from(at.0) * 20, at.1);
                crate::count_draws(1);
            }
            _ => draw_objects(pass, input, objects),
        }
    }
}

/// What a working map holds: its static map alone, the shapes of an overlay (the caster's bytes, the extension's shadow
/// generation and each shape's ghost, draws and content hashed), or something not known to be either (nothing yet, a pass from
/// outside drew into it, or it was not kept up): a map is made again whenever what it should hold is not this.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Overlay {
    Static,
    Shapes(u64),
    #[default]
    Unknown,
}

/// What an overlay of `objects` over the caster `caster` (its bytes hashed) shows. Two frames' overlays with the same key are
/// the same picture.
fn overlay_key(caster: u64, objects: &[usize], input: &ShadowInput<'_>) -> Overlay {
    use std::hash::{Hash, Hasher};
    if objects.is_empty() {
        return Overlay::Static;
    }
    let mut h = std::hash::DefaultHasher::new();
    caster.hash(&mut h);
    input.ext_generation.hash(&mut h);
    for &k in objects {
        let o = &input.objects[k];
        o.ghost.hash(&mut h);
        o.draws.hash(&mut h);
        o.content.hash(&mut h);
    }
    Overlay::Shapes(h.finish())
}

/// A cascade's slot in the scroll's uniform (the uniform offset alignment).
const SCROLL_STRIDE: u64 = 256;

fn copy_layer(
    encoder: &mut wgpu::CommandEncoder,
    from: &wgpu::Texture,
    to: &wgpu::Texture,
    layer: u32,
    to_layer: u32,
    size: u32,
) {
    fn at(t: &wgpu::Texture, layer: u32) -> wgpu::TexelCopyTextureInfo<'_> {
        wgpu::TexelCopyTextureInfo {
            texture: t,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: 0,
                y: 0,
                z: layer,
            },
            aspect: wgpu::TextureAspect::All,
        }
    }
    encoder.copy_texture_to_texture(
        at(from, layer),
        at(to, to_layer),
        wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
    );
}

fn depth_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    clear: bool,
    stamps: Option<wgpu::RenderPassTimestampWrites<'_>>,
) -> wgpu::RenderPass<'e> {
    crate::count_pass();
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("hd shadow"),
        color_attachments: &[],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view,
            depth_ops: Some(wgpu::Operations {
                load: if clear {
                    wgpu::LoadOp::Clear(1.0)
                } else {
                    wgpu::LoadOp::Load
                },
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: None,
        }),
        timestamp_writes: stamps,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

fn draw_objects(pass: &mut wgpu::RenderPass<'_>, input: &ShadowInput<'_>, objects: &[usize]) {
    let Some((v, i)) = input.loose else { return };
    pass.set_vertex_buffer(0, v.slice(..));
    pass.set_index_buffer(i.slice(..), wgpu::IndexFormat::Uint32);
    for &k in objects {
        for &(first, count, base) in &input.objects[k].draws {
            if count > 0 {
                pass.draw_indexed(first..first + count, base, 0..1);
                crate::count_draws(1);
            }
        }
    }
}

#[cfg(test)]
#[path = "shadows_tests.rs"]
mod tests;
