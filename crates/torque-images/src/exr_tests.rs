use std::io::Cursor;

use exr::prelude::{
    AnyChannel, AnyChannels, Blocks, Compression, Encoding, FlatSamples, Image, ImageAttributes,
    IntegerBounds, Layer, LayerAttributes, LineOrder, SmallVec, WritableImage, f16,
};

use super::*;

/// An EXR file of one float channel `name` of `w` x `h`, `compression` and, when given, tiles of `tile`.
pub(crate) fn file(
    name: &str,
    w: usize,
    h: usize,
    samples: FlatSamples,
    compression: Compression,
    tile: Option<usize>,
) -> Vec<u8> {
    let blocks = tile.map_or(Blocks::ScanLines, |t| Blocks::Tiles(Vec2(t, t)));
    let layer = Layer::new(
        (w, h),
        LayerAttributes::default(),
        Encoding {
            compression,
            blocks,
            line_order: LineOrder::Increasing,
        },
        AnyChannels::sort(SmallVec::from_vec(vec![AnyChannel::new(name, samples)])),
    );
    let mut out = Vec::new();
    Image::from_layer(layer)
        .write()
        .to_buffered(Cursor::new(&mut out))
        .unwrap();
    out
}

fn wave(w: usize, h: usize) -> Vec<u16> {
    (0..w * h)
        .map(|i| ((i * 2_654_435_761) % 65_536) as u16)
        .collect()
}

#[test]
fn a_float_channel_reads_back_within_one_step() {
    let (w, h) = (37, 21);
    let want = wave(w, h);
    let f32s: Vec<f32> = want.iter().map(|&v| f32::from(v) / 65_535.0).collect();
    for c in [
        Compression::Uncompressed,
        Compression::RLE,
        Compression::ZIP1,
        Compression::ZIP16,
        Compression::PIZ,
    ] {
        for tile in [None, Some(16)] {
            let bytes = file("Y", w, h, FlatSamples::F32(f32s.clone()), c, tile);
            let got = read_r16(&bytes, 4096).unwrap();
            assert_eq!((got.width, got.height), (w as u32, h as u32));
            let worst = got
                .texels
                .iter()
                .zip(&want)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap();
            assert!(worst <= 1, "{c:?} {tile:?}: {worst}");
        }
    }
}

#[test]
fn half_floats_and_values_out_of_range_are_clamped() {
    let vals = [-1.0f32, 0.0, 0.25, 1.0, 7.0, f32::NAN];
    let bytes = file(
        "R",
        vals.len(),
        1,
        FlatSamples::F16(vals.iter().map(|&v| f16::from_f32(v)).collect()),
        Compression::Uncompressed,
        None,
    );
    let got = read_r16(&bytes, 4096).unwrap();
    assert_eq!(got.texels, vec![0, 0, 16_384, 65_535, 65_535, 0]);
    assert_eq!(&got.to_rgba8()[8..12], &[64, 64, 64, 255]);
}

#[test]
fn files_past_the_side_or_not_exr_are_refused() {
    let bytes = file(
        "Y",
        64,
        8,
        FlatSamples::F32(vec![0.5; 512]),
        Compression::ZIP1,
        None,
    );
    assert!(matches!(read_r16(&bytes, 32), Err(ExrError::Size { .. })));
    assert_eq!(size_of(&bytes, 4096), Ok((64, 8)));
    assert!(read_r16(b"not an exr", 4096).is_err());
    assert!(read_r16(&bytes[..bytes.len() / 2], 4096).is_err());
}

/// A header of `w` x `h` pixels of `channels` float channels, in `blocks` with `compression`.
fn header_of(
    (w, h): (usize, usize),
    channels: usize,
    compression: Compression,
    blocks: exr::meta::BlockDescription,
) -> exr::meta::header::Header {
    use exr::meta::attribute::{ChannelDescription, SampleType};
    let list = (0..channels)
        .map(|i| ChannelDescription::named(format!("c{i}").as_str(), SampleType::F32))
        .collect();
    exr::meta::header::Header::new("l".into(), (w, h), list).with_encoding(
        compression,
        blocks,
        LineOrder::Increasing,
    )
}

/// Tiles of `side` square, the other settings the crate's default.
fn tiles(side: usize) -> exr::meta::BlockDescription {
    let base = exr::meta::header::Header::new("l".into(), (1, 1), SmallVec::new());
    match base.blocks {
        exr::meta::BlockDescription::Tiles(t) => {
            exr::meta::BlockDescription::Tiles(exr::meta::attribute::TileDescription {
                tile_size: Vec2(side, side),
                ..t
            })
        }
        scan_lines => scan_lines,
    }
}

#[test]
fn a_blocks_decode_memory_is_its_area_at_the_channels_bytes_a_pixel() {
    let big = (4096, 4096);
    // A 4096 x 4096 tile of 16 float channels: 1 GiB asked of a file of a few hundred bytes.
    let hostile = header_of(big, 16, Compression::ZIP16, tiles(4096));
    assert_eq!(block_bytes(&hostile), 1 << 30);
    assert!(block_bytes(&hostile) > MAX_BLOCK_BYTES);
    // Scan lines: the width by the compression's lines a block.
    let piz = header_of(
        big,
        16,
        Compression::PIZ,
        exr::meta::BlockDescription::ScanLines,
    );
    assert_eq!(block_bytes(&piz), 4096 * 32 * 64);
    // A tile past the layer counts as the layer.
    let small = header_of((8, 8), 16, Compression::ZIP16, tiles(4096));
    assert_eq!(block_bytes(&small), 8 * 8 * 64);
    // A height map: one channel, 4096 square, tiles of 64 or 16 lines.
    for h in [
        header_of(
            big,
            1,
            Compression::ZIP16,
            exr::meta::BlockDescription::ScanLines,
        ),
        header_of(big, 1, Compression::RLE, tiles(64)),
    ] {
        assert!(block_bytes(&h) <= MAX_BLOCK_BYTES);
    }
}

#[test]
fn a_file_whose_block_is_over_the_limit_is_refused_before_any_decode() {
    let bytes = file(
        "Y",
        64,
        64,
        FlatSamples::F32(vec![0.5; 64 * 64]),
        Compression::ZIP1,
        Some(32),
    );
    // Its tiles take 32 x 32 x 4 bytes.
    assert!(checked_within(&bytes, 4096, 4096).is_ok());
    assert_eq!(
        checked_within(&bytes, 4096, 4095),
        Err(ExrError::Block(4096))
    );
    assert!(read_r16(&bytes, 4096).is_ok());
}

#[test]
fn a_file_past_the_size_taken_or_with_too_many_parts_is_refused() {
    assert_eq!(
        size_of(&vec![0; MAX_FILE_BYTES + 1], 4096),
        Err(ExrError::File(MAX_FILE_BYTES + 1))
    );
    let part = |i: usize| {
        Layer::new(
            (4, 4),
            LayerAttributes::named(format!("l{i}").as_str()),
            Encoding::FAST_LOSSLESS,
            AnyChannels::sort(SmallVec::from_vec(vec![AnyChannel::new(
                "Y",
                FlatSamples::F32(vec![0.5; 16]),
            )])),
        )
    };
    let write = |n: usize| {
        let mut out = Vec::new();
        Image::from_layers(
            ImageAttributes::new(IntegerBounds::from_dimensions((4, 4))),
            (0..n).map(part).collect::<SmallVec<_>>(),
        )
        .write()
        .to_buffered(Cursor::new(&mut out))
        .unwrap();
        out
    };
    assert!(size_of(&write(MAX_PARTS), 4096).is_ok());
    assert_eq!(
        size_of(&write(MAX_PARTS + 1), 4096),
        Err(ExrError::Parts(MAX_PARTS + 1))
    );
}
