use super::*;

#[test]
fn halves_decode() {
    assert_eq!(f16_to_f32(0x3c00), 1.0);
    assert_eq!(f16_to_f32(0xc000), -2.0);
    assert_eq!(f16_to_f32(0x0001), 2f32.powi(-24));
    assert!(f16_to_f32(0x7c00).is_infinite() && f16_to_f32(0x7e00).is_nan());
    assert_eq!(non_finite(&[[0.0; 4], [f32::NAN, 0.0, 0.0, 0.0]]), 1);
}

#[test]
fn halves_encode_round_to_nearest() {
    // Every half that is a finite number survives the round trip.
    for h in 0u16..=0xffff {
        let f = f16_to_f32(h);
        if f.is_finite() {
            assert_eq!(f16_to_f32(f32_to_f16(f)).to_bits(), f.to_bits(), "{h:#x}");
        }
    }
    // Between two halves the nearer one; past the range infinity.
    let (a, b) = (f16_to_f32(0x3c00), f16_to_f32(0x3c01));
    assert_eq!(f32_to_f16(a + 0.4 * (b - a)), 0x3c00);
    assert_eq!(f32_to_f16(a + 0.6 * (b - a)), 0x3c01);
    assert_eq!(f32_to_f16(1e6), 0x7c00);
    assert_eq!(f32_to_f16(1e-9), 0);
}

#[test]
fn the_inverse_is_an_inverse() {
    #[rustfmt::skip]
    let m = [2.0, 0.0, 0.0, 0.0, 0.0, 3.0, 0.0, 0.0, 0.5, 0.25, 0.001, -1.0, 0.0, 0.0, 0.1, 0.0];
    let inv = invert(&m);
    for r in 0..4 {
        for c in 0..4 {
            let v: f32 = (0..4).map(|k| m[k * 4 + r] * inv[c * 4 + k]).sum();
            assert!(
                (v - if r == c { 1.0 } else { 0.0 }).abs() < 1e-4,
                "{r} {c} {v}"
            );
        }
    }
}

#[test]
fn a_cell_s_indices_go_to_runs_of_16_bits_that_draw_every_triangle_as_it_was() {
    use torque_render_scene::hd::clusters::Cluster;
    // 40,000 quads in one batch drawn as one cluster (a cell built without clusters): 160,000 vertices.
    let quads = 40_000u32;
    let indices: Vec<u32> = (0..quads)
        .flat_map(|q| [0, 1, 2, 0, 2, 3].map(|i| 4 * q + i))
        .collect();
    let one = Cluster {
        slot: 0,
        translucent: false,
        first: 0,
        count: indices.len() as u32,
        bounds: [[0.0; 3], [1.0; 3]],
    };
    let (short, runs) = short_indices(&indices, &[one]);
    assert_eq!(short.len(), indices.len());
    assert_eq!(runs.iter().map(|r| r.2).sum::<u32>(), indices.len() as u32);
    for &(_, first, count, base) in &runs {
        for i in first..first + count {
            assert_eq!(u32::from(short[i as usize]) + base, indices[i as usize]);
        }
    }
    // A triangle whose own vertices are too far apart for 16 bits is left out, the rest kept.
    let far = [0u32, 1, 70_000, 4, 5, 6];
    let cl = Cluster {
        slot: 0,
        translucent: false,
        first: 0,
        count: 6,
        bounds: [[0.0; 3], [1.0; 3]],
    };
    let (_, runs) = short_indices(&far, &[cl]);
    assert_eq!(runs.iter().map(|r| r.2).sum::<u32>(), 3);
}

/// A warm-up dropped unfinished (the window closed while its pipelines are still being made) tells its thread to stop and
/// waits for it: nothing of it runs on after the world is gone, and the drop takes no longer than the thread's current step.
#[test]
fn an_unfinished_warm_up_is_stopped_and_waited_for_when_dropped() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    let stop = Arc::new(AtomicBool::new(false));
    let ended = Arc::new(AtomicBool::new(false));
    let thread = {
        let (stop, ended) = (stop.clone(), ended.clone());
        std::thread::spawn(move || {
            // A warm-up that would run for a minute: a pipeline every 10 ms until told to stop.
            for _ in 0..6000 {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            ended.store(true, Ordering::Relaxed);
            Vec::new()
        })
    };
    let warming = Warming {
        keys: Vec::new(),
        stop,
        thread: Some(thread),
    };
    assert!(!warming.finished());
    let began = std::time::Instant::now();
    drop(warming);
    assert!(ended.load(Ordering::Relaxed), "the thread still runs");
    assert!(
        began.elapsed() < std::time::Duration::from_millis(1000),
        "the drop took {:?}",
        began.elapsed()
    );
}

#[test]
fn a_nearest_shape_material_samples_nearest_and_a_tge_one_as_before() {
    use torque_render_scene::hd::HdDraw;
    use torque_render_scene::shapes::{MAT_NEAREST, material_sampler};
    use torque_render_scene::{Draw, TexEnv};
    let wrap = 0x3;
    let nearest = MAT_NEAREST;
    let key = |flags: u32| {
        let d = Draw {
            label: "shape".into(),
            vertices: Vec::new(),
            lighting: true,
            tex_env: TexEnv::Modulate,
            texture: Some((0, material_sampler(flags))),
            blend: Blend::Opaque,
            cull: Cull::BackCw,
            depth_test: true,
            depth_write: true,
            fog: true,
            lights: Vec::new(),
            vertex_key: None,
        };
        let hd = HdDraw::from_fixed(&d, 0, &[0]);
        SamplerKey::of(&hd.texture.expect("textured").1)
    };
    let k = key(wrap | nearest);
    assert!(!k.mag && !k.min, "nearest near and far");
    assert!(
        k.mip && k.mipmaps && k.repeat,
        "the mipmap levels still blended"
    );
    assert_eq!(
        key(wrap),
        SamplerKey::PLAIN,
        "a TGE material: linear, as before"
    );
}

#[test]
fn a_table_of_mesh_places_past_the_largest_buffer_leaves_the_loose_draws_out() {
    let gpu = match torque_render_core::Gpu::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let mut world: HdWorld<crate::ext::NoExtension> = HdWorld::new(&gpu);
    assert!(world.nodes_fit(0) && world.nodes_fit(1000));
    world.set_buffer_cap(Some((NODE_BYTES * 200) as u64));
    // The table and its room to grow (a quarter and 64 more) within the cap, or not: the upload then leaves out every loose
    // draw before the table is touched, so no draw reads places of another frame.
    assert!(world.nodes_fit(100));
    assert!(!world.nodes_fit(120));
    assert!(world.nodes_fit(0));
}
