//! The rope plugin in the level demo (the `rope` feature): a red clothesline across the front of the plaza's stage, from the top of
//! one front pillar to the top of the other. A map starts with it hung, and leaving the map takes it down.
//!
//! The rope is the plugin's, made from a datablock and two points by the plugin's console functions; the demo's own scripts are
//! not touched, so a build without the feature plays exactly as before.

use crate::client::LevelDemo;

/// The map the line hangs in.
pub const MAP: &str = "plaza";

/// The line's ends: the tops of the stage's two front pillars, a little below them.
pub const START: [f32; 3] = [-8.0, -2.0, 3.6];
pub const END: [f32; 3] = [8.0, -2.0, 3.6];

/// How the line is made: 20 pieces, a rope 17 units long between ends 16 apart (it hangs about 2 units below them), red.
const DATABLOCK: &str = r#"datablock RopeData(PlazaLine)
{
   segments = 20;
   length = 17;
   thickness = 0.09;
   color = "205 70 55 255";
};"#;

/// Hangs the line if `map` is the plaza. The datablock is declared the first time.
pub fn hang(demo: &mut LevelDemo, map: &str) -> Result<(), String> {
    if map != MAP {
        return Ok(());
    }
    let vm = demo.client.vm_mut();
    if vm.objects().find(b"PlazaLine").is_none() {
        vm.eval(DATABLOCK.as_bytes())
            .map_err(|e| format!("the rope's datablock: {e:?}"))?;
    }
    let make = format!(
        "ropeFloor(0); $Plaza::Line = ropeCreate(\"{} {} {}\", \"{} {} {}\", PlazaLine);",
        START[0], START[1], START[2], END[0], END[1], END[2]
    );
    vm.eval(make.as_bytes())
        .map(drop)
        .map_err(|e| format!("the rope: {e:?}"))
}

/// Takes every rope down.
pub fn take_down(demo: &mut LevelDemo) {
    if let Err(e) = demo.client.vm_mut().eval(b"ropeClear();") {
        eprintln!("example-level-demo: ropeClear: {e:?}");
    }
}

/// The number of the line `hang` made, as the client's script engine holds it (0: none).
pub fn line(demo: &LevelDemo) -> i32 {
    demo.client
        .vm()
        .global("$Plaza::Line")
        .and_then(|v| String::from_utf8_lossy(&v).parse().ok())
        .unwrap_or(0)
}

/// The other two looks, for the pictures of them (`--rope-style-shots`; the plaza hangs neither): a cable drawn as a ribbon, and a
/// laser drawn as a line, glowing. Each is a datablock of its own, named and written here.
const STYLE_DATABLOCKS: [(&str, &str); 11] = [
    (
        "PlazaRibbon",
        r#"datablock RopeData(PlazaRibbon)
{
   style = "ribbon";
   segments = 8;
   thickness = 0.2;
   color = "245 190 50 255";
};"#,
    ),
    (
        "PlazaLaser",
        r#"datablock RopeData(PlazaLaser)
{
   style = "line";
   thickness = 0.05;
   color = "255 40 40 230";
   emissive = true;
};"#,
    ),
    (
        "ShadowRibbon",
        r#"datablock RopeData(ShadowRibbon)
{
   style = "ribbon";
   segments = 8;
   thickness = 0.5;
   color = "245 190 50 255";
   castShadow = 1;
};"#,
    ),
    (
        "ShadowLine",
        r#"datablock RopeData(ShadowLine)
{
   style = "line";
   thickness = 0.35;
   color = "255 40 40 230";
   emissive = true;
   castShadow = 1;
};"#,
    ),
    (
        "PlazaTube",
        r#"datablock RopeData(PlazaTube)
{
   segments = 10;
   thickness = 0.3;
   color = "210 90 60 255";
};"#,
    ),
    (
        "PlazaTubeLit",
        r#"datablock RopeData(PlazaTubeLit)
{
   segments = 10;
   thickness = 0.3;
   color = "210 90 60 255";
   receiveShadow = 0;
};"#,
    ),
    (
        "PlazaRibbonLit",
        r#"datablock RopeData(PlazaRibbonLit)
{
   style = "ribbon";
   segments = 8;
   thickness = 0.2;
   color = "245 190 50 255";
   receiveShadow = 0;
};"#,
    ),
    (
        "PlazaBraid",
        r#"datablock RopeData(PlazaBraid)
{
   segments = 14;
   thickness = 0.22;
   color = "255 255 255 255";
   texture = "rope/braid.png";
   textureLength = 0.35;
};"#,
    ),
    (
        "PlazaChain",
        r#"datablock RopeData(PlazaChain)
{
   style = "ribbon";
   segments = 10;
   thickness = 0.5;
   color = "255 255 255 255";
   texture = "rope/chain.png";
   textureLength = 1.0;
   castShadow = 1;
};"#,
    ),
    (
        "PlazaBeam",
        r#"datablock RopeData(PlazaBeam)
{
   style = "line";
   thickness = 0.35;
   color = "255 255 255 255";
   emissive = true;
   texture = "rope/beam.png";
   textureLength = 1.6;
   textureScroll = 1.5;
};"#,
    ),
    (
        "PlazaTwin",
        r#"datablock RopeData(PlazaTwin)
{
   style = "ribbon";
   segments = 10;
   thickness = 0.4;
   color = "255 255 255 255";
   texture = "rope/stripes.png";
   textureLength = 0.4;
};"#,
    ),
];

/// Declares the ribbon cable `PlazaRibbon` and the laser `PlazaLaser` the first time. A client's datablock ids do not skip the ones
/// the game's own datablocks hold, so a declaration that meets a taken id is refused with a warning and written again (the counter
/// has moved on).
pub fn declare_styles(demo: &mut LevelDemo) -> Result<(), String> {
    let vm = demo.client.vm_mut();
    for (name, source) in STYLE_DATABLOCKS {
        for _ in 0..16 {
            if vm.objects().find(name.as_bytes()).is_some() {
                break;
            }
            vm.eval(source.as_bytes())
                .map_err(|e| format!("the datablock {name}: {e:?}"))?;
        }
        if vm.objects().find(name.as_bytes()).is_none() {
            return Err(format!("the datablock {name} was not made"));
        }
    }
    Ok(())
}

/// Makes a rope of `datablock` between two points.
pub fn make(
    demo: &mut LevelDemo,
    datablock: &str,
    start: [f32; 3],
    end: [f32; 3],
) -> Result<(), String> {
    let script = format!(
        "ropeCreate(\"{} {} {}\", \"{} {} {}\", {datablock});",
        start[0], start[1], start[2], end[0], end[1], end[2]
    );
    demo.client
        .vm_mut()
        .eval(script.as_bytes())
        .map(drop)
        .map_err(|e| format!("the rope: {e:?}"))
}

/// Where the frame draws a mount of a ghost's shape for the ropes that follow it. The figure's parts are drawn in world space (the
/// shape has no meshes of its own to name), so its `hand` is a place on its right side at the height of its hip, carried by the
/// figure's drawn transform; any other node is the figure's origin.
pub struct Mounts;

impl torque_plugin_rope::FrameAnchors for Mounts {
    fn place(
        &self,
        shape: &torque_render_scene::hd::DynamicObject,
        node: &str,
    ) -> Option<[f32; 3]> {
        if node == "hand" {
            return Some(shape.transform.mul_p([0.5, 0.15, 0.95]));
        }
        torque_plugin_rope::DrawnShapes.place(shape, node)
    }
}

/// A texture made in code: `pixel(x, y)` for `width` x `height` texels, as a PNG file under `rope/`.
fn texture(
    files: &mut crate::files::GameFiles,
    name: &str,
    (width, height): (u32, u32),
    pixel: impl Fn(f32, f32) -> [u8; 4],
) {
    let mut texels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            texels.extend(pixel(
                (x as f32 + 0.5) / width as f32,
                (y as f32 + 0.5) / height as f32,
            ));
        }
    }
    if let Some(png) = torque_plugin_rope::encode_png(width, height, &texels) {
        files.insert(&format!("rope/{name}.png"), &png);
    }
}

/// Adds the textures the pictures of the rope styles use, made here in code (nothing is read from art files): a braid (`rope/braid.png`),
/// chain links with holes between them (`rope/chain.png`), a glowing beam (`rope/beam.png`) and candy stripes (`rope/stripes.png`).
pub fn add_textures(files: &mut crate::files::GameFiles) {
    let mix = |a: [f32; 3], b: [f32; 3], t: f32| [0, 1, 2].map(|k| a[k] + (b[k] - a[k]) * t);
    let byte = |c: [f32; 3], a: f32| {
        [c[0], c[1], c[2], a].map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8)
    };
    // Strands that wind around the rope: two bands of a diagonal, lit from one side of the round.
    texture(files, "braid", (32, 16), |u, v| {
        let strand = ((u * 4.0 + v * 2.0).fract() < 0.5) as u8 as f32;
        let round = 0.65 + 0.35 * (std::f32::consts::PI * v).sin();
        let edge = ((u * 4.0 + v * 2.0).fract() - 0.5)
            .abs()
            .min((u * 4.0 + v * 2.0).fract())
            * 6.0;
        let base = mix([0.55, 0.38, 0.20], [0.86, 0.68, 0.42], strand);
        let dark = edge.clamp(0.0, 1.0);
        byte(
            [0, 1, 2].map(|k| base[k] * round * (0.55 + 0.45 * dark)),
            1.0,
        )
    });
    // Oval links along the strip, each a ring: the metal where the ring is, a hole in it and between the links.
    texture(files, "chain", (32, 16), |u, v| {
        let (x, y) = ((u * 2.0).fract() - 0.5, v - 0.5);
        let d = ((x / 0.46).powi(2) + (y / 0.40).powi(2)).sqrt();
        let ring = ((d - 0.78).abs() < 0.17) as u8 as f32;
        let shine = 0.7 + 0.3 * (std::f32::consts::TAU * (u * 2.0 + v)).sin();
        byte(mix([0.45, 0.48, 0.52], [0.85, 0.88, 0.92], shine), ring)
    });
    // A beam: a hot core fading to red at its edges, rippling along its length.
    texture(files, "beam", (64, 16), |u, v| {
        let across = (-(((v - 0.5) / 0.22).powi(2))).exp();
        let ripple = 0.7 + 0.3 * (std::f32::consts::TAU * u * 4.0).sin();
        byte(
            mix([1.0, 0.18, 0.12], [1.0, 0.92, 0.80], across),
            across * ripple,
        )
    });
    texture(files, "stripes", (16, 16), |u, v| {
        let on = ((u * 4.0 + v * 4.0).fract() < 0.5) as u8 as f32;
        byte(mix([0.25, 0.25, 0.3], [1.0, 1.0, 1.0], on), 1.0)
    });
}
