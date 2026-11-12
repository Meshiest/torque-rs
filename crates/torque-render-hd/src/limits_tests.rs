use super::*;

const FRAGMENT: wgpu::ShaderStages = wgpu::ShaderStages::FRAGMENT;

fn texture(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

#[test]
fn bindings_are_counted_by_kind_in_the_stages_that_see_them() {
    let a = vec![
        crate::uniform(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
        crate::storage(1, true, FRAGMENT),
        crate::storage(2, true, wgpu::ShaderStages::VERTEX),
        texture(3, FRAGMENT),
        sampler(4, wgpu::ShaderStages::COMPUTE),
    ];
    let b = vec![
        texture(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
        sampler(1, FRAGMENT),
        crate::storage_target(2, wgpu::TextureFormat::Rgba8Unorm),
    ];
    let groups: [&[wgpu::BindGroupLayoutEntry]; 2] = [&a, &b];
    assert_eq!(
        count(&groups, FRAGMENT),
        StageCounts {
            sampled_textures: 2,
            samplers: 1,
            storage_buffers: 1,
            uniform_buffers: 1,
            storage_textures: 0,
        }
    );
    assert_eq!(
        count(&groups, wgpu::ShaderStages::VERTEX),
        StageCounts {
            sampled_textures: 1,
            storage_buffers: 1,
            uniform_buffers: 1,
            ..StageCounts::default()
        }
    );
    assert_eq!(
        count(&groups, wgpu::ShaderStages::COMPUTE),
        StageCounts {
            samplers: 1,
            storage_textures: 1,
            ..StageCounts::default()
        }
    );
}

#[test]
fn a_device_with_webgpus_default_limits_is_too_small_and_the_error_says_which_limit() {
    let err = check(&wgpu::Limits::default()).expect_err("the forward pipelines need more than 16");
    let s = err
        .shortfalls
        .iter()
        .find(|s| s.limit == "max_sampled_textures_per_shader_stage")
        .expect("the sampled textures are short");
    assert_eq!(s.have, 16);
    assert!(s.need > 16, "needs {}", s.need);
    assert_eq!((s.pipeline, s.stage), ("forward and water", "fragment"));
    let text = err.to_string();
    assert!(
        text.contains("max_sampled_textures_per_shader_stage needs")
            && text.contains(&format!("needs {} ", s.need))
            && text.contains("the device allows 16"),
        "{text}"
    );
}

#[test]
fn limits_at_what_the_pipelines_need_pass_and_one_less_fails() {
    let needs = pipeline_needs();
    let worst = |pick: fn(&StageCounts) -> u32| {
        needs
            .iter()
            .flat_map(|p| [pick(&p.vertex), pick(&p.fragment), pick(&p.compute)])
            .max()
            .unwrap_or(0)
    };
    let exact = wgpu::Limits {
        max_sampled_textures_per_shader_stage: worst(|c| c.sampled_textures),
        max_samplers_per_shader_stage: worst(|c| c.samplers),
        max_storage_buffers_per_shader_stage: worst(|c| c.storage_buffers),
        max_uniform_buffers_per_shader_stage: worst(|c| c.uniform_buffers),
        max_storage_textures_per_shader_stage: worst(|c| c.storage_textures),
        ..wgpu::Limits::default()
    };
    assert_eq!(check_needs(&needs, &exact), Ok(()));
    let one_less = |f: fn(&mut wgpu::Limits)| {
        let mut l = exact.clone();
        f(&mut l);
        check_needs(&needs, &l).expect_err("one short")
    };
    let e = one_less(|l| l.max_sampled_textures_per_shader_stage -= 1);
    assert_eq!(e.shortfalls.len(), 1);
    assert_eq!(
        e.shortfalls[0].limit,
        "max_sampled_textures_per_shader_stage"
    );
    let e = one_less(|l| l.max_samplers_per_shader_stage -= 1);
    assert_eq!(e.shortfalls[0].limit, "max_samplers_per_shader_stage");
    let e = one_less(|l| l.max_storage_buffers_per_shader_stage -= 1);
    assert_eq!(
        e.shortfalls[0].limit,
        "max_storage_buffers_per_shader_stage"
    );
    let e = one_less(|l| l.max_uniform_buffers_per_shader_stage -= 1);
    assert_eq!(
        e.shortfalls[0].limit,
        "max_uniform_buffers_per_shader_stage"
    );
}

/// Whether making `groups` as bind group layouts and one pipeline layout over them is a validation error on a device whose
/// sampled-texture limit is `limit`.
fn refused(
    adapter: &wgpu::Adapter,
    groups: &[Vec<wgpu::BindGroupLayoutEntry>],
    limit: u32,
) -> bool {
    let (device, _queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("limits test"),
        required_limits: wgpu::Limits {
            max_sampled_textures_per_shader_stage: limit,
            ..wgpu::Limits::default()
        },
        ..wgpu::DeviceDescriptor::default()
    }))
    .expect("device");
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let layouts: Vec<wgpu::BindGroupLayout> = groups
        .iter()
        .map(|entries| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("limits test"),
                entries,
            })
        })
        .collect();
    let refs: Vec<Option<&wgpu::BindGroupLayout>> = layouts.iter().map(Some).collect();
    let _ = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("limits test"),
        bind_group_layouts: &refs,
        immediate_size: 0,
    });
    pollster::block_on(scope.pop()).is_some()
}

#[test]
fn the_count_is_what_the_device_makes_of_the_real_layouts() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        flags: torque_render_core::instance_flags(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..wgpu::RequestAdapterOptions::default()
    })) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("skipped: no GPU ({e})");
            return;
        }
    };
    let have = adapter.limits().max_sampled_textures_per_shader_stage;
    for ((name, groups), needs) in pipeline_groups().into_iter().zip(pipeline_needs()) {
        let need = needs
            .vertex
            .sampled_textures
            .max(needs.fragment.sampled_textures)
            .max(needs.compute.sampled_textures);
        if need > have {
            eprintln!("{name}: the adapter allows only {have} (needs {need})");
            continue;
        }
        assert!(
            !refused(&adapter, &groups, need),
            "{name}: the device refuses the layouts at the {need} sampled textures counted"
        );
        assert!(
            refused(&adapter, &groups, need - 1),
            "{name}: the device takes the layouts at {} sampled textures, one under the {need} counted",
            need - 1
        );
    }
}

#[test]
fn the_froxel_gather_fits_webgpus_default_compute_limits() {
    let d = wgpu::Limits::default();
    let needs = pipeline_needs();
    let gather = needs
        .iter()
        .find(|p| p.name == "froxel volumetrics")
        .expect("the froxel gather is counted");
    let c = gather.compute;
    assert!(c.storage_textures >= 1 && c.storage_buffers >= 2, "{c:?}");
    assert!(
        c.sampled_textures <= d.max_sampled_textures_per_shader_stage,
        "{c:?}"
    );
    assert!(c.samplers <= d.max_samplers_per_shader_stage, "{c:?}");
    assert!(
        c.storage_buffers <= d.max_storage_buffers_per_shader_stage,
        "{c:?}"
    );
    assert!(
        c.uniform_buffers <= d.max_uniform_buffers_per_shader_stage,
        "{c:?}"
    );
    assert!(
        c.storage_textures <= d.max_storage_textures_per_shader_stage,
        "{c:?}"
    );
}
