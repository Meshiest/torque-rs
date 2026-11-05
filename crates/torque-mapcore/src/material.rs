//! Materials and the images they name.
//!
//! Legacy maps name their materials by texture name alone (a [`crate::geometry::Surfaces`] key, looked up
//! beside the model's file); a BSP importer would fill [`Material`] from its material files.

use std::collections::BTreeMap;

/// What a material is drawn as (bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct MaterialFlags(pub u32);

impl MaterialFlags {
    pub const TRANSLUCENT: MaterialFlags = MaterialFlags(1);
    pub const ALPHA_TEST: MaterialFlags = MaterialFlags(2);
    pub const ADDITIVE: MaterialFlags = MaterialFlags(4);
    /// Shows the sky through it (drawn as sky, not as a surface).
    pub const SKY: MaterialFlags = MaterialFlags(8);
    pub const WATER: MaterialFlags = MaterialFlags(16);
    /// Never drawn (it may still collide).
    pub const NO_DRAW: MaterialFlags = MaterialFlags(32);
    /// Not lit (full bright).
    pub const UNLIT: MaterialFlags = MaterialFlags(64);
    pub const TWO_SIDED: MaterialFlags = MaterialFlags(128);
    /// Its texture sampled with nearest-neighbor filtering (pixel art): sharp texels up close, its mipmaps still taken far
    /// away.
    pub const NEAREST: MaterialFlags = MaterialFlags(256);

    pub fn has(self, other: MaterialFlags) -> bool {
        self.0 & other.0 == other.0
    }

    /// Both sets of bits.
    pub fn with(self, other: MaterialFlags) -> MaterialFlags {
        MaterialFlags(self.0 | other.0)
    }
}

/// How a material's colour meets what is behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Blend {
    #[default]
    Opaque,
    /// `src * a + dst * (1 - a)`.
    Alpha,
    /// `src + dst`.
    Add,
    /// `src * dst`.
    Multiply,
}

/// A material: a texture name a layer (as the format names it; the asset source resolves it), its flags and blending, and the
/// importer's own key-values (kept for the format's later steps, never read by a consumer).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Material {
    pub name: String,
    pub base: Option<String>,
    /// The second base texture of a blend (a blend between two base textures).
    pub base2: Option<String>,
    pub bump: Option<String>,
    pub detail: Option<String>,
    pub env_mask: Option<String>,
    pub flags: MaterialFlags,
    pub blend: Blend,
    pub keys: BTreeMap<String, String>,
}

/// How an image's texels are stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Rgba8,
    /// DXT1, DXT3, DXT5: 4 x 4 blocks of 8, 16 and 16 bytes.
    Bc1,
    Bc2,
    Bc3,
}

/// A decoded or block-compressed image: its levels from the full size down (`levels[frame][mip]`; one frame for a still image,
/// six faces for a cube in order +x -x +y -y +z -z).
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub format: ImageFormat,
    pub levels: Vec<Vec<Vec<u8>>>,
}

/// How an [`EncodedImage`]'s bytes are encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Png,
}

/// An image still in its file's encoding, with the size its header gives (decoded by the consumer that draws it, within its
/// own caps).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedImage {
    pub encoding: Encoding,
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}
