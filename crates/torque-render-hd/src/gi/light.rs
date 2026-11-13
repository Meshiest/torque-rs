//! The voxels' light (`gi_light.wgsl`): which tiles a frame lights again and the pass that lights them. A tile is lit the frame
//! it is made, when a light that reaches it changed (moved, switched, recolored: its bytes in the lights' buffer differ), when
//! the sun changed (every tile, the nearest first, under the budget), and in turn: a share of every tile with triangles each
//! frame, so nothing stays lit by an old sky or an old bounce for more than [`GiTier::relight_rounds`] frames.

use std::collections::BTreeSet;

use torque_render_core::wgpu;

use super::GiTier;
use super::clipmap::{Clipmap, TileKey, tile_box};
use super::voxels::{ROW, VoxelGpu};
use crate::shaders::Permutation;
use crate::world::LIGHT_BYTES;

/// The light pass's uniform (`LightParams`).
const PARAMS_BYTES: u64 = 64;

/// What the lights are this frame: the bytes of the forward pass's lights buffer, how many, where the area lights begin, the
/// glowing surfaces' lights (from, to), and the sun (toward it and its linear light; None when it does not light).
pub(crate) struct GiLights<'a> {
    pub(crate) bytes: &'a [u8],
    pub(crate) count: u32,
    pub(crate) area_start: u32,
    pub(crate) glow: (u32, u32),
    pub(crate) sun: Option<([f32; 3], [f32; 3])>,
}

/// The tiles waiting to be lit again, the round robin's place and what the lights were.
#[derive(Default)]
pub(crate) struct Relight {
    queue: BTreeSet<TileKey>,
    cursor: Option<TileKey>,
    last_lights: Vec<u8>,
    last_sun: Option<([f32; 3], [f32; 3])>,
}

/// A light's sphere of reach (its place and radius) from its bytes in the lights' buffer.
fn sphere(bytes: &[u8]) -> Option<([f32; 3], f32)> {
    let f = |i: usize| -> Option<f32> {
        Some(f32::from_le_bytes(
            bytes.get(4 * i..4 * i + 4)?.try_into().ok()?,
        ))
    };
    Some(([f(0)?, f(1)?, f(2)?], f(3)?))
}

fn meets_sphere(b: &super::sources::Aabb, c: [f32; 3], r: f32) -> bool {
    let d2: f32 = (0..3)
        .map(|k| {
            let d = (b[0][k] - c[k]).max(0.0).max(c[k] - b[1][k]);
            d * d
        })
        .sum();
    d2 <= r * r
}

impl Relight {
    pub(crate) fn reset(&mut self) {
        *self = Relight::default();
    }

    /// The frame's tiles to light: `made` (the tiles made this frame, with triangles) first, then those a light change
    /// queued, nearest `eye` first, then the round robin's share, at most `budget` past `made` (None: every queued tile and
    /// every tile in turn).
    pub(crate) fn take(
        &mut self,
        clipmap: &Clipmap,
        made: &[TileKey],
        lights: &GiLights<'_>,
        eye: [f32; 3],
        tier: GiTier,
        converge: bool,
    ) -> Vec<TileKey> {
        let solid = &clipmap.solid;
        // The lights that changed mark the tiles in their reach (where they were and where they are).
        let n = (lights.count as usize).min(lights.bytes.len() / LIGHT_BYTES);
        let old_n = self.last_lights.len() / LIGHT_BYTES;
        for i in 0..n.max(old_n) {
            let new = lights
                .bytes
                .get(i * LIGHT_BYTES..(i + 1) * LIGHT_BYTES)
                .filter(|_| i < n);
            let old = self.last_lights.get(i * LIGHT_BYTES..(i + 1) * LIGHT_BYTES);
            if new == old {
                continue;
            }
            for (c, r) in [new, old].into_iter().flatten().filter_map(sphere) {
                self.queue
                    .extend(solid.iter().filter(|k| meets_sphere(&tile_box(**k), c, r)));
            }
        }
        self.last_lights = lights.bytes[..n * LIGHT_BYTES].to_vec();
        if self.last_sun != lights.sun {
            self.queue.extend(solid.iter().copied());
            self.last_sun = lights.sun;
        }
        self.queue.retain(|k| solid.contains(k));
        let mut out: Vec<TileKey> = made.to_vec();
        let mut taken: BTreeSet<TileKey> = made.iter().copied().collect();
        let budget = if converge {
            usize::MAX
        } else {
            tier.relight_budget
        };
        let dist = |k: &TileKey| {
            let b = tile_box(*k);
            (0..3)
                .map(|a| {
                    let d = 0.5 * (b[0][a] + b[1][a]) - eye[a];
                    d * d
                })
                .sum::<f32>()
        };
        let mut queued: Vec<(f32, TileKey)> = self
            .queue
            .iter()
            .filter(|k| !taken.contains(k))
            .map(|k| (dist(k), *k))
            .collect();
        queued.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        for (_, k) in queued.into_iter().take(budget) {
            self.queue.remove(&k);
            taken.insert(k);
            out.push(k);
        }
        // The round robin: the next share of every tile with triangles after the last one lit in turn.
        let share = if converge {
            solid.len()
        } else {
            solid.len().div_ceil(tier.relight_rounds as usize)
        };
        let start = self
            .cursor
            .and_then(|c| {
                solid
                    .range((std::ops::Bound::Excluded(c), std::ops::Bound::Unbounded))
                    .next()
            })
            .copied();
        let ring = start
            .into_iter()
            .flat_map(|s| solid.range(s..))
            .chain(solid.iter())
            .take(share);
        for &k in ring {
            self.cursor = Some(k);
            if taken.insert(k) {
                self.queue.remove(&k);
                out.push(k);
            }
        }
        out
    }
}

/// What the light pass's group binds besides its own buffers: the lights, the list's size, the probes' state and atlas, the
/// sky's coefficients.
type LightKey = (
    wgpu::Buffer,
    u64,
    wgpu::Buffer,
    wgpu::TextureView,
    wgpu::Buffer,
);

/// The probes and the sky the bounce reads.
pub(crate) struct Bounce<'a> {
    pub(crate) state: &'a wgpu::Buffer,
    pub(crate) atlas: &'a wgpu::TextureView,
    pub(crate) sampler: &'a wgpu::Sampler,
    pub(crate) sky: &'a wgpu::Buffer,
}

/// The light pass's pipeline, its uniform, the frame's list of tiles and its group.
pub(crate) struct LightGpu {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
    params: wgpu::Buffer,
    list: wgpu::Buffer,
    group: Option<(LightKey, wgpu::BindGroup)>,
    count: u32,
}

impl LightGpu {
    pub(crate) fn new(device: &wgpu::Device, cache: Option<&wgpu::PipelineCache>) -> LightGpu {
        let compute = wgpu::ShaderStages::COMPUTE;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd gi light"),
            entries: &[
                crate::uniform(0, compute),
                crate::uniform(1, compute),
                crate::storage(2, true, compute),
                crate::storage(3, true, compute),
                crate::storage(4, true, compute),
                crate::storage(5, false, compute),
                crate::storage(6, true, compute),
                // The probes (the bounce) and the sky past them.
                crate::storage(7, true, compute),
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: compute,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 9,
                    visibility: compute,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                crate::uniform(10, compute),
            ],
        });
        let module = crate::module(device, Permutation::GiLight);
        let pipeline =
            crate::compute_filter(device, cache, "hd gi light", &layout, &module, "cs_light");
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        LightGpu {
            layout,
            pipeline,
            params: buffer(
                "hd gi light params",
                PARAMS_BYTES,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            ),
            list: buffer(
                "hd gi light list",
                4096 * 16,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            ),
            group: None,
            count: 0,
        }
    }

    /// Writes the frame's tiles and the lights' uniform (`bounce`: the probes' ambient is added).
    pub(crate) fn write(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        tiles: &[TileKey],
        lights: &GiLights<'_>,
        bounce: bool,
    ) {
        self.count = tiles.len() as u32;
        let mut p = Vec::with_capacity(PARAMS_BYTES as usize);
        let (dir, sun, on) = match lights.sun {
            Some((d, s)) => (d, s, 1.0),
            None => ([0.0, 0.0, 1.0], [0.0; 3], 0.0),
        };
        crate::push_f32(
            &mut p,
            &[dir[0], dir[1], dir[2], on, sun[0], sun[1], sun[2], 0.0],
        );
        crate::push_u32(
            &mut p,
            &[
                lights.count,
                lights.area_start,
                lights.glow.0,
                lights.glow.1,
                self.count,
                u32::from(bounce),
                0,
                0,
            ],
        );
        queue.write_buffer(&self.params, 0, &p);
        if tiles.is_empty() {
            return;
        }
        let mut bytes = Vec::with_capacity(tiles.len() * 16);
        for k in tiles {
            for c in k.tile {
                bytes.extend_from_slice(&c.to_le_bytes());
            }
            bytes.extend_from_slice(&(k.level as i32).to_le_bytes());
        }
        if bytes.len() as u64 > self.list.size() {
            self.list = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("hd gi light list"),
                size: (bytes.len() as u64).next_power_of_two(),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&self.list, 0, &bytes);
    }

    /// Lights the written tiles' voxels.
    pub(crate) fn encode(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        voxels: &VoxelGpu,
        lights: &wgpu::Buffer,
        bounce: &Bounce<'_>,
    ) {
        if self.count == 0 {
            return;
        }
        let key = (
            lights.clone(),
            self.list.size(),
            bounce.state.clone(),
            bounce.atlas.clone(),
            bounce.sky.clone(),
        );
        if self.group.as_ref().is_none_or(|(k, _)| *k != key) {
            let entries = [
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: voxels.volume.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: lights.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: voxels.tiles.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: voxels.occ.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: voxels.voxels.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: self.list.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: bounce.state.as_entire_binding(),
                },
                crate::tv(8, bounce.atlas),
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: wgpu::BindingResource::Sampler(bounce.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: bounce.sky.as_entire_binding(),
                },
            ];
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("hd gi light"),
                layout: &self.layout,
                entries: &entries,
            });
            self.group = Some((key, group));
        }
        let Some((_, group)) = &self.group else {
            return;
        };
        crate::count_pass();
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("hd gi light"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, group, &[]);
        // Eight workgroups a tile (`gi_light.wgsl`'s layers).
        let groups = self.count * 8;
        pass.dispatch_workgroups(groups.min(ROW), groups.div_ceil(ROW), 1);
        self.count = 0;
    }
}
