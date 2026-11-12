//! Physically based materials and their `.mat.toml` sidecar.
//!
//! A [`PbrMaterial`] is a set of texture maps and scalar factors: base color (sRGB), a tangent-space normal map (OpenGL's
//! convention, +Y up; [`NormalConvention::DirectX`] marks a map whose green channel is flipped), an ORM map (occlusion,
//! roughness, metalness in R, G, B) or separate occlusion, roughness and metalness maps, an optional height map (R) and an
//! optional emissive map, with factors that multiply each. Nothing here names a library or a vendor: any material that has
//! these maps fits.
//!
//! **The sidecar.** A file `<name>.mat.toml` beside a texture holds one material at its top level; beside a shape it holds a
//! `[materials.<material name>]` table each (the names the shape's material list keeps). Map paths are relative to the
//! sidecar's folder. [`Sidecar::parse`] reads hostile text: a value or a [`SidecarError`], never a panic; the text, the number
//! of materials, every path and every factor are checked against the limits below.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The longest sidecar text read (bytes).
pub const MAX_SIDECAR_BYTES: usize = 64 * 1024;
/// The most materials one sidecar holds.
pub const MAX_MATERIALS: usize = 256;
/// The longest map path (bytes).
pub const MAX_PATH_BYTES: usize = 256;
/// The largest UV scale (a map repeated this many times across a unit of UV).
pub const MAX_UV_SCALE: f32 = 1024.0;
/// The largest normal scale.
pub const MAX_NORMAL_SCALE: f32 = 8.0;
/// The largest height scale (meters of relief at a height of 1).
pub const MAX_HEIGHT_SCALE: f32 = 1.0;
/// The largest emissive factor (linear radiance over the base's 1).
pub const MAX_EMISSIVE: f32 = 1.0e4;

/// Which way a normal map's green channel points.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NormalConvention {
    /// +Y up (OpenGL, glTF, Blender): what the renderer samples.
    #[default]
    OpenGl,
    /// +Y down (DirectX): the renderer flips the green channel.
    DirectX,
}

/// How a material's alpha is used.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlphaMode {
    #[default]
    Opaque,
    /// Cut out where the base's alpha is under [`PbrMaterial::alpha_cutoff`].
    Cutout,
    /// Blended over what is behind it (the translucent pass).
    Blend,
}

/// A physically based material: maps (paths relative to the sidecar) and factors (module docs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PbrMaterial {
    /// Base color (sRGB, alpha in A).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// Tangent-space normal (linear; X in R, Y in G; B is rebuilt and may be absent).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normal: Option<String>,
    pub normal_convention: NormalConvention,
    /// Occlusion, roughness and metalness packed in R, G and B (linear).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orm: Option<String>,
    /// Separate maps (R of each), used where `orm` is not given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occlusion: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roughness_map: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metal_map: Option<String>,
    /// Height (R, 1 the top; parallax at the highest quality).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<String>,
    /// Emissive color (sRGB).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emissive: Option<String>,
    /// Multiplies the base map (linear RGB and alpha).
    pub base_factor: [f32; 4],
    /// Multiplies the roughness map (or is the roughness without one).
    pub roughness: f32,
    /// Multiplies the metalness map (or is the metalness without one).
    pub metal: f32,
    /// How much of the occlusion map is applied (0 none, 1 all).
    pub ao_strength: f32,
    /// Scales the normal map's X and Y.
    pub normal_scale: f32,
    /// Meters of relief at a height of 1.
    pub height_scale: f32,
    /// Multiplies the emissive map (or is the emission without one), linear.
    pub emissive_factor: [f32; 3],
    pub alpha: AlphaMode,
    pub alpha_cutoff: f32,
    pub two_sided: bool,
    /// The maps repeat this many times across a unit of the mesh's UV.
    pub uv_scale: [f32; 2],
}

impl Default for PbrMaterial {
    fn default() -> PbrMaterial {
        PbrMaterial {
            base: None,
            normal: None,
            normal_convention: NormalConvention::OpenGl,
            orm: None,
            occlusion: None,
            roughness_map: None,
            metal_map: None,
            height: None,
            emissive: None,
            base_factor: [1.0; 4],
            roughness: 1.0,
            metal: 1.0,
            ao_strength: 1.0,
            normal_scale: 1.0,
            height_scale: 0.02,
            emissive_factor: [0.0; 3],
            alpha: AlphaMode::Opaque,
            alpha_cutoff: 0.5,
            two_sided: false,
            uv_scale: [1.0; 2],
        }
    }
}

/// Why a sidecar was refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SidecarError {
    #[error("the sidecar is {0} bytes, over the {MAX_SIDECAR_BYTES} read")]
    TooLong(usize),
    #[error("the sidecar is not valid TOML or not a material: {0}")]
    Syntax(String),
    #[error("the sidecar holds {0} materials, over the {MAX_MATERIALS} read")]
    TooMany(usize),
    #[error(
        "material `{material}`: map path `{path}` is not a relative path inside the sidecar's folder"
    )]
    Path { material: String, path: String },
    #[error("material `{material}`: `{field}` is {value}, outside {min}..={max}")]
    Range {
        material: String,
        field: &'static str,
        value: f32,
        min: f32,
        max: f32,
    },
}

/// Whether `p` is a relative path that stays inside its folder: no root, drive or `..`, no empty part, at most
/// [`MAX_PATH_BYTES`], every part plain ([`part_is_plain`]).
fn path_is_inside(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= MAX_PATH_BYTES
        && !p.starts_with(['/', '\\'])
        && !p.contains(':')
        && !p.contains('\0')
        && p.split(['/', '\\']).all(part_is_plain)
}

/// Whether one part of a path names a plain file or folder everywhere: not empty, not `..`, no trailing dot or space (Windows
/// drops them, so `a.png.` opens `a.png`) and no Windows device name (`CON`, `PRN`, `AUX`, `NUL`, `CONIN$`, `CONOUT$`,
/// `COM1`-`COM9` and `LPT1`-`LPT9` with the superscript digits one to three too, in any case and with any extension:
/// `nul.png` opens the device).
fn part_is_plain(part: &str) -> bool {
    if part.is_empty() || part == ".." || part.ends_with(['.', ' ']) {
        return false;
    }
    let stem = part
        .split('.')
        .next()
        .unwrap_or(part)
        .trim_end()
        .to_ascii_uppercase();
    let numbered = |prefix: &str| {
        stem.strip_prefix(prefix).is_some_and(|d| {
            matches!(d.as_bytes(), [b'1'..=b'9']) || matches!(d, "\u{b9}" | "\u{b2}" | "\u{b3}")
        })
    };
    let named = matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    );
    !(named || numbered("COM") || numbered("LPT"))
}

impl PbrMaterial {
    /// Every map path in order: base, normal, orm, occlusion, roughness, metalness, height, emissive.
    pub fn maps(&self) -> [Option<&str>; 8] {
        [
            &self.base,
            &self.normal,
            &self.orm,
            &self.occlusion,
            &self.roughness_map,
            &self.metal_map,
            &self.height,
            &self.emissive,
        ]
        .map(Option::as_deref)
    }

    /// Checks the paths and factors (named `name` in an error).
    ///
    /// # Errors
    /// The first path outside the folder or factor outside its range.
    pub fn check(&self, name: &str) -> Result<(), SidecarError> {
        if let Some(path) = self
            .maps()
            .into_iter()
            .flatten()
            .find(|p| !path_is_inside(p))
        {
            return Err(SidecarError::Path {
                material: name.to_string(),
                path: path.chars().take(MAX_PATH_BYTES).collect(),
            });
        }
        let ranges: [(&'static str, f32, f32, f32); 15] = [
            ("base_factor", self.base_factor[0], 0.0, 1.0),
            ("base_factor", self.base_factor[1], 0.0, 1.0),
            ("base_factor", self.base_factor[2], 0.0, 1.0),
            ("base_factor", self.base_factor[3], 0.0, 1.0),
            ("roughness", self.roughness, 0.0, 1.0),
            ("metal", self.metal, 0.0, 1.0),
            ("ao_strength", self.ao_strength, 0.0, 1.0),
            ("normal_scale", self.normal_scale, 0.0, MAX_NORMAL_SCALE),
            ("height_scale", self.height_scale, 0.0, MAX_HEIGHT_SCALE),
            (
                "emissive_factor",
                self.emissive_factor[0],
                0.0,
                MAX_EMISSIVE,
            ),
            (
                "emissive_factor",
                self.emissive_factor[1],
                0.0,
                MAX_EMISSIVE,
            ),
            (
                "emissive_factor",
                self.emissive_factor[2],
                0.0,
                MAX_EMISSIVE,
            ),
            ("alpha_cutoff", self.alpha_cutoff, 0.0, 1.0),
            (
                "uv_scale",
                self.uv_scale[0],
                1.0 / MAX_UV_SCALE,
                MAX_UV_SCALE,
            ),
            (
                "uv_scale",
                self.uv_scale[1],
                1.0 / MAX_UV_SCALE,
                MAX_UV_SCALE,
            ),
        ];
        match ranges
            .into_iter()
            .find(|&(_, v, min, max)| !(min..=max).contains(&v))
        {
            Some((field, value, min, max)) => Err(SidecarError::Range {
                material: name.to_string(),
                field,
                value,
                min,
                max,
            }),
            None => Ok(()),
        }
    }
}

/// A `.mat.toml` sidecar: one material (beside a texture) or materials by name (beside a shape).
#[derive(Debug, Clone, PartialEq)]
pub enum Sidecar {
    One(Box<PbrMaterial>),
    Named(BTreeMap<String, PbrMaterial>),
}

/// The text form of [`Sidecar::Named`].
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NamedText {
    materials: BTreeMap<String, PbrMaterial>,
}

impl Sidecar {
    /// The sidecar of `text`, checked (module docs).
    ///
    /// # Errors
    /// Text over [`MAX_SIDECAR_BYTES`], not TOML, not a material, over [`MAX_MATERIALS`] materials, or a material that fails
    /// [`PbrMaterial::check`].
    pub fn parse(text: &str) -> Result<Sidecar, SidecarError> {
        if text.len() > MAX_SIDECAR_BYTES {
            return Err(SidecarError::TooLong(text.len()));
        }
        let table: toml::Table =
            toml::from_str(text).map_err(|e| SidecarError::Syntax(e.message().to_string()))?;
        let syntax = |e: toml::de::Error| SidecarError::Syntax(e.message().to_string());
        let sidecar = if table.contains_key("materials") {
            let named: NamedText = table.try_into().map_err(syntax)?;
            if named.materials.len() > MAX_MATERIALS {
                return Err(SidecarError::TooMany(named.materials.len()));
            }
            Sidecar::Named(named.materials)
        } else {
            Sidecar::One(Box::new(table.try_into().map_err(syntax)?))
        };
        for (name, m) in sidecar.iter() {
            m.check(name)?;
        }
        Ok(sidecar)
    }

    /// The sidecar as text ([`Sidecar::parse`] reads it back equal).
    pub fn to_text(&self) -> String {
        let text = match self {
            Sidecar::One(m) => toml::to_string(m.as_ref()),
            Sidecar::Named(materials) => toml::to_string(&NamedText {
                materials: materials.clone(),
            }),
        };
        text.expect("a material's fields are all TOML values")
    }

    /// Its materials with their names (a single material's is empty).
    pub fn iter(&self) -> impl Iterator<Item = (&str, &PbrMaterial)> {
        let (one, named) = match self {
            Sidecar::One(m) => (Some(("", m.as_ref())), None),
            Sidecar::Named(map) => (None, Some(map.iter().map(|(k, v)| (k.as_str(), v)))),
        };
        one.into_iter().chain(named.into_iter().flatten())
    }

    /// The material for a shape's material `name`: the named one, or the single one whatever the name.
    pub fn material(&self, name: &str) -> Option<&PbrMaterial> {
        match self {
            Sidecar::One(m) => Some(m.as_ref()),
            Sidecar::Named(map) => map.get(name),
        }
    }
}

/// The sidecar's file name for a texture or shape file name: its extension replaced (`wall.png` -> `wall.mat.toml`).
pub fn sidecar_name(file: &str) -> String {
    let stem = match file.rfind('.') {
        Some(dot) if !file[dot..].contains(['/', '\\']) => &file[..dot],
        _ => file,
    };
    format!("{stem}.mat.toml")
}

#[cfg(test)]
#[path = "pbr_tests.rs"]
mod tests;
