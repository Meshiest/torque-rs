//! The dedicated-server variant (`torque-bundle --server`): its binaries must link no renderer, window or audio crate and
//! import no GPU, window or audio system library, so the bundle holds a server that needs no GPU, display or sound device.
//!
//! Two checks, both of which refuse the bundle:
//!
//! - Before the build, the crates the binaries link (`cargo tree`) against [`DRAWING_CRATES`], a list that goes beyond what CI
//!   holds `torque-app-server` to (the `server-deps` job of CI and the recipe of that name).
//! - After the build, the binaries' bytes against [`SYSTEM_LIBRARIES`]: the names of the libraries a binary imports (the PE
//!   import table's DLL names, an ELF's `DT_NEEDED` entries, a Mach-O's load commands) are plain strings in the file, so a
//!   case-insensitive search finds them without a parser, and also finds a library a program would load by name at run time.

/// Crates a dedicated server never links: the GPU, window, input and audio stacks, and the engine's drawing, GUI, font, audio
/// and client crates. A crate matches when its name is one of these or continues one with `-` or `_` (`wgpu-hal`, `bevy_ecs`).
pub const DRAWING_CRATES: &[&str] = &[
    // GPU and windows
    "wgpu",
    "naga",
    "winit",
    "tao",
    "raw-window-handle",
    "glutin",
    "glfw",
    "sdl2",
    "sdl3",
    "softbuffer",
    "minifb",
    "pixels",
    "ash",
    "vulkano",
    "glow",
    "khronos-egl",
    "gpu-allocator",
    "d3d12",
    "metal",
    "x11",
    "x11rb",
    "xkbcommon",
    "wayland",
    "smithay-client-toolkit",
    "objc2-app-kit",
    "objc2-metal",
    // UI and game frameworks
    "egui",
    "eframe",
    "bevy",
    // input and audio devices
    "gilrs",
    "cpal",
    "rodio",
    "kira",
    "alsa",
    "coreaudio",
    "objc2-core-audio",
    "libpulse",
    "pipewire",
    "oboe",
    // the engine's own
    "torque-render",
    "torque-gui",
    "torque-audio",
    "torque-client",
    "torque-fonts",
];

/// Whether a crate of this name is a renderer, window, input, audio, GUI or client crate. `torque-app` (the client's half, and
/// its `torque-app-` crates) is one; `torque-app-server` is not.
pub fn is_drawing(name: &str) -> bool {
    if name == "torque-app" || (name.starts_with("torque-app-") && name != "torque-app-server") {
        return true;
    }
    DRAWING_CRATES.iter().any(|crate_name| {
        name.strip_prefix(crate_name)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(['-', '_']))
    })
}

/// The crates of `linked` (name, version) that a dedicated server must not link, as `name version`.
pub fn drawing_crates(linked: &[(String, String)]) -> Vec<String> {
    linked
        .iter()
        .filter(|(name, _)| is_drawing(name))
        .map(|(name, version)| format!("{name} {version}"))
        .collect()
}

/// File names (lowercase) of the GPU, window and audio system libraries a dedicated server never imports. Not here: `user32` and
/// the like (every Windows program imports them), `winmm` and `libudev` (timers and device lists).
pub const SYSTEM_LIBRARIES: &[&str] = &[
    // Windows: Direct3D, DXGI, OpenGL, Vulkan, audio
    "d3d9.dll",
    "d3d10.dll",
    "d3d11.dll",
    "d3d12.dll",
    "dxgi.dll",
    "d3dcompiler_47.dll",
    "opengl32.dll",
    "vulkan-1.dll",
    "dsound.dll",
    "xaudio2_9.dll",
    "mmdevapi.dll",
    // Linux: OpenGL, EGL, Vulkan, X11, XCB, Wayland, ALSA, PulseAudio
    "libgl.so",
    "libegl.so",
    "libgles",
    "libvulkan.so",
    "libx11.so",
    "libxcb.so",
    "libwayland-client",
    "libasound.so",
    "libpulse.so",
    // macOS: Metal, AppKit, Core Audio, Audio Toolbox, OpenGL
    "metal.framework",
    "appkit.framework",
    "coreaudio.framework",
    "audiotoolbox.framework",
    "opengl.framework",
];

/// The libraries of [`SYSTEM_LIBRARIES`] that a binary's bytes name, in the list's order, case-insensitively.
pub fn system_libraries(binary: &[u8]) -> Vec<&'static str> {
    let lower = binary.to_ascii_lowercase();
    SYSTEM_LIBRARIES
        .iter()
        .copied()
        .filter(|library| memchr::memmem::find(&lower, library.as_bytes()).is_some())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drawing_crates_are_found_and_the_server_half_is_not() {
        for name in [
            "wgpu",
            "wgpu-hal",
            "winit",
            "cpal",
            "rodio",
            "kira",
            "sdl2",
            "sdl2-sys",
            "glutin",
            "softbuffer",
            "minifb",
            "pixels",
            "egui",
            "bevy_ecs",
            "ash",
            "vulkano",
            "glow",
            "gilrs",
            "x11-dl",
            "wayland-client",
            "alsa-sys",
            "torque-render-core",
            "torque-gui",
            "torque-audio",
            "torque-client",
            "torque-client-script-host",
            "torque-fonts",
            "torque-app",
            "torque-app-extras",
        ] {
            assert!(is_drawing(name), "{name}");
        }
        for name in [
            "torque-app-server",
            "torque-server",
            "torque-game",
            "serde",
            "ashlar",
            "pixelsort",
            "glowworm",
            "metallic",
        ] {
            assert!(!is_drawing(name), "{name}");
        }
        let linked = [
            ("serde".to_string(), "1.0.0".to_string()),
            ("winit".to_string(), "0.30.0".to_string()),
        ];
        assert_eq!(drawing_crates(&linked), ["winit 0.30.0"]);
    }

    #[test]
    fn system_libraries_are_found_by_name_in_any_case() {
        let mut pe = b"MZ....KERNEL32.dll\0USER32.dll\0".to_vec();
        assert!(system_libraries(&pe).is_empty());
        pe.extend(b"\0D3D11.dll\0DXGI.DLL\0");
        assert_eq!(system_libraries(&pe), ["d3d11.dll", "dxgi.dll"]);
        let elf = b"\x7fELF\0libc.so.6\0libm.so.6\0libX11.so.6\0libasound.so.2\0";
        assert_eq!(system_libraries(elf), ["libx11.so", "libasound.so"]);
        let macho = b"/usr/lib/libSystem.B.dylib\0/System/Library/Frameworks/Metal.framework/Versions/A/Metal\0";
        assert_eq!(system_libraries(macho), ["metal.framework"]);
        // `libglib` and `libgl` are different libraries.
        assert!(system_libraries(b"libglib-2.0.so.0").is_empty());
        assert_eq!(system_libraries(b"libGL.so.1"), ["libgl.so"]);
    }
}
