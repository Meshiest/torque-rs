//! How long the enhanced renderer's pipelines take to make with an empty pipeline cache and with the one it saved (a Vulkan
//! adapter with pipeline caches; skipped with a note without one). A measurement, run on its own:
//! `cargo test --release -p torque-render-hd --test main pipeline_cache:: -- --ignored --nocapture`. A driver that keeps its own
//! cache per program (NVIDIA's) has it cold only on a program's first run, such as the first after a build.

use std::path::Path;
use std::time::{Duration, Instant};

use torque_render_core::{Gpu, wgpu};
use torque_render_hd::HdWorld;
use torque_render_hd::ext::NoExtension;

/// The renderer on a Vulkan device whose cache file is in `dir`, its pipelines all made (the view's, the warm-up's and
/// [`HdWorld::prewarm`]'s): the device, how many bytes of the file it loaded, and how long the pipelines took.
fn build(dir: &Path) -> Option<(Gpu, Option<usize>, Duration)> {
    let gpu = match Gpu::with_backends_cached(&[wgpu::Backends::VULKAN], Some(dir)) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipped: {e}");
            return None;
        }
    };
    if gpu.pipeline_cache().is_none() {
        eprintln!("skipped: {} has no pipeline cache", gpu.adapter());
        return None;
    }
    let start = Instant::now();
    let mut world = HdWorld::<NoExtension>::new(&gpu);
    world.warm_effects();
    world.prewarm();
    gpu.finish().unwrap();
    let took = start.elapsed();
    drop(world);
    let loaded = gpu.pipeline_cache_loaded();
    Some((gpu, loaded, took))
}

#[test]
#[ignore = "a measurement; run with --ignored --nocapture"]
fn the_saved_cache_makes_the_pipelines_faster() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("hd-pipeline-cache");
    let _ = std::fs::remove_dir_all(&dir);
    let Some((gpu, loaded, cold)) = build(&dir) else {
        return;
    };
    assert_eq!(loaded, None);
    let written = gpu.save_pipeline_cache().unwrap().expect("a cache file");
    let adapter = gpu.adapter();
    drop(gpu);
    let (_gpu, loaded, warm) = build(&dir).unwrap();
    assert_eq!(loaded, Some(written));
    eprintln!(
        "{adapter}: empty cache {cold:.2?}, saved cache {warm:.2?} ({} KiB)",
        written.div_ceil(1024)
    );
}
