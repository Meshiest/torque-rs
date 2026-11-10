//! VMT materials: a KeyValues text ([`crate::kv`]) whose root key is the shader (`LightmappedGeneric`, `VertexLitGeneric`,
//! `WorldVertexTransition`, `UnlitGeneric`, ...) and whose block holds the shader's parameters (`$basetexture`, `$translucent`, ...).
//!
//! A `patch` material (what a map compiler writes for brushes that take a cube map) names another with `include` and changes
//! its parameters with `insert` and `replace` blocks: [`Vmt::patched`] applies them over the included material.
//!
//! The parameters read here are the common ones a renderer needs: the base textures (and the second of a blend), the bump
//! map, translucency, alpha testing, additive blending, two-sided drawing, self-illumination, the color and alpha, the
//! environment map, the detail texture, the surface property, and the tool flags (`%compilenodraw`, `%compilesky`, ...).

use std::collections::BTreeMap;

use crate::kv::{self, Pair, Value};
use crate::read::{Result, cap, invalid};

/// The largest material file read.
pub const MAX_FILE_BYTES: u64 = 1 << 20;
/// The most `include`s followed for one patch material.
pub const MAX_PATCH_DEPTH: usize = 8;

/// A material: its shader and its parameters (keys in lower case, string values; blocks such as `Proxies` kept apart).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Vmt {
    /// The shader, in lower case.
    pub shader: String,
    pub params: BTreeMap<String, String>,
    /// The blocks inside the material (`proxies`, fallbacks), by lower-case key.
    pub blocks: BTreeMap<String, Vec<Pair>>,
}

/// How a surface is blended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blend {
    Opaque,
    /// Alpha tested at the given cutoff (0..1).
    AlphaTest,
    Translucent,
    Additive,
}

impl Vmt {
    /// Reads a material's text.
    pub fn parse(text: &[u8]) -> Result<Vmt> {
        cap("the VMT file's size", text.len() as u64, MAX_FILE_BYTES)?;
        let kv = kv::parse(text)?;
        let Some(root) = kv.first() else {
            return invalid("an empty material");
        };
        let mut m = Vmt {
            shader: root.key.to_ascii_lowercase(),
            ..Vmt::default()
        };
        if let Some(block) = root.block() {
            for p in block {
                match &p.value {
                    Value::Str(s) => {
                        m.params
                            .entry(p.key.to_ascii_lowercase())
                            .or_insert_with(|| s.clone());
                    }
                    Value::Block(b) => {
                        m.blocks.insert(p.key.to_ascii_lowercase(), b.clone());
                    }
                }
            }
            // Hardware-level blocks (`">=dx90"`, `">dx80"`, `">=dx90_20b"`, `"GPU>=1"`): a current renderer meets every
            // lower bound and no upper one, so the `>` blocks' parameters apply over the others and the `<` blocks are left.
            for p in block {
                let key = p.key.to_ascii_lowercase();
                let lower_bound = key.starts_with('>') || key.starts_with("gpu>");
                if let (true, Some(b)) = (lower_bound, p.block()) {
                    for q in b {
                        if let Value::Str(v) = &q.value {
                            m.params.insert(q.key.to_ascii_lowercase(), v.clone());
                        }
                    }
                }
            }
        }
        Ok(m)
    }

    /// Whether this is a `patch` material.
    pub fn is_patch(&self) -> bool {
        self.shader == "patch"
    }

    /// A `patch` material applied over the material it includes, read with `load(path)` (followed up to
    /// [`MAX_PATCH_DEPTH`] times); any other material as it is.
    pub fn patched(self, load: &mut dyn FnMut(&str) -> Option<Vmt>) -> Result<Vmt> {
        let mut m = self;
        for _ in 0..MAX_PATCH_DEPTH {
            if !m.is_patch() {
                return Ok(m);
            }
            let Some(include) = m.params.get("include").cloned() else {
                return invalid("a patch material with no include");
            };
            let Some(mut base) = load(&include) else {
                return Err(crate::read::Error::Missing(include));
            };
            for key in ["insert", "replace"] {
                if let Some(b) = m.blocks.get(key) {
                    for p in b {
                        if let Value::Str(s) = &p.value {
                            base.params.insert(p.key.to_ascii_lowercase(), s.clone());
                        }
                    }
                }
            }
            m = base;
        }
        invalid("patch materials include each other too deep")
    }

    /// A parameter's string.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.params
            .get(&key.to_ascii_lowercase())
            .map(String::as_str)
    }

    /// A parameter as a flag (`1`, a non-zero number).
    pub fn flag(&self, key: &str) -> bool {
        self.get(key)
            .and_then(|s| s.trim().parse::<f32>().ok())
            .is_some_and(|v| v != 0.0)
    }

    /// A parameter as a number.
    pub fn float(&self, key: &str) -> Option<f32> {
        self.get(key).and_then(|s| s.trim().parse().ok())
    }

    /// A parameter as a color: `[r g b]` (0..1) or `{r g b}` (0..255).
    pub fn color(&self, key: &str) -> Option<[f32; 3]> {
        let s = self.get(key)?.trim();
        let scale = if s.starts_with('{') { 1.0 / 255.0 } else { 1.0 };
        let v: Vec<f32> = s
            .trim_matches(|c| matches!(c, '[' | ']' | '{' | '}'))
            .split_whitespace()
            .filter_map(|t| t.parse().ok())
            .collect();
        match v.as_slice() {
            [r, g, b, ..] => Some([r * scale, g * scale, b * scale]),
            [x] => Some([x * scale; 3]),
            _ => None,
        }
    }

    /// The base texture's path (`materials/<name>.vtf`).
    pub fn base_texture(&self) -> Option<String> {
        self.get("$basetexture").map(texture_path)
    }

    /// The second base texture of a blend (`WorldVertexTransition`).
    pub fn base_texture2(&self) -> Option<String> {
        self.get("$basetexture2").map(texture_path)
    }

    /// How the surface is blended.
    pub fn blend(&self) -> Blend {
        if self.flag("$additive") {
            Blend::Additive
        } else if self.flag("$translucent") {
            Blend::Translucent
        } else if self.flag("$alphatest") {
            Blend::AlphaTest
        } else {
            Blend::Opaque
        }
    }

    /// Whether both faces are drawn.
    pub fn two_sided(&self) -> bool {
        self.flag("$nocull")
    }

    /// Whether the surface is drawn without lighting (`UnlitGeneric`). (`$selfillum` lights only where the base texture's alpha
    /// says, so it is not unlit.)
    pub fn unlit(&self) -> bool {
        self.shader.starts_with("unlit")
    }

    /// Whether the surface is a tool's (never drawn: `%compilenodraw`, clip, triggers, hints, skip) or the sky.
    pub fn tool(&self) -> bool {
        [
            "%compilenodraw",
            "%compileclip",
            "%compiletrigger",
            "%compilehint",
            "%compileskip",
            "%compileplayerclip",
            "%compilenpcclip",
            "%compileinvisible",
            "%compileorigin",
            "%compilefog",
            "%compilenonsolid",
            "%compilesky",
            "%compile2dsky",
        ]
        .iter()
        .any(|k| self.flag(k))
    }

    /// Whether this is water (drawn as a translucent sheet).
    pub fn water(&self) -> bool {
        self.shader == "water" || self.flag("%compilewater")
    }
}

/// A texture name as its file's path: `materials/` before it, `.vtf` after (the case and slashes normalized).
pub fn texture_path(name: &str) -> String {
    let n = name.trim().replace('\\', "/").to_ascii_lowercase();
    let n = n.trim_start_matches('/');
    let n = n.strip_prefix("materials/").unwrap_or(n);
    let n = n.strip_suffix(".vtf").unwrap_or(n);
    format!("materials/{n}.vtf")
}

/// A material name as its file's path: `materials/` before it, `.vmt` after.
pub fn material_path(name: &str) -> String {
    let n = name.trim().replace('\\', "/").to_ascii_lowercase();
    let n = n.trim_start_matches('/');
    let n = n.strip_prefix("materials/").unwrap_or(n);
    let n = n.strip_suffix(".vmt").unwrap_or(n);
    format!("materials/{n}.vmt")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_params() {
        let m = Vmt::parse(
            b"\"VertexLitGeneric\" { \"$basetexture\" \"Models\\Props\\Crate\" \"$alphatest\" 1 $color \"{255 128 0}\" }",
        )
        .unwrap();
        assert_eq!(m.shader, "vertexlitgeneric");
        assert_eq!(
            m.base_texture().as_deref(),
            Some("materials/models/props/crate.vtf")
        );
        assert_eq!(m.blend(), Blend::AlphaTest);
        let c = m.color("$color").unwrap();
        assert!((c[1] - 128.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn takes_the_hardware_blocks_a_current_renderer_meets() {
        let m = Vmt::parse(
            b"VertexLitGeneric { $bumpmap n \">=dx90_20b\" { $baseTexture new } \"<dx90_20b\" { $baseTexture old } }",
        )
        .unwrap();
        assert_eq!(m.get("$basetexture"), Some("new"));
    }

    #[test]
    fn applies_a_patch() {
        let patch = Vmt::parse(
            b"patch { include \"materials/concrete/a.vmt\" insert { $envmap \"maps/x/c0\" } replace { $basetexture b2 } }",
        )
        .unwrap();
        let m = patch
            .patched(&mut |p| {
                assert_eq!(p, "materials/concrete/a.vmt");
                Vmt::parse(b"LightmappedGeneric { $basetexture b1 $surfaceprop concrete }").ok()
            })
            .unwrap();
        assert_eq!(m.shader, "lightmappedgeneric");
        assert_eq!(m.get("$basetexture"), Some("b2"));
        assert_eq!(m.get("$envmap"), Some("maps/x/c0"));
        assert_eq!(m.get("$surfaceprop"), Some("concrete"));
    }

    #[test]
    fn a_patch_loop_ends() {
        let p = Vmt::parse(b"patch { include a }").unwrap();
        let again = p.clone();
        assert!(p.patched(&mut |_| Some(again.clone())).is_err());
    }
}
