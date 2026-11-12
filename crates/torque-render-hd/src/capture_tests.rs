//! The environment cubes' GGX prefilter (`prefilter.wgsl`) against the device's own cube sampling and the lobes the levels stand
//! for: a cube whose level 0 holds 0.5 + 0.5 d (the direction `d` of each texel as the color), filtered down its whole chain. A
//! lobe about a direction `n` filters a linear function of the direction into one along `n` again, shortened by the lobe's mean
//! N.L: at every texel of every face and level the color less 0.5 must point along the texel's own direction (a face turned or
//! flipped against the device's convention, or a seam read from the wrong face, points elsewhere), and each level's mean length
//! must be the product of the lobes the chain applied down to it (a level filtered with another roughness is longer or shorter).

use torque_render_core::{Gpu, wgpu};
use torque_render_scene::hd::reflect::{CAPTURE_MIPS, CAPTURE_SIZE};

use super::{Cube, PrefilterPipe, mip_size};
use crate::halves;

/// The direction a cube face's point (u, v in -1..1, v down) is sampled along (the cube map convention).
fn face_dir(face: u32, u: f32, v: f32) -> [f32; 3] {
    let d = match face {
        0 => [1.0, -v, -u],
        1 => [-1.0, -v, u],
        2 => [u, 1.0, v],
        3 => [u, -1.0, -v],
        4 => [u, -v, 1.0],
        _ => [-u, -v, -1.0],
    };
    let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    d.map(|c| c / l)
}

/// The share of a linear function's slope a level keeps after its filter (the shader's: the lobe that widens the level above's
/// GGX alpha to this level's, `PREFILTER_SAMPLES` of the Hammersley set weighted by N.L; 1 where the lobe is under half a texel
/// and the level is the box of the one above). The roughnesses are the documented schedule, (m / (levels - 1))^2.
fn level_keeps(m: u32) -> f64 {
    let levels = f64::from(CAPTURE_MIPS - 1);
    let alpha = |m: f64| (m / levels).powi(4);
    let a = (alpha(f64::from(m)).powi(2) - alpha(f64::from(m - 1)).powi(2))
        .max(0.0)
        .sqrt();
    let size = f64::from(CAPTURE_SIZE >> m);
    if a * size <= 0.5 {
        return 1.0;
    }
    let n = 32u32;
    let (mut sum, mut weight) = (0.0, 0.0);
    for i in 0..n {
        let xi = (
            f64::from(i) / f64::from(n),
            f64::from(i.reverse_bits()) * 2.328_306_436_538_696_3e-10,
        );
        let cos_h = ((1.0 - xi.1) / (1.0 + (a * a - 1.0) * xi.1)).sqrt();
        let nl = 2.0 * cos_h * cos_h - 1.0;
        if nl > 0.0 {
            sum += nl * nl;
            weight += nl;
        }
    }
    sum / weight
}

/// An IEEE half of `v` (a normal number in 0..2, as here).
fn half(v: f32) -> u16 {
    let bits = v.to_bits();
    let exp = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    if exp <= 0 {
        return 0;
    }
    let mant = (bits >> 13) & 0x3ff;
    ((exp as u32) << 10 | mant) as u16
}

/// Face `face`'s level `level` of `texture`, RGBA as floats.
fn read(gpu: &Gpu, texture: &wgpu::Texture, face: u32, level: u32) -> Vec<[f32; 4]> {
    let (w, h) = mip_size(texture.width(), texture.height(), level);
    let row = 8 * w;
    let padded =
        row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let device = gpu.device();
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("prefilter readback"),
        size: u64::from(padded) * u64::from(h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: level,
            origin: wgpu::Origin3d {
                x: 0,
                y: 0,
                z: face,
            },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue().submit([encoder.finish()]);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("mapped"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("the copy finishes");
    let data = slice.get_mapped_range().expect("the mapped bytes");
    (0..h as usize)
        .flat_map(|y| halves(&data[y * padded as usize..y * padded as usize + row as usize]))
        .collect()
}

#[test]
fn every_prefiltered_texel_points_along_its_own_direction() {
    let gpu = match Gpu::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let device = gpu.device();
    let pipe = PrefilterPipe::new(device, None);
    let cube = Cube::prefiltered(device, "test cube", CAPTURE_SIZE, CAPTURE_MIPS, &pipe);
    let s = CAPTURE_SIZE;
    for face in 0..6 {
        let mut bytes = Vec::with_capacity((s * s * 8) as usize);
        for y in 0..s {
            for x in 0..s {
                let (u, v) = (
                    (x as f32 + 0.5) / s as f32 * 2.0 - 1.0,
                    (y as f32 + 0.5) / s as f32 * 2.0 - 1.0,
                );
                let d = face_dir(face, u, v);
                for c in [0.5 + 0.5 * d[0], 0.5 + 0.5 * d[1], 0.5 + 0.5 * d[2], 1.0] {
                    bytes.extend(half(c).to_le_bytes());
                }
            }
        }
        gpu.queue().write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &cube.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: face,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(8 * s),
                rows_per_image: Some(s),
            },
            wgpu::Extent3d {
                width: s,
                height: s,
                depth_or_array_layers: 1,
            },
        );
    }
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    cube.encode_prefilter(&mut encoder, 0..6, &pipe);
    gpu.queue().submit([encoder.finish()]);

    let mut keeps = 1.0;
    for level in 1..CAPTURE_MIPS {
        let (w, _) = mip_size(s, s, level);
        let mut worst = 1.0f32;
        let (mut along, mut texels) = (0.0f64, 0u32);
        for face in 0..6 {
            for (k, c) in read(&gpu, &cube.texture, face, level).iter().enumerate() {
                let (x, y) = (k as u32 % w, k as u32 / w);
                let n = face_dir(
                    face,
                    (x as f32 + 0.5) / w as f32 * 2.0 - 1.0,
                    (y as f32 + 0.5) / w as f32 * 2.0 - 1.0,
                );
                let g = [0, 1, 2].map(|i| 2.0 * c[i] - 1.0);
                let len = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt();
                assert!(
                    len <= 1.01,
                    "level {level} face {face} texel ({x}, {y}): {len} longer than the source's"
                );
                let g_n = g[0] * n[0] + g[1] * n[1] + g[2] * n[2];
                worst = worst.min(g_n / len.max(1e-6));
                along += f64::from(g_n);
                texels += 1;
                // Within the half floats' rounding over the levels.
                assert!((c[3] - 1.0).abs() < 4e-3, "the alpha is kept: {}", c[3]);
            }
        }
        keeps *= level_keeps(level);
        let mean = along / f64::from(texels);
        println!(
            "level {level}: least cosine to the texel's direction {worst:.4}, mean length {mean:.4}, the lobes' {keeps:.4}"
        );
        assert!(
            worst > 0.98,
            "level {level}: a texel points away from its direction ({worst})"
        );
        // The bilinear taps between a coarse level's few texels a face shorten it more than its lobe does: the last two levels
        // read levels of 4 and 2 texels a side (each of the last level's texels spans a whole face).
        let tolerance = match CAPTURE_MIPS - level {
            1 => 0.15,
            2 => 0.06,
            _ => 0.015,
        };
        assert!(
            (mean / keeps - 1.0).abs() < tolerance,
            "level {level}: the mean length {mean} is not the lobes' {keeps}"
        );
    }
}
