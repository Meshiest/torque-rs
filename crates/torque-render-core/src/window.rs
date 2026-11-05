//! A window's device: its surface, the backends a window may draw with and their order, and the surface's configuration.
//!
//! A window tries Vulkan first, then Direct3D 12, then Metal ([`WINDOW_BACKENDS`]); a backend with no adapter, or whose surface
//! does not start, falls back to the next. Two things go ahead of that order: wgpu's own `WGPU_BACKEND` environment variable
//! ([`BACKEND_ENV`], for one run) and the caller's own choice ([`window_gpu_prefer`]'s `first`, such as a profile's display
//! device). Offscreen devices ([`Gpu::new`](crate::Gpu::new)) keep their own order.

use crate::{COLOR_FORMAT, Gpu, RenderError};

/// The color format a window's surface must offer: the frames' own (`Rgba8Unorm`, no sRGB step: frames are drawn in gamma space).
pub const SURFACE_FORMAT: wgpu::TextureFormat = COLOR_FORMAT;

/// The backends a window may draw with, in the order a window tries them: Vulkan first, then Direct3D 12 and Metal. Vulkan makes
/// the renderers' pipelines in well under a second (and the driver keeps them for the next start); Direct3D 12's go through its
/// older shader compiler and take the enhanced renderer more than ten seconds at every start.
pub const WINDOW_BACKENDS: [wgpu::Backends; 3] = [
    wgpu::Backends::VULKAN,
    wgpu::Backends::DX12,
    wgpu::Backends::METAL,
];

/// The environment variable that puts backends ahead of a window's order for one run: wgpu's own `WGPU_BACKEND`, a comma list
/// such as `dx12` or `vulkan,metal` (`vk`, `d3d12` and `mtl` too). The ones it names are tried first, in [`WINDOW_BACKENDS`]'
/// order; names of backends a window does not draw with are left out.
pub const BACKEND_ENV: &str = "WGPU_BACKEND";

/// The order a window tries backends in: those `env` names ([`BACKEND_ENV`]), then `first`, then the rest of
/// [`WINDOW_BACKENDS`], each once.
pub fn window_order(
    env: Option<wgpu::Backends>,
    first: Option<wgpu::Backends>,
) -> Vec<wgpu::Backends> {
    let named = WINDOW_BACKENDS
        .into_iter()
        .filter(|b| env.is_some_and(|e| e.contains(*b)));
    let mut order: Vec<wgpu::Backends> = Vec::with_capacity(WINDOW_BACKENDS.len());
    for b in named.chain(first).chain(WINDOW_BACKENDS) {
        if !b.is_empty() && !order.contains(&b) {
            order.push(b);
        }
    }
    order
}

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
    pollster::block_on(window_gpu_async(target, backends))
}

/// A browser canvas's device: [`window_gpu`] on WebGPU, without blocking (the browser answers on its own event loop).
pub async fn web_gpu<T>(
    target: T,
) -> Result<(Gpu, wgpu::Surface<'static>, wgpu::Adapter), RenderError>
where
    T: Into<wgpu::SurfaceTarget<'static>>,
{
    window_gpu_async(target, wgpu::Backends::BROWSER_WEBGPU).await
}

async fn window_gpu_async<T>(
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
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
            apply_limit_buckets: false,
        })
        .await
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
    let gpu = Gpu::on_adapter_async(&adapter).await?;
    Ok((gpu, surface, adapter))
}

/// A device for a window: the backends [`BACKEND_ENV`] names, then the backend `first` asks for, then the others in
/// [`WINDOW_BACKENDS`]' order (Vulkan first), the first that starts ([`window_order`]). The last item says why `first` did not
/// start, when it was tried and did not (the window carries on with the next that does).
#[allow(clippy::type_complexity)]
pub fn window_gpu_prefer<T>(
    target: T,
    first: Option<wgpu::Backends>,
) -> Result<(Gpu, wgpu::Surface<'static>, wgpu::Adapter, Option<String>), RenderError>
where
    T: Into<wgpu::SurfaceTarget<'static>> + Clone,
{
    let order = window_order(wgpu::Backends::from_env(), first);
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

/// A device for a window, Vulkan first ([`window_gpu_prefer`] with no preference). `target` is the window (anything wgpu
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

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::Backends as B;

    #[test]
    fn a_window_tries_vulkan_first() {
        assert_eq!(window_order(None, None), [B::VULKAN, B::DX12, B::METAL]);
    }

    #[test]
    fn the_callers_choice_goes_first() {
        assert_eq!(
            window_order(None, Some(B::DX12)),
            [B::DX12, B::VULKAN, B::METAL]
        );
    }

    #[test]
    fn the_environment_goes_ahead_of_the_callers_choice() {
        assert_eq!(
            window_order(Some(B::DX12 | B::METAL), Some(B::VULKAN)),
            [B::DX12, B::METAL, B::VULKAN]
        );
    }

    #[test]
    fn backends_a_window_does_not_draw_with_are_left_out() {
        assert_eq!(
            window_order(Some(B::GL), None),
            [B::VULKAN, B::DX12, B::METAL]
        );
        assert_eq!(
            window_order(None, Some(B::empty())),
            [B::VULKAN, B::DX12, B::METAL]
        );
    }
}
