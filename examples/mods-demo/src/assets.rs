//! The content of the mods, made from code: no art is kept in the repository, the way the other examples' asset kit is made.
//!
//! The mods' folders (`game/mods`) hold only what is text (`mod.toml` and the scripts). The shapes, pictures and sound the
//! `orbs` mod ships are written here and put in the game's files at `mods/<id>/...` when the example starts, so the server has
//! them where it has the mod and a client that lacks the mod is sent them.

use std::f32::consts::TAU;
use std::io;

use torque::shapes::write::write_dts;
use torque_example_assets::builder::{FULL_UV, MeshData, ShapeBuilder, material_flags, turn};
use torque_example_assets::textures::png_rgba;

/// A file of a mod: its path inside the mod and its bytes.
pub type ModFile = (&'static str, Vec<u8>);

/// A shape of two boxes across each other (the second turned a quarter of a turn about z), self-lit, whose material is `name`
/// (the picture `<name>.png` beside the shape): a jewel for an orb, a squat block for a runner.
fn shape(name: &str, half: [f32; 3], twin: bool) -> Vec<u8> {
    let mut b = ShapeBuilder::new();
    let node = b.node("root", None, [0.0; 3]);
    let drawn = b.detail("detail1", 1.0);
    let flags = material_flags::S_WRAP | material_flags::T_WRAP | material_flags::NEVER_ENV_MAP;
    let material = b.material(
        name,
        if twin {
            flags | material_flags::SELF_ILLUMINATING
        } else {
            flags
        },
    );
    let mut mesh = MeshData::new(Some(material));
    mesh.add_box(half.map(|h| -h), half, |_| FULL_UV);
    b.object("body", node, drawn, mesh);
    if twin {
        let second = b.node_turned("twin", Some(node), [0.0; 3], turn([0.0, 0.0, 1.0], 45.0));
        let mut mesh = MeshData::new(Some(material));
        mesh.add_box(half.map(|h| -h), half, |_| FULL_UV);
        b.object("twin", second, drawn, mesh);
    }
    let collision = b.detail("Collision-1", -1.0);
    let sight = b.detail("LOS-9", -1.0);
    b.object(
        "Col-1",
        node,
        collision,
        MeshData::hull_box(half.map(|h| -h), half),
    );
    b.object(
        "LOS-9",
        node,
        sight,
        MeshData::hull_box(half.map(|h| -h), half),
    );
    write_dts(&b.build())
}

/// An orb's picture: a warm glow, bright at the middle of each face.
fn orb_png() -> io::Result<Vec<u8>> {
    png_rgba(32, 32, |x, y| {
        let (dx, dy) = ((x as f32 - 15.5) / 15.5, (y as f32 - 15.5) / 15.5);
        let t = (1.0 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
        let lift = |low: f32, high: f32| (low + (high - low) * t).round() as u8;
        [
            lift(230.0, 255.0),
            lift(150.0, 245.0),
            lift(30.0, 160.0),
            255,
        ]
    })
}

/// A runner's picture: its team's color with a lighter band across the top rows.
fn runner_png(color: [u8; 3]) -> io::Result<Vec<u8>> {
    png_rgba(16, 16, |x, y| {
        let band = if y < 4 { 60 } else { 0 };
        let chip = if (x / 4 + y / 4) % 2 == 0 { 0 } else { 18 };
        let c = |v: u8| v.saturating_add(band).saturating_sub(chip);
        [c(color[0]), c(color[1]), c(color[2]), 255]
    })
}

/// The beam ropes' picture: bright pulses along a dimmer line, repeated along the rope (the rope's color tints it).
fn beam_png() -> io::Result<Vec<u8>> {
    png_rgba(32, 4, |x, _| {
        let pulse = ((x as f32 / 32.0 * TAU).cos() * 0.5 + 0.5).powi(2);
        let v = (110.0 + 145.0 * pulse).round() as u8;
        [v, v, v, 255]
    })
}

/// A short rising ping as a 16-bit mono WAV (22,050 Hz, a tenth of a second).
fn ping_wav() -> Vec<u8> {
    const RATE: u32 = 22_050;
    let samples: Vec<i16> = (0..RATE / 10)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let tone = (TAU * (660.0 + 2_400.0 * t) * t).sin();
            (tone * (1.0 - t * 10.0).max(0.0) * 12_000.0) as i16
        })
        .collect();
    let data = samples.len() as u32 * 2;
    let mut wav = Vec::with_capacity(44 + data as usize);
    wav.extend(b"RIFF");
    wav.extend((36 + data).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(RATE.to_le_bytes());
    wav.extend((RATE * 2).to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend(data.to_le_bytes());
    for s in samples {
        wav.extend(s.to_le_bytes());
    }
    wav
}

/// The `orbs` mod's content: two runners, the orb, the beam's picture and the pickup sound.
pub fn orbs() -> io::Result<Vec<ModFile>> {
    Ok(vec![
        ("shapes/orb.dts", shape("orb", [0.28, 0.28, 0.28], true)),
        ("shapes/orb.png", orb_png()?),
        (
            "shapes/runner_a.dts",
            shape("runner_a", [0.35, 0.35, 0.5], false),
        ),
        ("shapes/runner_a.png", runner_png([70, 130, 230])?),
        (
            "shapes/runner_b.dts",
            shape("runner_b", [0.35, 0.35, 0.5], false),
        ),
        ("shapes/runner_b.png", runner_png([225, 90, 90])?),
        ("art/beam.png", beam_png()?),
        ("sounds/pickup.wav", ping_wav()),
    ])
}

/// The `confetti` mod's content (a mod the demo's prefs turn off).
pub fn confetti() -> io::Result<Vec<ModFile>> {
    Ok(vec![(
        "art/confetti.png",
        png_rgba(8, 8, |x, y| [255, (x * 32) as u8, (y * 32) as u8, 255])?,
    )])
}
