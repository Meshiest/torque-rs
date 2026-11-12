//! The images ropes are textured with: an asset the engine's loaders cache ([`ImageData`], read from `.png` and `.jpg` files by the
//! plugin's loader), decoded by `torque-images` and kept as RGBA8 with its mip levels.

use torque_images::mip::{self, RgbExtrude};
use torque_images::{Bitmap, Format, Kind};

/// The most bytes of a texture file the loader reads.
pub const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;
/// The longest side of a texture (pixels).
pub const MAX_IMAGE_SIDE: u32 = 2048;

/// A texture: RGBA8 with its mip levels, level 0 first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageData {
    /// Width of level 0.
    pub width: u32,
    /// Height of level 0.
    pub height: u32,
    /// The levels, each `width x height x 4` bytes of its own size (halved down to the side's end), rows from the top.
    pub levels: Vec<Vec<u8>>,
}

impl ImageData {
    /// A texture from the pixels of level 0 (`width x height` RGBA8 texels), with the mip levels the renderer builds for its own
    /// textures. An empty image, one with the wrong number of bytes and one with a side past [`MAX_IMAGE_SIDE`] are errors.
    pub fn from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Result<ImageData, String> {
        if width == 0 || height == 0 || width > MAX_IMAGE_SIDE || height > MAX_IMAGE_SIDE {
            return Err(format!(
                "a texture is 1 to {MAX_IMAGE_SIDE} pixels a side, not {width} x {height}"
            ));
        }
        if rgba.len() != width as usize * height as usize * 4 {
            return Err("the texture's bytes do not fit its size".to_string());
        }
        let bitmap = Bitmap {
            width,
            height,
            format: Format::Rgba,
            bits: rgba,
        };
        let chain = mip::extrude(&bitmap, RgbExtrude::C, false).map_err(|e| e.to_string())?;
        // The chain ends with a second 1 x 1 level (TGE's loop is a do-while); the texture's levels end at the first.
        let count = (u32::BITS - width.max(height).leading_zeros()) as usize;
        let levels = (0..count)
            .map(|i| chain.level(i).map(<[u8]>::to_vec))
            .collect::<Option<Vec<_>>>()
            .ok_or("the texture's mip levels did not build")?;
        Ok(ImageData {
            width,
            height,
            levels,
        })
    }

    /// One white texel: what an untextured rope is drawn through.
    pub fn white() -> ImageData {
        ImageData {
            width: 1,
            height: 1,
            levels: vec![vec![255; 4]],
        }
    }

    /// A soft beam made in code (64 x 16 texels): a hot core fading to red at the edges across the beam (v), rippling along it (u). For
    /// energy beams and lasers: with `additive`, `emissive` and a `textureScroll` it runs along the beam. Not a file: nothing is read.
    pub fn beam() -> ImageData {
        let (width, height) = (64u32, 16u32);
        let mut texels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let (u, v) = (
                    (x as f32 + 0.5) / width as f32,
                    (y as f32 + 0.5) / height as f32,
                );
                let across = (-(((v - 0.5) / 0.22).powi(2))).exp();
                let ripple = 0.7 + 0.3 * (std::f32::consts::TAU * u * 4.0).sin();
                let mix = |a: f32, b: f32| a + (b - a) * across;
                let rgba = [
                    mix(1.0, 1.0),
                    mix(0.18, 0.92),
                    mix(0.12, 0.8),
                    across * ripple,
                ];
                texels.extend(rgba.map(|c| (c * 255.0).round().clamp(0.0, 255.0) as u8));
            }
        }
        ImageData::from_rgba(width, height, texels).expect("the beam's texels fit its size")
    }

    /// Decodes the file `name` (its extension says which: `.png` or `.jpg`) from its bytes.
    pub fn decode(name: &str, bytes: &[u8]) -> Result<ImageData, String> {
        let kind = Kind::of_name(name).ok_or_else(|| format!("{name} is not a .png or .jpg"))?;
        let bitmap = torque_images::decode(kind, bytes).map_err(|e| e.to_string())?;
        ImageData::from_rgba(bitmap.width, bitmap.height, bitmap.to_rgba8())
    }

    /// The number of mip levels.
    pub fn mip_levels(&self) -> u32 {
        self.levels.len() as u32
    }
}

/// What the asset loader keeps of a texture file: the image, shared (a rope's world holds the same allocation, not a copy).
#[derive(Clone, Debug)]
pub struct SharedImage(pub std::sync::Arc<ImageData>);

/// A PNG of `width` x `height` RGBA8 texels (row 0 at the top), for textures made in code (a test's, a demo's): the deflate stream is
/// stored, not compressed, so the file is a little more than the texels' size. None for an empty image or one whose bytes do not fit.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    if width == 0 || height == 0 || rgba.len() != width as usize * height as usize * 4 {
        return None;
    }
    let chunk = |out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]| {
        out.extend((data.len() as u32).to_be_bytes());
        out.extend(kind);
        out.extend(data);
        out.extend(torque_bytes::crc32_parts(&[kind, data]).to_be_bytes());
    };
    let mut raw = Vec::with_capacity(rgba.len() + height as usize);
    for row in rgba.chunks(width as usize * 4) {
        raw.push(0);
        raw.extend(row);
    }
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, block) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        z.extend((block.len() as u16).to_le_bytes());
        z.extend((!(block.len() as u16)).to_le_bytes());
        z.extend(*block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for byte in &raw {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend(((b << 16) | a).to_be_bytes());
    let mut header = Vec::new();
    header.extend(width.to_be_bytes());
    header.extend(height.to_be_bytes());
    header.extend([8, 6, 0, 0, 0]);
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    Some(out)
}

/// How far a texture that runs `speed` repeats a second has moved along the rope after `seconds` of the effects' clock, in repeats
/// (0 to 1): what the shaders take off a vertex's u.
pub fn scroll_offset(speed: f32, seconds: f32) -> f32 {
    let x = speed * seconds;
    if x.is_finite() { x - x.floor() } else { 0.0 }
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_encoded_png_decodes_to_the_same_texels() {
        let rgba: Vec<u8> = (0..5 * 3 * 4).map(|i| (i * 7 % 256) as u8).collect();
        let png = encode_png(5, 3, &rgba).unwrap();
        let image = ImageData::decode("made.png", &png).unwrap();
        assert_eq!((image.width, image.height), (5, 3));
        assert_eq!(image.levels[0], rgba);
        assert!(encode_png(0, 3, &[]).is_none());
        assert!(encode_png(2, 2, &[0; 3]).is_none());
        // More than one stored block.
        let big = vec![9u8; 200 * 200 * 4];
        let image = ImageData::decode("big.png", &encode_png(200, 200, &big).unwrap()).unwrap();
        assert_eq!(image.levels[0], big);
    }

    #[test]
    fn the_scroll_runs_the_texture_along_by_the_clock_and_wraps() {
        assert_eq!(scroll_offset(0.0, 12.5), 0.0);
        assert!((scroll_offset(2.0, 0.25) - 0.5).abs() < 1e-6);
        assert!(
            (scroll_offset(2.0, 0.75) - 0.5).abs() < 1e-6,
            "one repeat later is the same place"
        );
        assert!(
            (scroll_offset(-1.0, 0.25) - 0.75).abs() < 1e-6,
            "a negative speed runs toward the start"
        );
        assert_eq!(scroll_offset(f32::NAN, 1.0), 0.0);
    }

    use super::*;

    #[test]
    fn the_levels_halve_down_to_one_texel() {
        let image = ImageData::from_rgba(8, 4, vec![200; 8 * 4 * 4]).unwrap();
        assert_eq!(image.mip_levels(), 4);
        let sizes: Vec<usize> = image.levels.iter().map(Vec::len).collect();
        assert_eq!(sizes, [8 * 4 * 4, 4 * 2 * 4, 2 * 4, 4]);
        assert_eq!(
            ImageData::from_rgba(1, 1, vec![9; 4]).unwrap().mip_levels(),
            1
        );
        assert_eq!(ImageData::white().levels[0], [255; 4]);
    }

    #[test]
    fn a_wrong_size_or_a_wrong_file_is_an_error_not_a_panic() {
        assert!(ImageData::from_rgba(0, 4, vec![]).is_err());
        assert!(ImageData::from_rgba(4, 4, vec![0; 3]).is_err());
        assert!(ImageData::from_rgba(MAX_IMAGE_SIDE + 1, 1, vec![0; 4]).is_err());
        assert!(ImageData::decode("rope.png", b"not a png").is_err());
        assert!(ImageData::decode("rope.gif", b"GIF89a").is_err());
    }
}
