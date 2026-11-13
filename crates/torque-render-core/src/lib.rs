//! The renderers' device layer: a [`Gpu`] (the wgpu adapter, device and queue), the backends it may start on and their order,
//! the sample counts its formats support, [`Offscreen`] colour and depth targets, a target's frame read back ([`Frame`]), a
//! window's surface ([`window`]) and the GUI pass ([`GuiPass`], with its backdrop blur [`blur::Blur`]). Every renderer draws on
//! a [`Gpu`]; nothing here draws a scene. A [`Gpu`] on Vulkan keeps its pipelines in a cache saved between runs
//! ([`pipeline_cache`]; the crate's one `unsafe` block, the only one in the workspace, is there).
//!
//! # Maintaining
//!
//! Rules a change must keep, each guarded by a test:
//! - Every device has an uncaptured-error handler that counts and prints ([`DeviceErrors`]), never wgpu's panic
//!   (`a_buffer_past_the_device_limit_is_a_counted_error_not_a_panic`).
//!
//! Costs a frame must not grow back: the GUI pass writes four vertices a quad into its kept byte list
//! ([`gui_pass::vertex_bytes`]) and its shared quad indices only when they grow (bench `gui pass vertices 3k quads`, timed
//! under the bench lock).

use std::fmt;
use std::path::Path;

pub mod blur;
pub mod gui_pass;
pub mod pipeline_cache;
pub mod window;
pub use gui_pass::GuiPass;
/// The GPU crate, for a window's surface.
pub use wgpu;

/// The name a display device list shows for a backend (the windowed client's Options list the one it draws with).
pub fn backend_name(backend: wgpu::Backend) -> &'static str {
    match backend {
        wgpu::Backend::Dx12 => "Direct3D 12",
        wgpu::Backend::Vulkan => "Vulkan",
        wgpu::Backend::Metal => "Metal",
        wgpu::Backend::Gl => "OpenGL",
        wgpu::Backend::BrowserWebGpu => "WebGPU",
        _ => "Unknown",
    }
}

/// The colour and depth formats of every target made here.
pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// The device's memory blocks: 8 MiB at first, at most 64 MiB, a larger resource in a block of its own (freed with it). The
/// default's 256 MiB blocks each kept a few small resources alive after the large ones in them were freed: a big scene's
/// renderers held 1.3 to 2 GiB of them free in blocks that could not be given back.
pub const MEMORY_HINTS: wgpu::MemoryHints = wgpu::MemoryHints::Manual {
    suballocated_device_memory_block_size: (8 << 20)..(64 << 20),
};
/// The largest frame the renderer makes (a side).
pub const MAX_SIDE: u32 = 8192;
/// The sample counts a multisampled scene pass may use (wgpu's; 1 is the single-sampled path).
pub const SAMPLE_COUNTS: [u32; 5] = [1, 2, 4, 8, 16];

/// The sample counts of [`SAMPLE_COUNTS`] a device can draw the scene pass with: those both the colour and the depth format support
/// on it, the colour resolvable (with `TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES` the adapter's own, else WebGPU's guaranteed 1 and 4).
fn supported_sample_counts(adapter: &wgpu::Adapter, features: wgpu::Features) -> Vec<u32> {
    let flags = |format: wgpu::TextureFormat| {
        if features.contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES) {
            adapter.get_texture_format_features(format).flags
        } else {
            format.guaranteed_format_features(features).flags
        }
    };
    let (color, depth) = (flags(COLOR_FORMAT), flags(DEPTH_FORMAT));
    SAMPLE_COUNTS
        .into_iter()
        .filter(|&n| {
            n == 1
                || (color.sample_count_supported(n)
                    && depth.sample_count_supported(n)
                    && color.contains(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_RESOLVE))
        })
        .collect()
}

/// The count a multisampled pass picks: the largest of `supported` at or under `wanted` (0 and 1 are one sample).
pub fn pick_samples(supported: &[u32], wanted: u32) -> u32 {
    supported
        .iter()
        .copied()
        .filter(|&n| n <= wanted.max(1))
        .max()
        .unwrap_or(1)
}

/// The optional features the HDR renderer uses when the adapter has them: GPU timestamps (its frame times),
/// timestamps between passes (per-pass GPU times), `Rg11b10Ufloat` render targets (bloom), depth clip control (shadow pancaking),
/// filterable 32-bit float textures, for its indirect draws a first instance in the arguments (a draw names its cluster)
/// and the draw count read from a buffer, and BC textures (the physically based materials' maps go up block-compressed).
/// The device asks for those of them the adapter offers; a single-sampled frame depends on none.
pub fn hd_wanted() -> wgpu::Features {
    wgpu::Features::TIMESTAMP_QUERY
        | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS
        | wgpu::Features::RG11B10UFLOAT_RENDERABLE
        | wgpu::Features::DEPTH_CLIP_CONTROL
        | wgpu::Features::FLOAT32_FILTERABLE
        | wgpu::Features::INDIRECT_FIRST_INSTANCE
        | wgpu::Features::MULTI_DRAW_INDIRECT_COUNT
        | wgpu::Features::TEXTURE_COMPRESSION_BC
}

/// Why a frame could not be drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderError {
    NoAdapter(String),
    NoDevice(String),
    BadSize(u32, u32),
    Readback(String),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenderError::NoAdapter(e) => write!(f, "no GPU adapter: {e}"),
            RenderError::NoDevice(e) => write!(f, "no GPU device: {e}"),
            RenderError::BadSize(w, h) => write!(f, "a frame of {w}x{h} cannot be drawn"),
            RenderError::Readback(e) => write!(f, "reading the frame back failed: {e}"),
        }
    }
}

impl std::error::Error for RenderError {}

/// An RGBA8 frame, rows top to bottom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Frame {
    /// The RGB bytes (alpha dropped).
    pub fn rgb(&self) -> Vec<u8> {
        self.rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect()
    }
}

/// A colour and depth target of one size (`Rgba8Unorm`, no sRGB, and 32-bit float depth, as every frame is drawn).
pub struct Offscreen {
    pub width: u32,
    pub height: u32,
    texture: wgpu::Texture,
    pub color: wgpu::TextureView,
    pub depth: wgpu::TextureView,
}

/// The flags every instance is made with: wgpu's build defaults, and in a release build without the objects' labels handed to the
/// driver (a frame's hundred command lists each named cost a few percent of its encode; `WGPU_DISCARD_HAL_LABELS=0` keeps them
/// for a graphics debugger).
pub fn instance_flags() -> wgpu::InstanceFlags {
    let mut flags = wgpu::InstanceFlags::from_build_config();
    if !cfg!(debug_assertions) {
        flags |= wgpu::InstanceFlags::DISCARD_HAL_LABELS;
    }
    flags.with_env()
}

/// An adapter, its device and queue, the sample counts its formats support, its downlevel flags and its pipeline cache.
#[derive(Clone)]
pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    info: wgpu::AdapterInfo,
    sample_counts: Vec<u32>,
    downlevel: wgpu::DownlevelFlags,
    cache: Option<pipeline_cache::DeviceCache>,
    errors: std::sync::Arc<DeviceErrors>,
}

/// The most device errors printed; the rest are counted.
pub const MAX_PRINTED_DEVICE_ERRORS: u64 = 16;

/// The errors a device reported that no error scope caught (wgpu's default handler panics on them): counted by kind and the
/// first [`MAX_PRINTED_DEVICE_ERRORS`] printed to stderr. A host that sees [`DeviceErrorCounts::out_of_memory`] rise makes its
/// renderer again.
#[derive(Debug, Default)]
pub struct DeviceErrors {
    validation: std::sync::atomic::AtomicU64,
    out_of_memory: std::sync::atomic::AtomicU64,
    internal: std::sync::atomic::AtomicU64,
    last: std::sync::Mutex<Option<String>>,
}

/// What [`DeviceErrors`] counted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceErrorCounts {
    pub validation: u64,
    pub out_of_memory: u64,
    pub internal: u64,
    /// The last error's text.
    pub last: Option<String>,
}

impl DeviceErrors {
    /// Counts `error` (and prints it while few have been).
    pub fn report(&self, error: &wgpu::Error) {
        use std::sync::atomic::Ordering::Relaxed;
        let counter = match error {
            wgpu::Error::Validation { .. } => &self.validation,
            wgpu::Error::OutOfMemory { .. } => &self.out_of_memory,
            wgpu::Error::Internal { .. } => &self.internal,
        };
        counter.fetch_add(1, Relaxed);
        let text = error.to_string();
        let seen = self.validation.load(Relaxed)
            + self.out_of_memory.load(Relaxed)
            + self.internal.load(Relaxed);
        if seen <= MAX_PRINTED_DEVICE_ERRORS {
            eprintln!("render device error: {text}");
        }
        *self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(text);
    }

    /// The counts so far.
    pub fn counts(&self) -> DeviceErrorCounts {
        use std::sync::atomic::Ordering::Relaxed;
        DeviceErrorCounts {
            validation: self.validation.load(Relaxed),
            out_of_memory: self.out_of_memory.load(Relaxed),
            internal: self.internal.load(Relaxed),
            last: self
                .last
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        }
    }
}

impl Gpu {
    /// The first high-performance adapter (DX12 or Vulkan on Windows), no window.
    pub fn new() -> Result<Gpu, RenderError> {
        Gpu::with_backends(&[
            wgpu::Backends::DX12,
            wgpu::Backends::VULKAN | wgpu::Backends::METAL,
        ])
    }

    /// An offscreen device on the first of `order` that has an adapter, its pipeline cache in [`pipeline_cache::dir`].
    pub fn with_backends(order: &[wgpu::Backends]) -> Result<Gpu, RenderError> {
        Gpu::with_backends_cached(order, pipeline_cache::dir().as_deref())
    }

    /// [`Gpu::with_backends`] with its pipeline cache file in `cache_dir` (None: in memory only).
    pub fn with_backends_cached(
        order: &[wgpu::Backends],
        cache_dir: Option<&Path>,
    ) -> Result<Gpu, RenderError> {
        // DX12 first on Windows, then Vulkan and Metal.
        let mut adapter = None;
        let mut why = String::new();
        for &backends in order {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends,
                flags: instance_flags(),
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
                apply_limit_buckets: false,
            })) {
                Ok(a) => {
                    adapter = Some(a);
                    break;
                }
                Err(e) => why = e.to_string(),
            }
        }
        let adapter = adapter.ok_or(RenderError::NoAdapter(why))?;
        pollster::block_on(Gpu::on_adapter_cached(&adapter, cache_dir))
    }

    /// The device on an adapter the caller chose (a window's: [`window::window_gpu`]). The device asks for the adapter's limits
    /// and for the optional features of [`hd_wanted`] it has (the HDR renderer draws on this device), and for the adapter's
    /// own texture format features when it has them (sample counts beyond WebGPU's 4); single-sampled draws use none
    /// of them.
    pub fn on_adapter(adapter: &wgpu::Adapter) -> Result<Gpu, RenderError> {
        pollster::block_on(Gpu::on_adapter_async(adapter))
    }

    /// [`Gpu::on_adapter`] without blocking (a browser answers the device's request on its own event loop). The features and
    /// limits asked for are only ever the ones the adapter offers. The pipeline cache is [`pipeline_cache::dir`]'s.
    pub async fn on_adapter_async(adapter: &wgpu::Adapter) -> Result<Gpu, RenderError> {
        Gpu::on_adapter_cached(adapter, pipeline_cache::dir().as_deref()).await
    }

    /// [`Gpu::on_adapter_async`] with its pipeline cache file in `cache_dir` (None: in memory only). The device asks for
    /// `PIPELINE_CACHE` when the adapter has it (Vulkan) and starts its cache from the adapter's file there.
    pub async fn on_adapter_cached(
        adapter: &wgpu::Adapter,
        cache_dir: Option<&Path>,
    ) -> Result<Gpu, RenderError> {
        let info = adapter.get_info();
        let mut wanted = hd_wanted() | wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES;
        if !cfg!(target_arch = "wasm32") {
            wanted |= wgpu::Features::PIPELINE_CACHE;
        }
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("render"),
                required_features: adapter.features() & wanted,
                required_limits: adapter.limits(),
                memory_hints: MEMORY_HINTS,
                ..wgpu::DeviceDescriptor::default()
            })
            .await
            .map_err(|e| RenderError::NoDevice(e.to_string()))?;
        let errors = std::sync::Arc::new(DeviceErrors::default());
        let sink = std::sync::Arc::clone(&errors);
        device.on_uncaptured_error(std::sync::Arc::new(move |e: wgpu::Error| sink.report(&e)));
        let sample_counts = supported_sample_counts(adapter, device.features());
        let downlevel = adapter.get_downlevel_capabilities().flags;
        let cache = pipeline_cache::open(&device, &info, cache_dir).await;
        Ok(Gpu {
            device,
            queue,
            info,
            sample_counts,
            downlevel,
            cache,
            errors,
        })
    }

    /// The device's errors no error scope caught ([`DeviceErrors`]: counted and printed, never a panic).
    pub fn device_errors(&self) -> DeviceErrorCounts {
        self.errors.counts()
    }

    /// The device's pipeline cache, for every pipeline made on it (`cache: gpu.pipeline_cache()`); None on a backend without
    /// caches (all but Vulkan) and in a browser.
    pub fn pipeline_cache(&self) -> Option<&wgpu::PipelineCache> {
        self.cache.as_ref().map(|c| &c.cache)
    }

    /// How many bytes of a saved cache file the device's cache started from; None when it started empty (no file, one refused
    /// or outdated, or one with no pipelines in it).
    pub fn pipeline_cache_loaded(&self) -> Option<usize> {
        self.cache.as_ref().and_then(|c| c.loaded)
    }

    /// Saves the pipeline cache to its file ([`pipeline_cache::write_file`]): the bytes written, Ok(None) when there is no
    /// cache or no file to write.
    pub fn save_pipeline_cache(&self) -> std::io::Result<Option<usize>> {
        self.cache.as_ref().map_or(Ok(None), |c| c.save())
    }

    /// The device (a window's surface is configured on it).
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// The queue.
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// The adapter's description.
    pub fn adapter_info(&self) -> &wgpu::AdapterInfo {
        &self.info
    }

    /// The optional features the device has (of [`hd_wanted`], and `PIPELINE_CACHE` when it has a pipeline cache).
    pub fn features(&self) -> wgpu::Features {
        self.device.features()
    }

    /// The device's limits (the adapter's).
    pub fn limits(&self) -> wgpu::Limits {
        self.device.limits()
    }

    /// `name (backend, driver driver-info, device type)`.
    pub fn adapter(&self) -> String {
        format!(
            "{} ({:?}, driver {} {}, {:?})",
            self.info.name,
            self.info.backend,
            self.info.driver,
            self.info.driver_info,
            self.info.device_type
        )
    }

    /// The sample counts a scene pass can be drawn with on this device (ascending, 1 first).
    pub fn msaa_counts(&self) -> &[u32] {
        &self.sample_counts
    }

    /// The adapter's downlevel flags (whether it executes indirect draws, among others).
    pub fn downlevel_flags(&self) -> wgpu::DownlevelFlags {
        self.downlevel
    }

    /// A target of `width` x `height` pixels.
    pub fn offscreen(&self, width: u32, height: u32) -> Result<Offscreen, RenderError> {
        if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
            return Err(RenderError::BadSize(width, height));
        }
        let extent = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frame"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: COLOR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let depth = self.depth_view(width, height);
        let color = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(Offscreen {
            width,
            height,
            texture,
            color,
            depth,
        })
    }

    /// A depth buffer of `width` x `height` (a window's surface needs its own).
    pub fn depth_view(&self, width: u32, height: u32) -> wgpu::TextureView {
        let depth = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        depth.create_view(&wgpu::TextureViewDescriptor::default())
    }

    /// Shows a window's frame (after drawing into its view).
    pub fn present(&self, frame: wgpu::SurfaceTexture) {
        self.queue.present(frame);
    }

    /// Waits until the GPU has done everything submitted (a frame timing ends here).
    pub fn finish(&self) -> Result<(), RenderError> {
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map(|_| ())
            .map_err(|e| RenderError::Readback(e.to_string()))
    }

    /// Reads `target`'s colour back (waiting for the GPU).
    pub fn read_back(&self, target: &Offscreen) -> Result<Frame, RenderError> {
        let (w, h) = (target.width, target.height);
        let extent = wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        };
        let row = 4 * w;
        let padded =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: u64::from(padded) * u64::from(h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("readback"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(h),
                },
            },
            extent,
        );
        self.queue.submit([encoder.finish()]);
        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| RenderError::Readback(e.to_string()))?;
        rx.recv()
            .map_err(|e| RenderError::Readback(e.to_string()))?
            .map_err(|e| RenderError::Readback(e.to_string()))?;
        let data = slice
            .get_mapped_range()
            .map_err(|e| RenderError::Readback(e.to_string()))?;
        let mut rgba = Vec::with_capacity((row * h) as usize);
        for y in 0..h as usize {
            let start = y * padded as usize;
            rgba.extend_from_slice(&data[start..start + row as usize]);
        }
        drop(data);
        readback.unmap();
        Ok(Frame {
            width: w,
            height: h,
            rgba,
        })
    }
}

#[cfg(test)]
mod tests {
    /// The `key = value` lines of a manifest's lint tables (`[lints.*]`, or the workspace's `[workspace.lints.*]`), each with
    /// its table's tool, in order.
    fn lint_lines(manifest: &str, prefix: &str) -> Vec<(String, String)> {
        let mut tool = None;
        let mut lines = Vec::new();
        for line in manifest.lines().map(str::trim) {
            if line.starts_with('[') {
                tool = line
                    .strip_prefix(prefix)
                    .and_then(|t| t.strip_suffix(']'))
                    .map(str::to_owned);
            } else if let Some(tool) = &tool
                && !line.is_empty()
                && !line.starts_with('#')
            {
                lines.push((tool.clone(), line.to_owned()));
            }
        }
        lines
    }

    /// The crate has one `unsafe` block, under the one `expect(unsafe_code)` attribute (and no `allow` of it).
    #[test]
    fn one_unsafe_block_is_expected() {
        let expect = concat!("#[expect(", "unsafe_code)]");
        let allow = concat!("allow(", "unsafe_code");
        let block = concat!("unsafe", " {");
        let (mut expects, mut allows, mut blocks) = (0, 0, 0);
        for entry in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/src")).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                expects += text.matches(expect).count();
                allows += text.matches(allow).count();
                blocks += text.matches(block).count();
            }
        }
        assert_eq!((expects, allows, blocks), (1, 0, 1));
    }

    /// The crate's lints are the workspace's, `unsafe_code` lowered from `forbid` to `deny` (Cargo.toml says why).
    #[test]
    fn the_lints_are_the_workspaces() {
        let workspace = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml"));
        let ours = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
        let mut expected = lint_lines(workspace, "[workspace.lints.");
        assert!(expected.len() > 10, "the workspace's lints were not found");
        for (_, line) in &mut expected {
            if line == "unsafe_code = \"forbid\"" {
                *line = "unsafe_code = \"deny\"".into();
            }
        }
        assert_eq!(lint_lines(ours, "[lints."), expected);
    }
}
