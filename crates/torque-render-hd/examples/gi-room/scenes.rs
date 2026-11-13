//! The test scenes: their geometry (boxes in static cells, quads among the planes), lights, cameras and the surfaces the
//! reference and the checks sample.

use std::collections::BTreeMap;

use torque_render_core::wgpu;
use torque_render_hd::ext::{ExtCell, HdExtension, PlainScene, Surfaces};
use torque_render_hd::shaders::{NONE, ShaderExtension};
use torque_render_scene::hd::cells::{Batch, Cell, SlotTexture, StaticCells, cell_key};
use torque_render_scene::hd::kit::{box_object, look_at};
use torque_render_scene::hd::scatter::{ScatterLook, ScatterTable};
use torque_render_scene::hd::{
    AreaShape, DynamicObject, HdAreaLight, HdCamera, HdDraw, HdLight, HdMode, HdScene, HdVertex,
    Quality, clusters, material, oct_encode,
};
use torque_render_scene::{Blend, Cull, Filter, Sampler};

/// The pictures' size.
pub const SIZE: (u32, u32) = (1920, 1080);
const FOV: u8 = 90;

/// The engine's shading with the area-lights example's glossier floor and blocks.
pub struct Room;

impl HdExtension for Room {
    type Geometry = StaticCells;
    type Scene = PlainScene;
    type CellRuns = ();
    type LevelInput = ();
    const FRAME_EXT_BYTES: usize = 16;

    fn new(_device: &wgpu::Device) -> Room {
        Room
    }

    fn shaders() -> ShaderExtension {
        NONE
    }

    fn mode(scene: &PlainScene) -> Option<HdMode> {
        scene.mode
    }

    fn time(scene: &PlainScene) -> f32 {
        scene.time
    }

    fn level_input(_cell: &ExtCell<Room>) {}

    fn surfaces(&self, scene: &PlainScene) -> Surfaces {
        let mut s = Surfaces::of(&Self::mode(scene).unwrap_or_else(Self::fallback_mode).image);
        s.table[4] = 0.45;
        s.table[8] = 0.3;
        s
    }
}

/// A flat surface the checks sample: a rectangle about `center` with half sides `u` and `v`, facing `normal`, in a region named
/// `name` (its room and side).
#[derive(Debug, Clone, Copy)]
pub struct Region {
    pub name: &'static str,
    pub center: [f32; 3],
    pub u: [f32; 3],
    pub v: [f32; 3],
    pub normal: [f32; 3],
}

impl Region {
    /// The region's `n` x `n` sample points (cell centers), each a hundredth of a unit off the surface.
    pub fn points(&self, n: usize) -> Vec<[f32; 3]> {
        let mut out = Vec::with_capacity(n * n);
        for j in 0..n {
            for i in 0..n {
                let a = (i as f32 + 0.5) / n as f32 * 2.0 - 1.0;
                let b = (j as f32 + 0.5) / n as f32 * 2.0 - 1.0;
                out.push(std::array::from_fn(|k| {
                    self.center[k] + a * self.u[k] + b * self.v[k] + 0.01 * self.normal[k]
                }));
            }
        }
        out
    }

    /// The region's `n` x `n` sample points, each with the region's normal.
    pub fn facing(&self, n: usize) -> Vec<([f32; 3], [f32; 3])> {
        self.points(n)
            .into_iter()
            .map(|p| (p, self.normal))
            .collect()
    }
}

/// A test scene: its frame, the camera of its picture and the surfaces the checks sample.
pub struct TestScene {
    pub name: &'static str,
    pub scene: PlainScene,
    pub camera: HdCamera,
    pub regions: Vec<Region>,
}

/// The six faces of the box from `lo` to `hi` (each a quad of four vertices, its normal outward) of gamma `color`, `lit` false
/// for an emitter drawn as its color.
fn box_quads(lo: [f32; 3], hi: [f32; 3], color: [u8; 3], lit: bool) -> Vec<[HdVertex; 4]> {
    let mut kind = material::KIND_SHAPE << material::KIND_SHIFT;
    if !lit {
        kind |= material::UNLIT;
    }
    let mut out = Vec::with_capacity(6);
    for axis in 0..3 {
        for sign in [-1.0f32, 1.0] {
            let mut n = [0.0f32; 3];
            n[axis] = sign;
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            // Clockwise seen from outside (the static meshes' front: their back faces are culled).
            let corners: [(f32, f32); 4] = if sign > 0.0 {
                [(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0)]
            } else {
                [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
            };
            out.push(corners.map(|(a, b)| {
                let mut p = [0.0; 3];
                p[axis] = if sign > 0.0 { hi[axis] } else { lo[axis] };
                p[u] = lo[u] + a * (hi[u] - lo[u]);
                p[v] = lo[v] + b * (hi[v] - lo[v]);
                HdVertex {
                    position: p,
                    normal: oct_encode(n),
                    uv: [0.0; 2],
                    color: [color[0], color[1], color[2], 255],
                    material: kind,
                    extra: 0,
                }
            }));
        }
    }
    out
}

/// A box of the static geometry.
#[derive(Debug, Clone, Copy)]
pub struct Block {
    pub lo: [f32; 3],
    pub hi: [f32; 3],
    pub color: [u8; 3],
}

const fn block(lo: [f32; 3], hi: [f32; 3], color: [u8; 3]) -> Block {
    Block { lo, hi, color }
}

/// The boxes as static cells (each box in the cell its middle lies in), one untextured slot, `generation` their build: a
/// renderer keeps a cell whose key and generation it already holds, so scenes drawn by one renderer take distinct generations.
pub fn cells_of(blocks: &[Block], generation: u64) -> StaticCells {
    let mut by_cell: BTreeMap<[i32; 3], Vec<[HdVertex; 4]>> = BTreeMap::new();
    for b in blocks {
        let mid = std::array::from_fn(|k| 0.5 * (b.lo[k] + b.hi[k]));
        by_cell
            .entry(cell_key(mid))
            .or_default()
            .extend(box_quads(b.lo, b.hi, b.color, true));
    }
    let mut cells = StaticCells::default();
    cells.slots.insert(
        0,
        SlotTexture {
            texture: None,
            sampler: Sampler {
                repeat: true,
                mag: Filter::Linear,
                min: Filter::Linear,
                mip: Filter::Linear,
                mipmaps: true,
            },
        },
    );
    for (key, mut quads) in by_cell {
        clusters::order(&mut quads, clusters::cell_origin(key));
        let vertices: Vec<HdVertex> = quads.iter().flatten().copied().collect();
        let indices: Vec<u32> = (0..quads.len() as u32)
            .flat_map(|q| [0, 1, 2, 0, 2, 3].map(|i| 4 * q + i))
            .collect();
        let batches = vec![Batch {
            slot: 0,
            translucent: false,
            first: 0,
            count: indices.len() as u32,
        }];
        let bounds = vertices.iter().fold(
            [[f32::INFINITY; 3], [f32::NEG_INFINITY; 3]],
            |[lo, hi], v| {
                [
                    std::array::from_fn(|k| lo[k].min(v.position[k])),
                    std::array::from_fn(|k| hi[k].max(v.position[k])),
                ]
            },
        );
        let clusters = clusters::of_cell(&vertices, &batches, key);
        cells.cells.insert(
            key,
            Cell {
                key,
                bounds,
                vertices,
                indices,
                batches,
                clusters,
                generation,
                resident: false,
                ext: (),
            },
        );
    }
    cells
}

/// The boxes as one plane draw (the planes' path: a room or a ground uploaded with the eye-dependent draws), `lit` false for
/// an emitter drawn as its color.
fn planes_of(blocks: &[Block], lit: bool) -> HdDraw {
    let quads: Vec<[HdVertex; 4]> = blocks
        .iter()
        .flat_map(|b| box_quads(b.lo, b.hi, b.color, lit))
        .collect();
    let plane =
        (material::KIND_PLANE << material::KIND_SHIFT) | if lit { 0 } else { material::UNLIT };
    let vertices: Vec<HdVertex> = quads
        .iter()
        .flatten()
        .map(|v| HdVertex {
            material: plane,
            ..*v
        })
        .collect();
    let indices = (0..quads.len() as u32)
        .flat_map(|q| [0, 1, 2, 0, 2, 3].map(|i| 4 * q + i))
        .collect();
    HdDraw {
        label: "gi room".into(),
        vertices,
        indices,
        texture: None,
        blend: Blend::Opaque,
        cull: Cull::None,
        depth_test: true,
        depth_write: true,
        pbr: None,
    }
}

const WALL: [u8; 3] = [200, 196, 188];
const FLOOR: [u8; 3] = [150, 128, 104];
const RED: [u8; 3] = [190, 60, 48];
const BLUE: [u8; 3] = [70, 100, 170];
const DOOR: [u8; 3] = [120, 84, 56];

/// The cells' generations of the scenes drawn by one renderer (each distinct: see [`cells_of`]).
const OPEN_GENERATION: u64 = 1;
const SHUT_GENERATION: u64 = 2;
const COURTYARD_GENERATION: u64 = 3;

/// The two rooms' boxes: room A from x -6 to 0, room B from 0.5 to 6.5 (y -3 to 3, z 0 to 3), the wall between them half a
/// unit thick with a doorway 1.2 wide and 2.2 high, open when `open` and else filled by a brown door set a tenth of a unit
/// back from room A's side, A's west wall blue and its south wall red.
pub fn two_rooms_blocks(open: bool) -> Vec<Block> {
    let mut b = vec![
        block([-6.5, -3.5, -0.5], [7.0, 3.5, 0.0], FLOOR),
        block([-6.5, -3.5, 3.0], [7.0, 3.5, 3.5], WALL),
        block([-6.5, -3.0, 0.0], [-6.0, 3.0, 3.0], BLUE),
        block([6.5, -3.0, 0.0], [7.0, 3.0, 3.0], WALL),
        block([-6.5, -3.5, 0.0], [0.25, -3.0, 3.0], RED),
        block([0.25, -3.5, 0.0], [7.0, -3.0, 3.0], WALL),
        block([-6.5, 3.0, 0.0], [7.0, 3.5, 3.0], WALL),
    ];
    b.push(block([0.0, -3.0, 0.0], [0.5, -0.6, 3.0], WALL));
    b.push(block([0.0, 0.6, 0.0], [0.5, 3.0, 3.0], WALL));
    b.push(block([0.0, -0.6, 2.2], [0.5, 0.6, 3.0], WALL));
    if !open {
        b.push(block([0.1, -0.6, 0.0], [0.4, 0.6, 2.2], DOOR));
    }
    b
}

/// The block wall in room B: 20 blocks half a unit a side, ten along x and two high, against the north wall.
pub fn block_wall() -> Vec<Block> {
    (0..20)
        .map(|i| {
            let (ix, iz) = ((i % 10) as f32, (i / 10) as f32);
            block(
                [1.5 + 0.5 * ix, 2.0, 0.5 * iz],
                [2.0 + 0.5 * ix, 2.5, 0.5 + 0.5 * iz],
                [220, 200, 120],
            )
        })
        .collect()
}

/// Room A's ceiling panel: a 2 x 1 rectangle lit downward, and its emitter's box.
fn panel() -> (HdAreaLight, Block) {
    let at = [-3.0, 0.0, 2.97];
    (
        HdAreaLight {
            light: HdLight {
                position: at,
                color: [1.0, 0.92, 0.8].map(|c| c * 4.0),
                radius: 14.0,
                carrier: None,
            },
            shape: AreaShape::Rect {
                half_u: [0.0, 0.5, 0.0],
                half_v: [1.0, 0.0, 0.0],
            },
            shadows: true,
        },
        block(
            [at[0] - 1.0, at[1] - 0.5, at[2] - 0.01],
            [at[0] + 1.0, at[1] + 0.5, at[2] + 0.02],
            [255, 246, 232],
        ),
    )
}

/// A night: a faint ambient, no sun, a black sky.
fn night(s: &mut HdScene<StaticCells>) {
    s.visible_distance = 200.0;
    s.fog_distance = 150.0;
    s.fog.start = 150.0;
    s.fog.end = 200.0;
    s.sun.ambient = [0.02, 0.02, 0.025];
    s.clear = [0.0; 3];
}

fn camera_of(s: &mut HdScene<StaticCells>, from: [f32; 3], to: [f32; 3]) -> HdCamera {
    let eye = look_at(from, to);
    s.eye = Some(eye);
    s.fov = FOV;
    s.size = SIZE;
    HdCamera::new(eye, f32::from(FOV), s.visible_distance, SIZE.0, SIZE.1)
}

/// The surfaces of the two rooms the checks sample: each room's floor and its walls (the divider's faces beside the door).
fn two_rooms_regions() -> Vec<Region> {
    let r = |name, center, u, v, normal| Region {
        name,
        center,
        u,
        v,
        normal,
    };
    let (x, y, z) = ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]);
    let s = |v: [f32; 3], k: f32| v.map(|c| c * k);
    vec![
        r("A floor", [-3.0, 0.0, 0.0], s(x, 2.6), s(y, 2.6), z),
        r("A west wall", [-6.0, 0.0, 1.5], s(y, 2.6), s(z, 1.2), x),
        r("A south wall", [-3.0, -3.0, 1.5], s(x, 2.6), s(z, 1.2), y),
        r(
            "A north wall",
            [-3.0, 3.0, 1.5],
            s(x, 2.6),
            s(z, 1.2),
            s(y, -1.0),
        ),
        r(
            "A divider",
            [0.0, 1.8, 1.2],
            s(y, 0.7),
            s(z, 1.0),
            s(x, -1.0),
        ),
        r("B floor", [3.5, -0.5, 0.0], s(x, 2.6), s(y, 2.2), z),
        r("B divider", [0.5, -1.8, 1.2], s(y, 0.7), s(z, 1.0), x),
        r(
            "B east wall",
            [6.5, 0.0, 1.5],
            s(y, 2.6),
            s(z, 1.2),
            s(x, -1.0),
        ),
        r("B south wall", [3.5, -3.0, 1.5], s(x, 2.6), s(z, 1.2), y),
    ]
}

/// The open doorway's surfaces (its jambs' faces, its lintel's underside and the floor under it): where the probes' cascades
/// meet thin walls on both sides, the doorway check's points.
pub fn doorway_regions() -> Vec<Region> {
    let (x, y, z) = ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]);
    let s = |v: [f32; 3], k: f32| v.map(|c| c * k);
    let r = |name, center, u, v, normal| Region {
        name,
        center,
        u,
        v,
        normal,
    };
    vec![
        r("south jamb", [0.25, -0.6, 1.1], s(x, 0.25), s(z, 1.1), y),
        r(
            "north jamb",
            [0.25, 0.6, 1.1],
            s(x, 0.25),
            s(z, 1.1),
            s(y, -1.0),
        ),
        r(
            "lintel",
            [0.25, 0.0, 2.2],
            s(x, 0.25),
            s(y, 0.6),
            s(z, -1.0),
        ),
        r("threshold", [0.25, 0.0, 0.0], s(x, 0.25), s(y, 0.6), z),
    ]
}

/// The two rooms (the door `open` or shut: the leak test), lit by room A's ceiling panel at night, seen from A's south-west
/// corner toward the door.
pub fn two_rooms(open: bool) -> TestScene {
    let mut scene = PlainScene {
        frame: HdScene::new(StaticCells::default()),
        mode: Some(HdMode::new(Quality::Ultra)),
        time: 0.0,
    };
    let (light, emitter) = panel();
    let mut blocks = two_rooms_blocks(open);
    blocks.extend(block_wall());
    let s = &mut scene.frame;
    let generation = if open {
        OPEN_GENERATION
    } else {
        SHUT_GENERATION
    };
    s.cells = cells_of(&blocks, generation);
    s.planes = vec![planes_of(&[emitter], false)];
    s.area_lights = vec![light];
    night(s);
    let camera = camera_of(s, [-5.4, -2.4, 1.8], [2.5, 1.2, 0.6]);
    TestScene {
        name: if open { "two-rooms" } else { "two-rooms-shut" },
        scene,
        camera,
        regions: two_rooms_regions(),
    }
}

/// A block of the area-lights room (a moving shape: it receives the light but casts none into the voxels).
fn shape(ghost: u32, center: [f32; 3], h: [f32; 3], color: [u8; 3]) -> DynamicObject {
    let mut o = box_object(ghost, center, h);
    for d in &mut o.draws {
        for v in &mut d.vertices {
            v.color = [color[0], color[1], color[2], 255];
        }
    }
    o
}

/// The `area-lights` example's room (8 x 6 x 3, its walls among the planes, a blue wall at +x and three blocks) under its
/// ceiling panel, at night, from the same corner.
pub fn area_room() -> TestScene {
    let mut scene = PlainScene {
        frame: HdScene::new(StaticCells::default()),
        mode: Some(HdMode::new(Quality::Ultra)),
        time: 0.0,
    };
    let (x, y, z) = (4.0, 3.0, 3.0);
    let t = 0.02;
    let room = [
        block([-x, -y, -t], [x, y, 0.0], [120, 112, 104]),
        block([-x, -y, z], [x, y, z + t], WALL),
        block([x, -y, 0.0], [x + t, y, z], [60, 90, 160]),
        block([-x - t, -y, 0.0], [-x, y, z], WALL),
        block([-x, y, 0.0], [x, y + t, z], WALL),
        block([-x, -y - t, 0.0], [x, -y, z], WALL),
    ];
    let at = [1.4, 1.0, z - 0.02];
    let light = HdAreaLight {
        light: HdLight {
            position: at,
            color: [1.0, 0.92, 0.8].map(|c| c * 3.2),
            radius: 12.0,
            carrier: None,
        },
        shape: AreaShape::Rect {
            half_u: [0.0, 0.6, 0.0],
            half_v: [1.2, 0.0, 0.0],
        },
        shadows: true,
    };
    let emitter = block(
        [at[0] - 1.2, at[1] - 0.6, at[2] - 0.008],
        [at[0] + 1.2, at[1] + 0.6, at[2] + 0.008],
        [255, 246, 232],
    );
    let s = &mut scene.frame;
    s.planes = vec![planes_of(&room, true), planes_of(&[emitter], false)];
    s.objects = vec![
        shape(1, [-0.6, 1.7, 0.45], [0.45, 0.45, 0.45], [235, 235, 232]),
        shape(2, [2.3, -0.4, 0.3], [0.7, 0.35, 0.3], [210, 110, 50]),
        shape(3, [0.6, -1.5, 0.9], [0.25, 0.25, 0.9], [150, 150, 155]),
    ];
    s.area_lights = vec![light];
    night(s);
    s.sun.ambient = [0.04, 0.04, 0.05];
    let camera = camera_of(s, [-3.6, -2.7, 1.6], [1.0, 0.8, 0.3]);
    TestScene {
        name: "area-room",
        scene,
        camera,
        regions: Vec::new(),
    }
}

/// A courtyard under the afternoon sun and the physical sky: a ground 80 units a side (a plane), an orange and a white wall in
/// an L, a pavilion of four pillars under a green roof, and a white block.
pub fn courtyard() -> TestScene {
    let mut scene = PlainScene {
        frame: HdScene::new(StaticCells::default()),
        mode: Some(HdMode::new(Quality::Ultra)),
        time: 0.0,
    };
    let mut blocks = vec![
        block([-6.0, -6.0, 0.0], [6.0, -5.5, 4.0], [210, 120, 60]),
        block([-6.0, -5.5, 0.0], [-5.5, 6.0, 4.0], WALL),
        block([1.0, 1.0, 3.0], [5.6, 5.6, 3.4], [90, 150, 90]),
        block([-2.0, -3.0, 0.0], [-1.0, -2.0, 1.0], [235, 235, 232]),
    ];
    for (px, py) in [(1.2, 1.2), (1.2, 5.0), (5.0, 1.2), (5.0, 5.0)] {
        blocks.push(block([px, py, 0.0], [px + 0.4, py + 0.4, 3.0], WALL));
    }
    let ground = block([-40.0, -40.0, -0.5], [40.0, 40.0, 0.0], [130, 120, 100]);
    let s = &mut scene.frame;
    s.cells = cells_of(&blocks, COURTYARD_GENERATION);
    s.planes = vec![planes_of(&[ground], true)];
    s.atmosphere = true;
    s.visible_distance = 600.0;
    s.fog_distance = 300.0;
    s.fog.start = 300.0;
    s.fog.end = 600.0;
    let sun = {
        let (az, el) = (230f32.to_radians(), 35f32.to_radians());
        [az.sin() * el.cos(), az.cos() * el.cos(), el.sin()]
    };
    let light = ScatterTable::earth().light(sun, &ScatterLook::default());
    s.sun = light.sun();
    s.fog = light.fog(s.fog);
    s.clear = light.fog;
    s.atmosphere_sun = Some(sun);
    let camera = camera_of(s, [9.0, -3.5, 2.2], [-1.0, 2.0, 1.2]);
    TestScene {
        name: "courtyard",
        scene,
        camera,
        regions: Vec::new(),
    }
}
