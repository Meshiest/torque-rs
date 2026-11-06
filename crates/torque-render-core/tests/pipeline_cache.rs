//! The pipeline cache's file and a device started from it (the device tests are skipped with a note when the machine has no
//! Vulkan adapter with pipeline caches): a cache saved and loaded again is accepted, and a truncated, damaged or another
//! adapter's file starts an empty cache without a panic.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use torque_render_core::blur::Blur;
use torque_render_core::pipeline_cache::{self, encode, read_file, write_file};
use torque_render_core::{Gpu, GuiPass, wgpu};

/// A fresh directory of the test's own under cargo's temporary directory for tests.
fn test_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("pipeline-cache-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_save_replaces_the_file_and_other_drivers_files() {
    let dir = test_dir("files");
    let old = dir.join("pipelines-k-0000000000000001.bin");
    let keep = dir.join("pipelines-kk-0000000000000001.bin");
    std::fs::write(&old, b"old").unwrap();
    std::fs::write(&keep, b"other key").unwrap();
    let path = dir.join("pipelines-k-0000000000000002.bin");
    // A crashed save's temporary file, an hour old, and one a save is writing now.
    let stale = dir.join("pipelines-k-0000000000000002.bin.1.tmp");
    let fresh = dir.join("pipelines-k-0000000000000002.bin.2.tmp");
    std::fs::write(&stale, b"cut short").unwrap();
    std::fs::write(&fresh, b"being written").unwrap();
    let hour_ago = std::time::SystemTime::now() - Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(&stale)
        .unwrap()
        .set_modified(hour_ago)
        .unwrap();
    write_file(&path, b"one").unwrap();
    assert!(!stale.exists() && fresh.exists());
    std::fs::remove_file(&fresh).unwrap();
    write_file(&path, b"two").unwrap();
    assert_eq!(read_file(&path).as_deref(), Some(&b"two"[..]));
    assert!(!old.exists() && keep.exists());
    let names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(names.len(), 2, "no temporary file is left: {names:?}");
    std::fs::write(&path, &encode(b"two")[..10]).unwrap();
    assert_eq!(read_file(&path), None, "a truncated file is refused");
    let mut flipped = encode(b"two");
    *flipped.last_mut().unwrap() ^= 1;
    std::fs::write(&path, &flipped).unwrap();
    assert_eq!(read_file(&path), None, "a damaged file is refused");
    assert_eq!(read_file(&dir.join("missing.bin")), None);
}

/// A Vulkan device whose cache file is in `dir`, or None (with a note) when there is no Vulkan adapter with pipeline caches.
fn vulkan(dir: &Path) -> Option<Gpu> {
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
    Some(gpu)
}

/// A compute shader of a few loops on `seed`, a different module for every seed (the driver compiles each).
fn shader(seed: u64) -> String {
    format!(
        "@group(0) @binding(0) var<storage, read_write> out: array<vec4<f32>>;
         fn mix3(v: vec4<f32>, k: f32) -> vec4<f32> {{
             var a = v;
             for (var i = 0u; i < 64u; i = i + 1u) {{
                 a = sin(a * k + vec4<f32>(f32(i))) * cos(a.yzwx * {seed}.0) + sqrt(abs(a.zwxy) + 1.0);
                 if (a.x > a.y) {{ a = a.wzyx * 0.5; }} else {{ a = normalize(a + vec4<f32>(0.25)); }}
             }}
             return a;
         }}
         @compute @workgroup_size(64)
         fn main(@builtin(global_invocation_id) id: vec3<u32>) {{
             let i = id.x;
             var v = out[i];
             for (var j = 0u; j < 8u; j = j + 1u) {{
                 v = mix3(v, f32(j) * 0.37 + {seed}.0);
             }}
             out[i] = v;
         }}"
    )
}

/// Makes the test's pipelines on `gpu` with its cache (the GUI pass, the blur and 24 compute pipelines of `nonce`'s
/// shaders), waiting for the device: how long they took.
fn make_pipelines(gpu: &Gpu, nonce: u64) -> Duration {
    let start = Instant::now();
    let _gui = GuiPass::new(gpu, None);
    let _blur = Blur::new(gpu);
    let device = gpu.device();
    let pipelines: Vec<_> = (0..24)
        .map(|k| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("cache test"),
                source: wgpu::ShaderSource::Wgsl(shader(nonce * 100 + k).into()),
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("cache test"),
                layout: None,
                module: &module,
                entry_point: Some("main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: gpu.pipeline_cache(),
            })
        })
        .collect();
    gpu.finish().unwrap();
    drop(pipelines);
    start.elapsed()
}

/// Shaders no earlier run has made (the driver's own cache has none of them).
fn nonce() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |d| d.as_nanos() as u64 % 1_000_000 + 1)
}

#[test]
fn a_saved_cache_is_loaded_again_and_warms_the_pipelines() {
    let dir = test_dir("device");
    let Some(gpu) = vulkan(&dir) else {
        return;
    };
    assert_eq!(gpu.pipeline_cache_loaded(), None, "no file yet");
    let nonce = nonce();
    let cold = make_pipelines(&gpu, nonce);
    let written = gpu.save_pipeline_cache().unwrap().expect("a cache file");
    assert!(written > 0);
    drop(gpu);

    let again = vulkan(&dir).unwrap();
    let loaded = again
        .pipeline_cache_loaded()
        .expect("the saved cache is accepted");
    assert_eq!(loaded, written);
    let warm = make_pipelines(&again, nonce);
    eprintln!(
        "{}: cold {cold:.2?}, warm {warm:.2?}, {written} bytes",
        again.adapter()
    );
}

#[test]
fn a_bad_file_starts_an_empty_cache() {
    let dir = test_dir("bad");
    let Some(gpu) = vulkan(&dir) else {
        return;
    };
    make_pipelines(&gpu, 7);
    gpu.save_pipeline_cache().unwrap().expect("a cache file");
    let name = pipeline_cache::file_name(gpu.adapter_info()).unwrap();
    drop(gpu);
    let path = dir.join(name);
    let good = read_file(&path).unwrap();
    // wgpu's header in the data: the magic (0..8), then the adapter key (17..32, the vendor and device) and the driver's key
    // (32..48). Each altered copy is written whole with a good file header, so only wgpu's checks can refuse it.
    let altered = |at: usize| {
        let mut d = good.clone();
        d[at] ^= 0xff;
        d
    };
    let cases: [(&str, Vec<u8>); 6] = [
        ("truncated", encode(&good)[..good.len() / 2].to_vec()),
        ("damaged", {
            let mut f = encode(&good);
            *f.last_mut().unwrap() ^= 1;
            f
        }),
        ("not wgpu's", encode(&altered(0))),
        ("another adapter's", encode(&altered(20))),
        ("another driver's", encode(&altered(40))),
        ("wgpu's header only", encode(&good[..48])),
    ];
    for (what, bytes) in cases {
        std::fs::write(&path, &bytes).unwrap();
        let gpu = vulkan(&dir).unwrap();
        assert_eq!(gpu.pipeline_cache_loaded(), None, "{what}");
        make_pipelines(&gpu, 7);
        assert!(
            gpu.save_pipeline_cache().unwrap().is_some(),
            "{what}: the empty cache is saved over it"
        );
    }
}
