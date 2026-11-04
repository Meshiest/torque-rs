//! A window's device: its surface, the backends a window may draw with and their order, and the surface's configuration.

use crate::{COLOR_FORMAT, Gpu, RenderError};

/// The colour format a window's surface must offer: the frames' own (`Rgba8Unorm`, no sRGB step: frames are drawn in gamma space).
pub const SURFACE_FORMAT: wgpu::TextureFormat = COLOR_FORMAT;

/// The backends a window may draw with, in the order this crate prefers them (Direct3D 12 first on Windows: what Windows players
/// expect, and clear of Vulkan driver quirks).
pub const WINDOW_BACKENDS: [wgpu::Backends; 3] = [
    wgpu::Backends::DX12,
    wgpu::Backends::VULKAN,
    wgpu::Backends::METAL,
];

/// The backend a display device name (`$pref::Video::displayDevice`, [`crate::backend_name`]'s words) asks for; None for any other
/// name ("OpenGL" among them: there is no OpenGL backend).
pub fn backend_named(name: &str) -> Option<wgpu::Backends> {
    WINDOW_BACKENDS
        .into_iter()
        .find(|b| backends_name(*b).is_some_and(|n| n.eq_ignore_ascii_case(name.trim())))
}

/// The display device name of one backend.
pub fn backends_name(b: wgpu::Backends) -> Option<&'static str> {
    Some(if b == wgpu::Backends::DX12 {
        crate::backend_name(wgpu::Backend::Dx12)
    } else if b == wgpu::Backends::VULKAN {
        crate::backend_name(wgpu::Backend::Vulkan)
    } else if b == wgpu::Backends::METAL {
        crate::backend_name(wgpu::Backend::Metal)
    } else {
        return None;
    })
}

/// The backends this machine can draw with: those of [`WINDOW_BACKENDS`] with at least one adapter that is not a CPU device, by
/// their display device names, in that order.
pub fn usable_backends() -> Vec<&'static str> {
    WINDOW_BACKENDS
        .into_iter()
        .filter(|b| {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: *b,
                flags: crate::instance_flags(),
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            pollster::block_on(instance.enumerate_adapters(*b))
                .iter()
                .any(|a| a.get_info().device_type != wgpu::DeviceType::Cpu)
        })
        .filter_map(backends_name)
        .collect()
}

/// A device for a window: the window's surface and an adapter of `backends` that can present to it.
fn window_gpu_with<T>(
    target: T,
    backends: wgpu::Backends,
) -> Result<(Gpu, wgpu::Surface<'static>, wgpu::Adapter), RenderError>
where
    T: Into<wgpu::SurfaceTarget<'static>>,
{
    let target = target.into();
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        flags: crate::instance_flags(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let surface = instance
        .create_surface(target)
        .map_err(|e| RenderError::NoAdapter(format!("the window's surface: {e}")))?;
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: Some(&surface),
        apply_limit_buckets: false,
    }))
    .map_err(|e| RenderError::NoAdapter(e.to_string()))?;
    if !surface
        .get_capabilities(&adapter)
        .formats
        .contains(&SURFACE_FORMAT)
    {
        return Err(RenderError::NoAdapter(format!(
            "the window's surface does not offer {SURFACE_FORMAT:?}"
        )));
    }
    let gpu = Gpu::on_adapter(&adapter)?;
    Ok((gpu, surface, adapter))
}

/// A device for a window: the backend `first` asks for when it starts, else the others in [`WINDOW_BACKENDS`]' order (Direct3D
/// 12 first). The last item says why `first` did not start, when it did not (the window carries on with the next that does).
#[allow(clippy::type_complexity)]
pub fn window_gpu_prefer<T>(
    target: T,
    first: Option<wgpu::Backends>,
) -> Result<(Gpu, wgpu::Surface<'static>, wgpu::Adapter, Option<String>), RenderError>
where
    T: Into<wgpu::SurfaceTarget<'static>> + Clone,
{
    let mut order: Vec<wgpu::Backends> = first.into_iter().collect();
    order.extend(WINDOW_BACKENDS.into_iter().filter(|b| Some(*b) != first));
    let mut why_first = None;
    let mut last = RenderError::NoAdapter("no backend".into());
    for b in order {
        match window_gpu_with(target.clone(), b) {
            Ok((g, s, a)) => return Ok((g, s, a, why_first)),
            Err(e) => {
                if Some(b) == first {
                    why_first = Some(e.to_string());
                }
                last = e;
            }
        }
    }
    Err(last)
}

/// A device for a window, Direct3D 12 first ([`window_gpu_prefer`] with no preference). `target` is the window (anything wgpu
/// makes a surface of).
pub fn window_gpu<T>(target: T) -> Result<(Gpu, wgpu::Surface<'static>, wgpu::Adapter), RenderError>
where
    T: Into<wgpu::SurfaceTarget<'static>> + Clone,
{
    window_gpu_prefer(target, None).map(|(g, s, a, _)| (g, s, a))
}

/// Configures `surface` for frames of `width` x `height` on `gpu`'s device (vsync on: a window never draws faster than the
/// display shows).
pub fn configure_surface(gpu: &Gpu, surface: &wgpu::Surface<'_>, width: u32, height: u32) {
    configure_surface_vsync(gpu, surface, width, height, true);
}

/// [`configure_surface`] with vertical sync on (`Fifo`: one frame a refresh) or off (`$pref::Video::disableVerticalSync`:
/// `Immediate`, else `Mailbox`, else `Fifo`, whatever the adapter offers first: frames as fast as they are drawn).
pub fn configure_surface_vsync(
    gpu: &Gpu,
    surface: &wgpu::Surface<'_>,
    width: u32,
    height: u32,
    vsync: bool,
) {
    surface.configure(
        gpu.device(),
        &wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: SURFACE_FORMAT,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: width.max(1),
            height: height.max(1),
            present_mode: if vsync {
                wgpu::PresentMode::AutoVsync
            } else {
                wgpu::PresentMode::AutoNoVsync
            },
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
        },
    );
}
