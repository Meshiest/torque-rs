//! The enhanced renderer's WGSL (`shaders/`, our own) and its permutations. A module is a permutation's constants, then
//! `common.wgsl` (with the extension's common chunk) when the file uses it, then the file. [`validate_all`] parses and validates
//! every permutation with naga (the same front end wgpu compiles them with): the `shaders N of N valid`.

use torque_render_scene::hd::reflect::{CAPTURE_MIPS, CAPTURE_SIZE};

/// The shader files, by name (embedded at build time).
pub const FILES: [(&str, &str); 45] = [
    ("common.wgsl", include_str!("../shaders/common.wgsl")),
    ("brdf.wgsl", include_str!("../shaders/brdf.wgsl")),
    ("area.wgsl", include_str!("../shaders/area.wgsl")),
    ("forward.wgsl", include_str!("../shaders/forward.wgsl")),
    ("pbr.wgsl", include_str!("../shaders/pbr.wgsl")),
    ("particles.wgsl", include_str!("../shaders/particles.wgsl")),
    (
        "gpu_particle_types.wgsl",
        include_str!("../shaders/gpu_particle_types.wgsl"),
    ),
    (
        "gpu_particles.wgsl",
        include_str!("../shaders/gpu_particles.wgsl"),
    ),
    (
        "gpu_sprite.wgsl",
        include_str!("../shaders/gpu_sprite.wgsl"),
    ),
    ("clusters.wgsl", include_str!("../shaders/clusters.wgsl")),
    ("tonemap.wgsl", include_str!("../shaders/tonemap.wgsl")),
    ("shadow.wgsl", include_str!("../shaders/shadow.wgsl")),
    ("contact.wgsl", include_str!("../shaders/contact.wgsl")),
    (
        "contact_filter.wgsl",
        include_str!("../shaders/contact_filter.wgsl"),
    ),
    ("ao.wgsl", include_str!("../shaders/ao.wgsl")),
    ("ao_filter.wgsl", include_str!("../shaders/ao_filter.wgsl")),
    ("taa.wgsl", include_str!("../shaders/taa.wgsl")),
    ("fxaa.wgsl", include_str!("../shaders/fxaa.wgsl")),
    ("reflect.wgsl", include_str!("../shaders/reflect.wgsl")),
    ("water.wgsl", include_str!("../shaders/water.wgsl")),
    ("hiz.wgsl", include_str!("../shaders/hiz.wgsl")),
    ("ssr.wgsl", include_str!("../shaders/ssr.wgsl")),
    ("down.wgsl", include_str!("../shaders/down.wgsl")),
    ("prefilter.wgsl", include_str!("../shaders/prefilter.wgsl")),
    ("cubeface.wgsl", include_str!("../shaders/cubeface.wgsl")),
    (
        "irradiance.wgsl",
        include_str!("../shaders/irradiance.wgsl"),
    ),
    ("skycommon.wgsl", include_str!("../shaders/skycommon.wgsl")),
    ("skyfog.wgsl", include_str!("../shaders/skyfog.wgsl")),
    (
        "atmosphere.wgsl",
        include_str!("../shaders/atmosphere.wgsl"),
    ),
    ("sky.wgsl", include_str!("../shaders/sky.wgsl")),
    (
        "volumetric.wgsl",
        include_str!("../shaders/volumetric.wgsl"),
    ),
    ("froxels.wgsl", include_str!("../shaders/froxels.wgsl")),
    ("bloom.wgsl", include_str!("../shaders/bloom.wgsl")),
    ("exposure.wgsl", include_str!("../shaders/exposure.wgsl")),
    ("surface.wgsl", include_str!("../shaders/surface.wgsl")),
    ("wboit.wgsl", include_str!("../shaders/wboit.wgsl")),
    ("clouds.wgsl", include_str!("../shaders/clouds.wgsl")),
    (
        "cloudshadow.wgsl",
        include_str!("../shaders/cloudshadow.wgsl"),
    ),
    ("cull.wgsl", include_str!("../shaders/cull.wgsl")),
    (
        "shadowcommon.wgsl",
        include_str!("../shaders/shadowcommon.wgsl"),
    ),
    ("sunshadow.wgsl", include_str!("../shaders/sunshadow.wgsl")),
    (
        "fullscreen.wgsl",
        include_str!("../shaders/fullscreen.wgsl"),
    ),
    (
        "dissolve_common.wgsl",
        include_str!("../shaders/dissolve_common.wgsl"),
    ),
    ("dissolve.wgsl", include_str!("../shaders/dissolve.wgsl")),
    (
        "dissolve_shadow.wgsl",
        include_str!("../shaders/dissolve_shadow.wgsl"),
    ),
];

/// An extension's WGSL, composed into the renderer's modules: its uniforms' blocks, its hooks and its own entry points (the
/// engine's own, [`NONE`], has no surfaces of its own: every hook leaves the engine's shading as it is).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShaderExtension {
    /// With common.wgsl in every module that has it: the uniforms' extension blocks (`FrameExt`, `ReflExt`) and the constants of
    /// the material word's extension bits.
    pub common: &'static str,
    /// Before forward.wgsl (the forward pass, a secondary view, the water): the hooks forward.wgsl and water.wgsl call (`ext_*`,
    /// `reflect_class`), the extension's bindings and entry points.
    pub forward: &'static str,
    /// After shadow.wgsl: `CasterExt`, `ext_excluded` and the extension's casters.
    pub shadow: &'static str,
}

/// The engine's own extension: no surfaces of its own.
pub const NONE: ShaderExtension = ShaderExtension {
    common: include_str!("../shaders/ext_common.wgsl"),
    forward: include_str!("../shaders/ext_forward.wgsl"),
    shadow: include_str!("../shaders/ext_shadow.wgsl"),
};

static EXTENSION: std::sync::OnceLock<ShaderExtension> = std::sync::OnceLock::new();

/// Installs the extension the modules are composed with, before the first module is built; false when one already is.
pub fn register(ext: ShaderExtension) -> bool {
    EXTENSION.set(ext).is_ok()
}

/// The extension the modules are composed with ([`NONE`] unless another was registered first).
pub fn extension() -> ShaderExtension {
    *EXTENSION.get_or_init(|| NONE)
}

/// A module the renderer builds: which file, with which constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permutation {
    /// The forward pass; `clustered: false` is the brute-force light loop (the clusters gate's reference); `front_layer` the
    /// translucent pass's front layer (blended exactly over the weighted composite's copy of the frame, in the blending space).
    Forward {
        clustered: bool,
        front_layer: bool,
    },
    Clusters,
    Tonemap,
    /// The shadow maps' casters; `unclipped`: the device clamps depth (pancaking), else the vertex stage does.
    Shadow {
        unclipped: bool,
    },
    /// The sun's visibility mask: the cascades and the screen-space contact shadows at each pixel of the prepass.
    Contact,
    /// The mask's spatial filter.
    ContactFilter,
    /// The ambient occlusion (GTAO, or SSAO), each frame's estimate on its own.
    Ao {
        gtao: bool,
    },
    /// The ambient occlusion's spatial filter.
    AoFilter,
    /// The temporal anti-aliasing resolve.
    Taa,
    /// The Low quality's FXAA-style edge filter.
    Fxaa,
    /// The forward pass of a secondary view (the water's mirrored camera, a light capture's face): every light in turn (no clusters of
    /// its own), cut at the water plane when the frame says so.
    View,
    /// The water: the forward pass's module with `fs_water`.
    Water,
    /// The Hi-Z pyramid.
    Hiz,
    /// The screen-space reflections' trace and accumulation.
    Ssr,
    /// A mip level from the one above it.
    Down,
    /// An environment cube's prefiltered level from the cube's level above it (a face a pipeline: its `FACE` constant).
    Prefilter,
    /// The diffuse ambient's spherical harmonics from the environment cubes (compute).
    Irradiance,
    /// The physical sky's tables (compute).
    Atmosphere,
    /// The atmosphere's sky and the sun disk (full-frame, in the opaque pass).
    Sky,
    /// The volumetric light's march and its composite.
    Volumetric,
    /// The froxel volumetrics' gather, sum (compute) and composite (the point lights in the fog).
    Froxels,
    /// The bloom chain.
    Bloom,
    /// The automatic exposure (compute).
    Exposure,
    /// The weighted blended transparency's composite.
    Wboit,
    /// The volumetric clouds' march and their shadow map.
    Clouds,
    /// The static cells' clusters' cull and the compaction of their draw lists (compute).
    Cull,
    /// The forward pass's module with the GPU particle pool's sprite vertex entry (`vs_gpu_sprite`), of the same constants.
    Sprite {
        clustered: bool,
        front_layer: bool,
    },
    /// The GPU particle pool's frame: its step, spawns and lists (compute).
    Particles,
    /// The forward module of the opaque pass (`front_layer` false) with a dissolving object's entries (`dissolve.wgsl`:
    /// `vs_dissolve`, `fs_dissolve`, `vs_depth_dissolve`, `fs_depth_dissolve`); its `vs_main` and `fs_depth` are plain
    /// functions there (`vs_main_of`, `fs_depth_of`) for them to call.
    Dissolve {
        clustered: bool,
    },
    /// The shadow module with a dissolving object's casters (`dissolve_shadow.wgsl`).
    ShadowDissolve {
        unclipped: bool,
    },
    /// The forward pass of a draw with a physically based material: the forward module of the same constants with `pbr.wgsl`
    /// (`vs_pbr`, `fs_pbr`, `fs_pbr_wboit`); `parallax` marches the height map (the highest quality). Its own module: the
    /// permutations above never include `pbr.wgsl`, so a draw without a material compiles to the same shaders as before.
    ForwardPbr {
        clustered: bool,
        front_layer: bool,
        parallax: bool,
    },
}

/// What [`Permutation::Dissolve`] turns into plain functions in the forward pass's text, and what it calls them.
pub const DISSOLVE_RENAMES: [(&str, &str); 2] = [
    ("@vertex\nfn vs_main(", "fn vs_main_of("),
    ("@fragment\nfn fs_depth(", "fn fs_depth_of("),
];

impl Permutation {
    /// Every module the renderer can build.
    pub const ALL: [Permutation; 48] = [
        Permutation::Forward {
            clustered: true,
            front_layer: false,
        },
        Permutation::Forward {
            clustered: false,
            front_layer: false,
        },
        Permutation::Forward {
            clustered: true,
            front_layer: true,
        },
        Permutation::Forward {
            clustered: false,
            front_layer: true,
        },
        Permutation::Clusters,
        Permutation::Tonemap,
        Permutation::Shadow { unclipped: true },
        Permutation::Shadow { unclipped: false },
        Permutation::Contact,
        Permutation::ContactFilter,
        Permutation::Ao { gtao: true },
        Permutation::Ao { gtao: false },
        Permutation::AoFilter,
        Permutation::Taa,
        Permutation::Fxaa,
        Permutation::View,
        Permutation::Water,
        Permutation::Hiz,
        Permutation::Ssr,
        Permutation::Down,
        Permutation::Prefilter,
        Permutation::Irradiance,
        Permutation::Atmosphere,
        Permutation::Sky,
        Permutation::Volumetric,
        Permutation::Froxels,
        Permutation::Bloom,
        Permutation::Exposure,
        Permutation::Wboit,
        Permutation::Clouds,
        Permutation::Cull,
        Permutation::Sprite {
            clustered: true,
            front_layer: false,
        },
        Permutation::Sprite {
            clustered: false,
            front_layer: false,
        },
        Permutation::Sprite {
            clustered: true,
            front_layer: true,
        },
        Permutation::Sprite {
            clustered: false,
            front_layer: true,
        },
        Permutation::Particles,
        Permutation::Dissolve { clustered: true },
        Permutation::Dissolve { clustered: false },
        Permutation::ShadowDissolve { unclipped: true },
        Permutation::ShadowDissolve { unclipped: false },
        Permutation::ForwardPbr {
            clustered: true,
            front_layer: false,
            parallax: false,
        },
        Permutation::ForwardPbr {
            clustered: false,
            front_layer: false,
            parallax: false,
        },
        Permutation::ForwardPbr {
            clustered: true,
            front_layer: true,
            parallax: false,
        },
        Permutation::ForwardPbr {
            clustered: false,
            front_layer: true,
            parallax: false,
        },
        Permutation::ForwardPbr {
            clustered: true,
            front_layer: false,
            parallax: true,
        },
        Permutation::ForwardPbr {
            clustered: false,
            front_layer: false,
            parallax: true,
        },
        Permutation::ForwardPbr {
            clustered: true,
            front_layer: true,
            parallax: true,
        },
        Permutation::ForwardPbr {
            clustered: false,
            front_layer: true,
            parallax: true,
        },
    ];

    pub fn name(self) -> String {
        match self {
            Permutation::Forward {
                clustered,
                front_layer,
            } => format!("forward.wgsl (CLUSTERED = {clustered}, FRONT_LAYER = {front_layer})"),
            Permutation::Clusters => "clusters.wgsl".to_string(),
            Permutation::Tonemap => "tonemap.wgsl".to_string(),
            Permutation::Shadow { unclipped } => format!("shadow.wgsl (UNCLIPPED = {unclipped})"),
            Permutation::Contact => "contact.wgsl".to_string(),
            Permutation::ContactFilter => "contact_filter.wgsl".to_string(),
            Permutation::Ao { gtao } => format!("ao.wgsl (GTAO = {gtao})"),
            Permutation::AoFilter => "ao_filter.wgsl".to_string(),
            Permutation::Taa => "taa.wgsl".to_string(),
            Permutation::Fxaa => "fxaa.wgsl".to_string(),
            Permutation::View => {
                "forward.wgsl (a secondary view: CLUSTERED = false, CLIP = true)".to_string()
            }
            Permutation::Water => "water.wgsl".to_string(),
            Permutation::Hiz => "hiz.wgsl".to_string(),
            Permutation::Ssr => "ssr.wgsl".to_string(),
            Permutation::Down => "down.wgsl".to_string(),
            Permutation::Prefilter => "prefilter.wgsl".to_string(),
            Permutation::Irradiance => "irradiance.wgsl".to_string(),
            Permutation::Atmosphere => "atmosphere.wgsl".to_string(),
            Permutation::Sky => "sky.wgsl".to_string(),
            Permutation::Volumetric => "volumetric.wgsl".to_string(),
            Permutation::Froxels => "froxels.wgsl".to_string(),
            Permutation::Bloom => "bloom.wgsl".to_string(),
            Permutation::Exposure => "exposure.wgsl".to_string(),
            Permutation::Wboit => "wboit.wgsl".to_string(),
            Permutation::Clouds => "clouds.wgsl".to_string(),
            Permutation::Cull => "cull.wgsl".to_string(),
            Permutation::Sprite {
                clustered,
                front_layer,
            } => format!(
                "forward.wgsl with gpu_sprite.wgsl (CLUSTERED = {clustered}, FRONT_LAYER = {front_layer})"
            ),
            Permutation::Particles => "gpu_particles.wgsl".to_string(),
            Permutation::Dissolve { clustered } => {
                format!("forward.wgsl with dissolve.wgsl (CLUSTERED = {clustered})")
            }
            Permutation::ShadowDissolve { unclipped } => {
                format!("shadow.wgsl with dissolve_shadow.wgsl (UNCLIPPED = {unclipped})")
            }
            Permutation::ForwardPbr {
                clustered,
                front_layer,
                parallax,
            } => format!(
                "forward.wgsl with pbr.wgsl (CLUSTERED = {clustered}, FRONT_LAYER = {front_layer}, PARALLAX = {parallax})"
            ),
        }
    }

    /// The module's source text.
    pub fn source(self) -> String {
        let file = |name: &str| {
            FILES
                .iter()
                .find(|(n, _)| *n == name)
                .map_or("", |(_, t)| *t)
        };
        let ext = extension();
        // common.wgsl's uniforms end in the extension's blocks: composed with it everywhere.
        let common = &format!(
            "{}\n{}\n{}",
            file("common.wgsl"),
            file("shadowcommon.wgsl"),
            ext.common
        );
        // The full-target triangle's vertex entry point, for the passes that draw one.
        let full = file("fullscreen.wgsl");
        // The surface helpers and the extension's hooks come before the forward pass's functions.
        let forward = &format!(
            "{}\n{}\n{}\n{}\n{}",
            file("surface.wgsl"),
            ext.forward,
            file("sunshadow.wgsl"),
            file("forward.wgsl"),
            file("particles.wgsl")
        );
        match self {
            Permutation::Forward {
                clustered,
                front_layer,
            } => format!(
                "const CLUSTERED: bool = {clustered};\nconst FRONT_LAYER: bool = {front_layer};\nconst CLIP: bool = false;\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
                common,
                file("skycommon.wgsl"),
                file("brdf.wgsl"),
                file("area.wgsl"),
                file("reflect.wgsl"),
                forward,
                file("skyfog.wgsl"),
                file("cloudshadow.wgsl")
            ),
            Permutation::View => format!(
                "const CLUSTERED: bool = false;\nconst FRONT_LAYER: bool = false;\nconst CLIP: bool = true;\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
                common,
                file("skycommon.wgsl"),
                file("brdf.wgsl"),
                file("area.wgsl"),
                file("reflect.wgsl"),
                forward,
                file("skyfog.wgsl"),
                file("cloudshadow.wgsl")
            ),
            Permutation::Water => format!(
                "const CLUSTERED: bool = true;\nconst FRONT_LAYER: bool = false;\nconst CLIP: bool = false;\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
                common,
                file("skycommon.wgsl"),
                file("brdf.wgsl"),
                file("area.wgsl"),
                file("reflect.wgsl"),
                forward,
                file("skyfog.wgsl"),
                file("cloudshadow.wgsl"),
                file("water.wgsl")
            ),
            Permutation::Sprite {
                clustered,
                front_layer,
            } => format!(
                "{}\n{}\n{}",
                Permutation::Forward {
                    clustered,
                    front_layer
                }
                .source(),
                file("gpu_particle_types.wgsl"),
                file("gpu_sprite.wgsl")
            ),
            Permutation::Particles => format!(
                "{}\n{}",
                file("gpu_particle_types.wgsl"),
                file("gpu_particles.wgsl")
            ),
            Permutation::Dissolve { clustered } => {
                let mut forward = Permutation::Forward {
                    clustered,
                    front_layer: false,
                }
                .source();
                for (entry, plain) in DISSOLVE_RENAMES {
                    forward = forward.replacen(entry, plain, 1);
                }
                format!(
                    "{}\n{}\n{}",
                    forward,
                    file("dissolve_common.wgsl"),
                    file("dissolve.wgsl")
                )
            }
            Permutation::ShadowDissolve { unclipped } => format!(
                "{}\n{}\n{}",
                Permutation::Shadow { unclipped }.source(),
                file("dissolve_common.wgsl"),
                file("dissolve_shadow.wgsl")
            ),
            Permutation::ForwardPbr {
                clustered,
                front_layer,
                parallax,
            } => format!(
                "const PARALLAX: bool = {parallax};
{}
{}",
                Permutation::Forward {
                    clustered,
                    front_layer
                }
                .source(),
                file("pbr.wgsl")
            ),
            Permutation::Hiz => file("hiz.wgsl").to_string(),
            Permutation::Ssr => format!("{}\n{}\n{}", common, full, file("ssr.wgsl")),
            Permutation::Down => file("down.wgsl").to_string(),
            Permutation::Prefilter => format!(
                "const CUBE_SIZE: f32 = {CAPTURE_SIZE}.0;\nconst CUBE_LEVELS: f32 = {CAPTURE_MIPS}.0;\n{}\n{}\n{}\n{}",
                // brdf.wgsl's PI without the rest of common.wgsl (its helpers read the frame's uniform).
                file("common.wgsl")
                    .lines()
                    .find(|l| l.starts_with("const PI: f32"))
                    .unwrap_or_default(),
                file("brdf.wgsl"),
                file("cubeface.wgsl"),
                file("prefilter.wgsl")
            ),
            Permutation::Irradiance => {
                format!("{}\n{}", file("cubeface.wgsl"), file("irradiance.wgsl"))
            }
            Permutation::Clusters => format!("{}\n{}", common, file("clusters.wgsl")),
            Permutation::Tonemap => format!("{}\n{}", full, file("tonemap.wgsl")),
            Permutation::Shadow { unclipped } => format!(
                "const UNCLIPPED: bool = {unclipped};\n{}\n{}",
                file("shadow.wgsl"),
                ext.shadow
            ),
            Permutation::Contact => format!(
                "{}\n{}\n{}\n{}",
                common,
                full,
                file("sunshadow.wgsl"),
                file("contact.wgsl")
            ),
            Permutation::ContactFilter => {
                format!("{}\n{}\n{}", common, full, file("contact_filter.wgsl"))
            }
            Permutation::Ao { gtao } => format!(
                "const GTAO: bool = {gtao};\n{}\n{}\n{}",
                common,
                full,
                file("ao.wgsl")
            ),
            Permutation::AoFilter => format!("{}\n{}\n{}", common, full, file("ao_filter.wgsl")),
            Permutation::Taa => format!("{}\n{}\n{}", common, full, file("taa.wgsl")),
            Permutation::Fxaa => format!("{}\n{}", full, file("fxaa.wgsl")),
            Permutation::Atmosphere => {
                format!("{}\n{}", file("skycommon.wgsl"), file("atmosphere.wgsl"))
            }
            Permutation::Sky => format!(
                "{}\n{}\n{}\n{}\n{}",
                common,
                full,
                file("skycommon.wgsl"),
                file("sky.wgsl"),
                file("cloudshadow.wgsl")
            ),
            Permutation::Volumetric => format!(
                "{}\n{}\n{}\n{}\n{}",
                common,
                full,
                file("skycommon.wgsl"),
                file("volumetric.wgsl"),
                file("cloudshadow.wgsl")
            ),
            Permutation::Froxels => format!(
                "{}
{}
{}
{}",
                common,
                full,
                file("skycommon.wgsl"),
                file("froxels.wgsl")
            ),
            Permutation::Bloom => file("bloom.wgsl").to_string(),
            Permutation::Exposure => file("exposure.wgsl").to_string(),
            Permutation::Wboit => format!("{}\n{}", full, file("wboit.wgsl")),
            Permutation::Clouds => format!(
                "{}\n{}\n{}\n{}",
                common,
                full,
                file("skycommon.wgsl"),
                file("clouds.wgsl")
            ),
            Permutation::Cull => file("cull.wgsl").to_string(),
        }
    }
}

/// naga's verdict on one module: `Ok` or the error as text.
pub fn validate(p: Permutation) -> Result<(), String> {
    let src = p.source();
    let module = naga::front::wgsl::parse_str(&src).map_err(|e| e.emit_to_string(&src))?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .map(|_| ())
    .map_err(|e| format!("{e:?}"))
}

/// Every permutation's verdict: (valid, total, the failures).
pub fn validate_all() -> (usize, usize, Vec<String>) {
    let mut bad = Vec::new();
    for p in Permutation::ALL {
        if let Err(e) = validate(p) {
            bad.push(format!("{}: {e}", p.name()));
        }
    }
    let total = Permutation::ALL.len();
    (total - bad.len(), total, bad)
}

/// A digest of every shader file and the extension's chunks (a stamp of the shaders a build draws with).
pub fn source_digest() -> u64 {
    let ext = extension();
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for (name, text) in FILES.into_iter().chain([
        ("ext common", ext.common),
        ("ext forward", ext.forward),
        ("ext shadow", ext.shadow),
    ]) {
        for b in name.bytes().chain(text.bytes()) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

#[cfg(test)]
#[path = "shaders_tests.rs"]
mod tests;
