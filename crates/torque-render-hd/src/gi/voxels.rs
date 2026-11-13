//! The voxel clipmap on the GPU (`gi_common.wgsl`, `gi_voxel.wgsl`): the levels' uniform, every level's tile infos, occupancy
//! words and voxels, and the pass that makes a frame's tiles from the static triangles.

use torque_render_core::wgpu;

use super::clipmap::{LEVEL_TILES, MAX_LEVELS, TILE, TILE_VOXELS, TileKey, voxel_size};
use crate::shaders::Permutation;

/// The levels' uniform (`GiVolume`).
pub(crate) const VOLUME_BYTES: u64 = (MAX_LEVELS as u64) * 32 + 16;
/// A tile's info (its tag, a bit for each quarter of its layers holding a filled voxel), its occupancy words, a voxel's words.
const INFO_BYTES: u64 = 8;
const WORD_BYTES: u64 = 4 * (TILE_VOXELS / 32);
pub(crate) const VOXEL_BYTES: u64 = 16;
/// A job's bytes (`Job`).
const JOB_BYTES: u64 = 32;
/// Workgroups along x of a dispatch (the rest go along y).
pub(crate) const ROW: u32 = 65535;

/// The levels' uniform bytes: each level's low corner (voxels) and voxel size, the levels in use, the frame and the voxel
/// pass's jobs.
pub(crate) fn volume_bytes(
    origins: &[[i32; 3]; MAX_LEVELS],
    levels: u32,
    frame: u32,
    jobs: u32,
) -> Vec<u8> {
    let mut b = Vec::with_capacity(VOLUME_BYTES as usize);
    for (l, o) in origins.iter().enumerate() {
        for c in o {
            b.extend_from_slice(&c.to_le_bytes());
        }
        b.extend_from_slice(&0i32.to_le_bytes());
        let s = voxel_size(l as u32);
        crate::push_f32(&mut b, &[s, 1.0 / s, 0.0, 0.0]);
    }
    crate::push_u32(&mut b, &[levels, frame, jobs, 0]);
    b
}

/// The most of `levels` levels whose voxels one buffer holds within the device's limits (its largest storage binding and
/// buffer: Ultra's four levels are 128 MiB, WebGPU's default binding); 0 when not even one fits.
pub(crate) fn levels_that_fit(device: &wgpu::Device, levels: u32) -> u32 {
    let limits = device.limits();
    let room = limits
        .max_storage_buffer_binding_size
        .min(limits.max_buffer_size);
    let level = LEVEL_TILES * TILE_VOXELS * VOXEL_BYTES;
    levels.min((room / level) as u32)
}

/// The clipmap's buffers and the pass that makes its tiles.
pub(crate) struct VoxelGpu {
    pub(crate) volume: wgpu::Buffer,
    pub(crate) tiles: wgpu::Buffer,
    pub(crate) occ: wgpu::Buffer,
    pub(crate) voxels: wgpu::Buffer,
    jobs: wgpu::Buffer,
    list: wgpu::Buffer,
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
    /// The group, with the triangle buffer's generation and the job buffers' sizes it binds.
    group: Option<((u64, u64, u64), wgpu::BindGroup)>,
    /// The jobs of the frame written (encoded by [`VoxelGpu::encode`]).
    count: u32,
}

fn buffer(
    device: &wgpu::Device,
    label: &str,
    size: u64,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(16),
        usage,
        mapped_at_creation: false,
    })
}

const KEEP: wgpu::BufferUsages = wgpu::BufferUsages::STORAGE
    .union(wgpu::BufferUsages::COPY_DST)
    .union(wgpu::BufferUsages::COPY_SRC);

impl VoxelGpu {
    pub(crate) fn new(
        device: &wgpu::Device,
        cache: Option<&wgpu::PipelineCache>,
        levels: u32,
    ) -> VoxelGpu {
        let l = u64::from(levels);
        let compute = wgpu::ShaderStages::COMPUTE;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd gi voxelize"),
            entries: &[
                crate::uniform(0, compute),
                crate::storage(1, true, compute),
                crate::storage(2, true, compute),
                crate::storage(3, true, compute),
                crate::storage(4, false, compute),
                crate::storage(5, false, compute),
                crate::storage(6, false, compute),
            ],
        });
        let module = crate::module(device, Permutation::GiVoxel);
        let pipeline = crate::compute_filter(
            device,
            cache,
            "hd gi voxelize",
            &layout,
            &module,
            "cs_voxelize",
        );
        VoxelGpu {
            volume: buffer(
                device,
                "hd gi volume",
                VOLUME_BYTES,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            ),
            tiles: buffer(device, "hd gi tiles", l * LEVEL_TILES * INFO_BYTES, KEEP),
            occ: buffer(
                device,
                "hd gi occupancy",
                l * LEVEL_TILES * WORD_BYTES,
                KEEP,
            ),
            voxels: buffer(
                device,
                "hd gi voxels",
                l * LEVEL_TILES * TILE_VOXELS * VOXEL_BYTES,
                KEEP,
            ),
            jobs: buffer(device, "hd gi jobs", 256 * JOB_BYTES, KEEP),
            list: buffer(device, "hd gi job list", 4096 * 4, KEEP),
            layout,
            pipeline,
            group: None,
            count: 0,
        }
    }

    /// The GPU bytes the clipmap holds.
    pub(crate) fn bytes(&self) -> u64 {
        self.tiles.size() + self.occ.size() + self.voxels.size()
    }

    /// Writes the frame's jobs (each tile and its triangles) for [`VoxelGpu::encode`].
    pub(crate) fn write_jobs(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        jobs: &[(TileKey, Vec<u32>)],
    ) {
        self.count = jobs.len() as u32;
        if jobs.is_empty() {
            return;
        }
        let mut job_bytes = Vec::with_capacity(jobs.len() * JOB_BYTES as usize);
        let mut list: Vec<u32> = Vec::new();
        for (k, tris) in jobs {
            let first = list.len() as u32;
            list.extend_from_slice(tris);
            for c in k.tile {
                job_bytes.extend_from_slice(&c.to_le_bytes());
            }
            job_bytes.extend_from_slice(&(k.level as i32).to_le_bytes());
            crate::push_u32(&mut job_bytes, &[first, list.len() as u32 - first, 0, 0]);
        }
        if list.is_empty() {
            list.push(0);
        }
        let need_jobs = job_bytes.len() as u64;
        if need_jobs > self.jobs.size() {
            self.jobs = buffer(device, "hd gi jobs", need_jobs.next_power_of_two(), KEEP);
        }
        let need_list = list.len() as u64 * 4;
        if need_list > self.list.size() {
            self.list = buffer(
                device,
                "hd gi job list",
                need_list.next_power_of_two(),
                KEEP,
            );
        }
        queue.write_buffer(&self.jobs, 0, &job_bytes);
        let mut bytes = Vec::with_capacity(list.len() * 4);
        crate::push_u32(&mut bytes, &list);
        queue.write_buffer(&self.list, 0, &bytes);
    }

    /// Makes the written jobs' tiles from `tris` (of generation `tris_generation`).
    pub(crate) fn encode(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        tris: &wgpu::Buffer,
        tris_generation: u64,
    ) {
        if self.count == 0 {
            return;
        }
        let key = (tris_generation, self.jobs.size(), self.list.size());
        if self.group.as_ref().is_none_or(|(k, _)| *k != key) {
            let entries = [
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.volume.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: tris.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.jobs.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.list.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: self.tiles.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: self.occ.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: self.voxels.as_entire_binding(),
                },
            ];
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("hd gi voxelize"),
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
            label: Some("hd gi voxelize"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, group, &[]);
        // Four workgroups a tile (`gi_voxel.wgsl`'s parts).
        let groups = self.count * 4;
        pass.dispatch_workgroups(groups.min(ROW), groups.div_ceil(ROW), 1);
        self.count = 0;
    }
}

// A level's tiles are its voxels over the tile's.
const _: () = assert!(TILE_VOXELS == 512 && LEVEL_TILES == 4096 && TILE == 8);
