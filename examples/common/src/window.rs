//! A window's GPU device and surface, kept to the window's size, and a frame of it to draw into.

use std::sync::Arc;

use torque_render_core::window::{WINDOW_BACKENDS, configure_surface_vsync, window_gpu};
use torque_render_core::{Gpu, RenderError, wgpu};
use winit::window::Window;

/// An offscreen device (pictures and tests) on the first of the engine's window backends ([`WINDOW_BACKENDS`]: Vulkan first)
/// with an adapter, so the pictures are drawn as the windows draw them.
pub fn offscreen_gpu() -> Result<Gpu, RenderError> {
    Gpu::with_backends(&WINDOW_BACKENDS)
}

pub struct WindowSurface {
    /// The surface draws into the window; kept with it.
    _window: Arc<Window>,
    pub gpu: Gpu,
    surface: wgpu::Surface<'static>,
    size: (u32, u32),
}

impl WindowSurface {
    /// The device and surface of `window` on the engine's window backend order ([`window_gpu`]: Vulkan first, `WGPU_BACKEND`
    /// ahead of it), its client area `size`, frames shown at the display's refresh.
    pub fn new(window: &Arc<Window>, size: (u32, u32)) -> Result<WindowSurface, String> {
        let (gpu, surface, _adapter) = window_gpu(window.clone()).map_err(|e| e.to_string())?;
        configure_surface_vsync(&gpu, &surface, size.0, size.1, true);
        Ok(WindowSurface {
            _window: window.clone(),
            gpu,
            surface,
            size,
        })
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn resize(&mut self, size: (u32, u32)) {
        self.size = size;
        configure_surface_vsync(&self.gpu, &self.surface, size.0, size.1, true);
    }

    /// The next frame to draw into, or None when the surface has none now (it is configured again for the next try).
    pub fn frame(&mut self) -> Option<wgpu::SurfaceTexture> {
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => Some(f),
            _ => {
                self.resize(self.size);
                None
            }
        }
    }

    /// Shows a frame drawn into.
    pub fn present(&self, frame: wgpu::SurfaceTexture) {
        self.gpu.present(frame);
    }
}
