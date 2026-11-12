//! What the enhanced renderer's pipelines need of the device's limits, checked before they are made.
//!
//! The forward and water pipelines bind the frame, texture, sky and reflection groups together, and the volumetric pass and the
//! clouds' march the frame, their own and the sky group: more sampled textures in one stage than WebGPU's default of 16 (which an
//! adapter in a browser reports). Each pipeline's groups are counted here from the same binding lists the layouts are made from,
//! so the count cannot drift from them. [`check`] compares it with a device's limits and names each one that falls short; the
//! renderer's constructors ([`crate::HdWorld::try_new`]) return that as an error so a caller can fall back to a renderer that
//! needs less.

use std::fmt;

use torque_render_core::wgpu;

/// The bindings of each kind a pipeline has in one shader stage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StageCounts {
    pub sampled_textures: u32,
    pub samplers: u32,
    pub storage_buffers: u32,
    pub uniform_buffers: u32,
    pub storage_textures: u32,
}

/// A pipeline layout's bindings in each of its stages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PipelineNeeds {
    pub name: &'static str,
    pub vertex: StageCounts,
    pub fragment: StageCounts,
    pub compute: StageCounts,
}

/// The bindings `groups` (one pipeline layout's) have in `stage`.
pub fn count(groups: &[&[wgpu::BindGroupLayoutEntry]], stage: wgpu::ShaderStages) -> StageCounts {
    let mut c = StageCounts::default();
    for e in groups.iter().flat_map(|g| g.iter()) {
        if !e.visibility.intersects(stage) {
            continue;
        }
        // A binding array counts against wgpu's `max_binding_array_elements_per_shader_stage`, not these: none is counted.
        if e.count.is_some() {
            continue;
        }
        match e.ty {
            wgpu::BindingType::Texture { .. } => c.sampled_textures += 1,
            wgpu::BindingType::Sampler(_) => c.samplers += 1,
            wgpu::BindingType::StorageTexture { .. } => c.storage_textures += 1,
            wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                ..
            } => c.uniform_buffers += 1,
            wgpu::BindingType::Buffer { .. } => c.storage_buffers += 1,
            _ => {}
        }
    }
    c
}

/// The groups of each pipeline layout that binds several of the big groups together, as the renderer makes them.
pub(crate) fn pipeline_groups() -> Vec<(&'static str, Vec<Vec<wgpu::BindGroupLayoutEntry>>)> {
    let frame = crate::world::frame_entries();
    let texture = crate::world::texture_entries();
    let sky = crate::sky::group_entries();
    vec![
        (
            "forward and water",
            vec![
                frame.clone(),
                texture,
                sky.clone(),
                crate::world::reflect::layout_entries(),
            ],
        ),
        (
            "volumetric light",
            vec![
                frame.clone(),
                crate::volumetric::layout_entries(),
                sky.clone(),
            ],
        ),
        (
            "froxel volumetrics",
            vec![frame.clone(), crate::froxels::layout_entries(), sky.clone()],
        ),
        (
            "froxel composite",
            vec![
                frame.clone(),
                crate::froxels::composite_entries(),
                sky.clone(),
            ],
        ),
        (
            "clouds' march",
            vec![frame, crate::clouds::march_entries(), sky],
        ),
    ]
}

/// What each of those pipeline layouts needs per stage.
pub fn pipeline_needs() -> Vec<PipelineNeeds> {
    pipeline_groups()
        .into_iter()
        .map(|(name, groups)| {
            let groups: Vec<&[wgpu::BindGroupLayoutEntry]> =
                groups.iter().map(Vec::as_slice).collect();
            PipelineNeeds {
                name,
                vertex: count(&groups, wgpu::ShaderStages::VERTEX),
                fragment: count(&groups, wgpu::ShaderStages::FRAGMENT),
                compute: count(&groups, wgpu::ShaderStages::COMPUTE),
            }
        })
        .collect()
}

/// One limit the device falls short of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shortfall {
    /// The limit's name in WebGPU (`max_sampled_textures_per_shader_stage`).
    pub limit: &'static str,
    /// What the pipelines need of it.
    pub need: u32,
    /// What the device allows.
    pub have: u32,
    /// The pipeline layout that needs most of it, and the stage.
    pub pipeline: &'static str,
    pub stage: &'static str,
}

/// The limits the device falls short of for the enhanced renderer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LimitError {
    pub shortfalls: Vec<Shortfall>,
}

impl fmt::Display for LimitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "this graphics device is too limited for the enhanced renderer: "
        )?;
        for (i, s) in self.shortfalls.iter().enumerate() {
            if i > 0 {
                write!(f, "; ")?;
            }
            write!(
                f,
                "{} needs {} (the {} pipelines, {} stage) but the device allows {}",
                s.limit, s.need, s.pipeline, s.stage, s.have
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for LimitError {}

/// Checks `needs` against `limits`: Ok, or each limit that is short (with the pipeline that needs most of it).
pub fn check_needs(needs: &[PipelineNeeds], limits: &wgpu::Limits) -> Result<(), LimitError> {
    type Pick = fn(&StageCounts) -> u32;
    let kinds: [(&'static str, u32, Pick); 5] = [
        (
            "max_sampled_textures_per_shader_stage",
            limits.max_sampled_textures_per_shader_stage,
            |c| c.sampled_textures,
        ),
        (
            "max_samplers_per_shader_stage",
            limits.max_samplers_per_shader_stage,
            |c| c.samplers,
        ),
        (
            "max_storage_buffers_per_shader_stage",
            limits.max_storage_buffers_per_shader_stage,
            |c| c.storage_buffers,
        ),
        (
            "max_uniform_buffers_per_shader_stage",
            limits.max_uniform_buffers_per_shader_stage,
            |c| c.uniform_buffers,
        ),
        (
            "max_storage_textures_per_shader_stage",
            limits.max_storage_textures_per_shader_stage,
            |c| c.storage_textures,
        ),
    ];
    let mut shortfalls = Vec::new();
    for (limit, have, pick) in kinds {
        let worst = needs
            .iter()
            .flat_map(|p| {
                [
                    ("vertex", p.vertex),
                    ("fragment", p.fragment),
                    ("compute", p.compute),
                ]
                .map(|(stage, c)| (pick(&c), p.name, stage))
            })
            .max_by_key(|(n, ..)| *n);
        if let Some((need, pipeline, stage)) = worst
            && need > have
        {
            shortfalls.push(Shortfall {
                limit,
                need,
                have,
                pipeline,
                stage,
            });
        }
    }
    if shortfalls.is_empty() {
        Ok(())
    } else {
        Err(LimitError { shortfalls })
    }
}

/// [`check_needs`] over the renderer's own pipeline layouts.
pub fn check(limits: &wgpu::Limits) -> Result<(), LimitError> {
    check_needs(&pipeline_needs(), limits)
}

#[cfg(test)]
#[path = "limits_tests.rs"]
mod tests;
