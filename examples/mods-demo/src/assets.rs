//! The content of the mods, made from code: no art is kept in the repository, the way the other examples' asset kit is made.
//!
//! The mods' folders (`game/mods`) hold only what is text (`mod.toml` and the scripts). The shapes, pictures and sound the
//! `soccer` mod ships are written here and put in the game's files at `mods/<id>/...` when the example starts, so the server has
//! them where it has the mod and a client that lacks the mod is sent them.

use std::f32::consts::{PI, TAU};
use std::io;

use torque::shapes::write::write_dts;
use torque_example_assets::builder::{
    FULL_UV, Face, MeshData, ShapeBuilder, UvRect, Vec3, material_flags,
};
use torque_example_assets::textures::png_rgba;

/// A file of a mod: its path inside the mod and its bytes.
pub type ModFile = (String, Vec<u8>);

// The pitch's numbers. The soccer mod's rules (`lib/rules.lua`) and datablocks (`server.cs`) repeat them, and the bots
// (`crate::bot`) steer by them: `the_mods_scripts_agree_with_the_pitchs_numbers` checks the scripts against these.

/// The pitch inside its walls reaches this far from the middle across (x).
pub const HALF_WIDTH: f32 = 8.0;
/// And this far along (y), to the goal lines.
pub const HALF_LENGTH: f32 = 12.0;
/// A goal's mouth reaches this far from the middle, between its posts.
pub const GOAL_HALF: f32 = 3.5;
/// How far a goal's net reaches back from the goal line.
pub const GOAL_DEPTH: f32 = 1.5;
/// The height of a goal's crossbar's underside.
pub const GOAL_HEIGHT: f32 = 2.1;
/// The posts' and the crossbar's thickness.
pub const POST: f32 = 0.2;
/// The nets' thickness.
pub const NET: f32 = 0.06;
/// The ball's radius as its shape is drawn (the `BallData` datablock collides as a ball of the same radius).
pub const BALL_RADIUS: f32 = 0.45;
/// The floor's size (the `PitchData` block is scaled to it): the pitch inside the walls and a border round it.
pub const FLOOR: [f32; 2] = [22.0, 32.0];

/// The side of a cell of a net's mesh.
const NET_CELL: f32 = 0.2;
/// The material flag of a see-through texture (`TSMaterialList::Translucent`): blended by its alpha.
const TRANSLUCENT: u32 = 0x4;

const BLUE: [u8; 3] = [52, 110, 220];
const RED: [u8; 3] = [214, 64, 58];
const WHITE: [u8; 3] = [246, 246, 242];

const FLAGS: u32 = material_flags::S_WRAP | material_flags::T_WRAP | material_flags::NEVER_ENV_MAP;

/// A shape builder with its root node and its drawn detail.
fn start() -> (ShapeBuilder, usize, usize) {
    let mut b = ShapeBuilder::new();
    let node = b.node("root", None, [0.0; 3]);
    let drawn = b.detail("detail1", 1.0);
    (b, node, drawn)
}

/// The collision and line-of-sight details of `b`: the box `min`..`max`.
fn finish(mut b: ShapeBuilder, node: usize, min: Vec3, max: Vec3) -> Vec<u8> {
    let collision = b.detail("Collision-1", -1.0);
    let sight = b.detail("LOS-9", -1.0);
    b.object("Col-1", node, collision, MeshData::hull_box(min, max));
    b.object("LOS-9", node, sight, MeshData::hull_box(min, max));
    write_dts(&b.build())
}

/// A box of half-extents `half` round the origin drawn with the picture `<name>.png`, the whole picture on each face.
fn solid(name: &str, half: Vec3) -> Vec<u8> {
    let (mut b, node, drawn) = start();
    let material = b.material(name, FLAGS);
    let mut mesh = MeshData::new(Some(material));
    let min = half.map(|h| -h);
    mesh.add_box(min, half, |_| FULL_UV);
    b.object("body", node, drawn, mesh);
    finish(b, node, min, half)
}

/// A unit cube round the origin drawn with `<name>.png`: the floor and the walls are this cube scaled.
fn block(name: &str) -> Vec<u8> {
    solid(name, [0.5; 3])
}

/// A thin panel of net of half-extents `half` round the origin: the see-through picture `<name>.png` repeated once each
/// [`NET_CELL`] across every face, so the mesh is the same size on a panel of any size.
fn net_panel(name: &str, half: Vec3) -> Vec<u8> {
    let (mut b, node, drawn) = start();
    let material = b.material(name, FLAGS | TRANSLUCENT);
    let mut mesh = MeshData::new(Some(material));
    let min = half.map(|h| -h);
    let cells = |u: usize, v: usize| [0.0, 0.0, 2.0 * half[u] / NET_CELL, 2.0 * half[v] / NET_CELL];
    mesh.add_box(min, half, |face| match face {
        Face::XNeg | Face::XPos => cells(1, 2),
        Face::YNeg | Face::YPos => cells(0, 2),
        Face::ZNeg | Face::ZPos => cells(0, 1),
    });
    b.object("body", node, drawn, mesh);
    finish(b, node, min, half)
}

/// A piece of a goal (the rules place them; each team's goal has its own shapes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GoalPiece {
    Post,
    Bar,
    NetBack,
    NetSide,
    NetRoof,
}

impl GoalPiece {
    const ALL: [GoalPiece; 5] = [
        GoalPiece::Post,
        GoalPiece::Bar,
        GoalPiece::NetBack,
        GoalPiece::NetSide,
        GoalPiece::NetRoof,
    ];

    /// Its shape's name (`shapes/<name>_<team>.dts`).
    fn name(self) -> &'static str {
        match self {
            GoalPiece::Post => "post",
            GoalPiece::Bar => "bar",
            GoalPiece::NetBack => "net_back",
            GoalPiece::NetSide => "net_side",
            GoalPiece::NetRoof => "net_roof",
        }
    }

    /// Its half-extents.
    fn half(self) -> Vec3 {
        let depth = (GOAL_DEPTH - POST) / 2.0;
        match self {
            GoalPiece::Post => [POST / 2.0, POST / 2.0, GOAL_HEIGHT / 2.0],
            GoalPiece::Bar => [GOAL_HALF + POST, POST / 2.0, POST / 2.0],
            GoalPiece::NetBack => [GOAL_HALF + POST, NET / 2.0, GOAL_HEIGHT / 2.0],
            GoalPiece::NetSide => [NET / 2.0, depth, GOAL_HEIGHT / 2.0],
            GoalPiece::NetRoof => [GOAL_HALF + POST / 2.0, depth, NET / 2.0],
        }
    }

    /// Its shape for `team`: white posts with the team's band, a white crossbar, and the team's tinted net.
    fn shape(self, team: &str) -> Vec<u8> {
        match self {
            GoalPiece::Post => solid(&format!("post_{team}"), self.half()),
            GoalPiece::Bar => solid("bar", self.half()),
            GoalPiece::NetBack | GoalPiece::NetSide | GoalPiece::NetRoof => {
                net_panel(&format!("net_{team}"), self.half())
            }
        }
    }
}

/// A player: a body and a head, its feet at the origin, drawn with `<name>.png` (the left half of the picture for every face
/// but the front, +y, which takes the right half).
fn figure(name: &str) -> Vec<u8> {
    let (mut b, node, drawn) = start();
    let material = b.material(name, FLAGS);
    let uv = |face: Face| -> UvRect {
        if face == Face::YPos {
            [0.5, 0.0, 1.0, 1.0]
        } else {
            [0.0, 0.0, 0.5, 1.0]
        }
    };
    let mut mesh = MeshData::new(Some(material));
    mesh.add_box([-0.45, -0.45, 0.0], [0.45, 0.45, 1.3], uv);
    mesh.add_box([-0.3, -0.3, 1.32], [0.3, 0.3, 1.8], |_| {
        [0.0, 0.0, 0.5, 0.25]
    });
    b.object("body", node, drawn, mesh);
    finish(b, node, [-0.45, -0.45, 0.0], [0.45, 0.45, 1.8])
}

/// The ball: a sphere of [`BALL_RADIUS`] round the origin, the picture wrapped once round it.
fn ball() -> Vec<u8> {
    const SEGMENTS: u16 = 20;
    const RINGS: u16 = 12;
    let (mut b, node, drawn) = start();
    let material = b.material("ball", FLAGS);
    let mut mesh = MeshData::new(Some(material));
    for ring in 0..=RINGS {
        let v = f32::from(ring) / f32::from(RINGS);
        let (sin_t, cos_t) = (v * PI).sin_cos();
        for segment in 0..=SEGMENTS {
            let u = f32::from(segment) / f32::from(SEGMENTS);
            let (sin_p, cos_p) = (u * TAU).sin_cos();
            let n = [sin_t * cos_p, sin_t * sin_p, cos_t];
            mesh.positions.push(n.map(|c| c * BALL_RADIUS));
            mesh.normals.push(n);
            mesh.tverts.push([u, v]);
        }
    }
    // Clockwise seen from outside, as the builder's own boxes are wound.
    let row = SEGMENTS + 1;
    for ring in 0..RINGS {
        for segment in 0..SEGMENTS {
            let a = ring * row + segment;
            let (b1, c, d) = (a + 1, a + row + 1, a + row);
            mesh.indices.extend([a, b1, c, a, c, d]);
        }
    }
    b.object("body", node, drawn, mesh);
    let r = BALL_RADIUS;
    finish(b, node, [-r; 3], [r; 3])
}

/// Where the pixel `(x, y)` of a `w` by `h` picture of the floor's top lies on the floor (the picture's top row is the far
/// end, +y).
fn floor_point(x: u32, y: u32, w: u32, h: u32) -> [f32; 2] {
    let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
    [(u - 0.5) * FLOOR[0], (0.5 - v) * FLOOR[1]]
}

/// The floor's picture: mown stripes inside the walls with the pitch's lines, darker grass round it.
fn pitch_png() -> io::Result<Vec<u8>> {
    const LINE: f32 = 0.09;
    let near = |d: f32| d.abs() < LINE;
    png_rgba(256, 512, |x, y| {
        let [px, py] = floor_point(x, y, 256, 512);
        let (ax, ay) = (px.abs(), py.abs());
        if ax > HALF_WIDTH - 0.25 || ay > HALF_LENGTH - 0.25 {
            return [58, 104, 60, 255];
        }
        let circle = (px * px + py * py).sqrt();
        let (area_x, area_y) = (GOAL_HALF + 0.5, HALF_LENGTH - 3.0);
        let area = ax < area_x && ay > area_y && (near(ax - area_x) || near(ay - area_y));
        let lines = near(ax - (HALF_WIDTH - 0.5))
            || near(ay - (HALF_LENGTH - 0.5))
            || near(py)
            || near(circle - 2.6)
            || circle < 0.2
            || area;
        if lines && ax < HALF_WIDTH - 0.4 && ay < HALF_LENGTH - 0.4 {
            return [236, 240, 232, 255];
        }
        if (py + HALF_LENGTH).rem_euclid(4.0) < 2.0 {
            [88, 150, 78, 255]
        } else {
            [78, 136, 70, 255]
        }
    })
}

/// The walls' picture: a light board with a darker rail along its top.
fn wall_png() -> io::Result<Vec<u8>> {
    png_rgba(16, 16, |_, y| {
        if y < 3 {
            [70, 76, 90, 255]
        } else {
            let v = 222 - (y as u8) * 2;
            [v, v, v.saturating_add(6), 255]
        }
    })
}

/// A net's picture, one cell of its mesh: light strands tinted with its team's color, see-through between them.
fn net_png(color: [u8; 3]) -> io::Result<Vec<u8>> {
    let tint = color.map(|c| ((u16::from(c) + 3 * 235) / 4) as u8);
    png_rgba(16, 16, |x, y| {
        let alpha = if x < 2 || y < 2 { 225 } else { 0 };
        [tint[0], tint[1], tint[2], alpha]
    })
}

/// A post's picture: white, with a band of `color` a little below the middle.
fn post_png(color: [u8; 3]) -> io::Result<Vec<u8>> {
    png_rgba(8, 32, |_, y| {
        if (18..24).contains(&y) {
            [color[0], color[1], color[2], 255]
        } else {
            [WHITE[0], WHITE[1], WHITE[2], 255]
        }
    })
}

/// A player's picture: its team's color on the left half (a lighter band at the shoulders), the front's on the right (a white
/// stripe down the middle).
fn figure_png(color: [u8; 3]) -> io::Result<Vec<u8>> {
    png_rgba(32, 16, |x, y| {
        let lift = |v: u8, by: u8| v.saturating_add(by);
        if x < 16 {
            let by = if (2..5).contains(&y) { 40 } else { 0 };
            [
                lift(color[0], by),
                lift(color[1], by),
                lift(color[2], by),
                255,
            ]
        } else if (22..26).contains(&x) {
            [245, 245, 245, 255]
        } else {
            [color[0], color[1], color[2], 255]
        }
    })
}

/// The golden ratio.
const PHI: f32 = std::f32::consts::GOLDEN_RATIO;

fn unit(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    v.map(|c| c / len)
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The faces of a ball made as a truncated icosahedron: the directions of the 12 pentagons' centers (the icosahedron's corners)
/// and of the 20 hexagons' (the dodecahedron's), each with the distance of its face's plane from the middle, the corners' distance being 1. A ray from the middle in direction `n` leaves through the face whose `dot(n, center) / distance` is the largest.
fn ball_faces() -> (Vec<([f32; 3], f32)>, usize) {
    let mut pentagons = Vec::new();
    for a in [-1.0, 1.0] {
        for b in [-PHI, PHI] {
            pentagons.extend([[0.0, a, b], [a, b, 0.0], [b, 0.0, a]]);
        }
    }
    let mut hexagons = Vec::new();
    for a in [-1.0, 1.0] {
        for b in [-1.0, 1.0] {
            for c in [-1.0, 1.0] {
                hexagons.push([a, b, c]);
            }
            let (inv, phi) = (b / PHI, PHI * a);
            hexagons.extend([[0.0, phi, inv], [phi, inv, 0.0], [inv, 0.0, phi]]);
        }
    }
    // The truncated icosahedron's corners (edge 2): the even permutations of these.
    let mut corners = Vec::new();
    for a in [-1.0, 1.0] {
        for b in [-1.0, 1.0] {
            for c in [-1.0, 1.0] {
                for [x, y, z] in [
                    [0.0, a, 3.0 * PHI * b],
                    [a, (2.0 + PHI) * b, 2.0 * PHI * c],
                    [PHI * a, 2.0 * b, (2.0 * PHI + 1.0) * c],
                ] {
                    corners.extend([unit([x, y, z]), unit([y, z, x]), unit([z, x, y])]);
                }
            }
        }
    }
    let faces = pentagons.iter().chain(&hexagons).map(|&c| {
        let c = unit(c);
        let plane = corners.iter().map(|&v| dot(v, c)).fold(f32::MIN, f32::max);
        (c, plane)
    });
    (faces.collect(), pentagons.len())
}

/// The ball's picture, wrapped once round the sphere (columns are longitude, rows run pole to pole): the faces of a truncated
/// icosahedron, black pentagons among white hexagons, with a thin dark seam along every edge. Each texel takes its color from the
/// direction it covers on the sphere, so the pattern is even all over and does not pinch at the poles.
fn ball_png() -> io::Result<Vec<u8>> {
    const W: u32 = 1024;
    const H: u32 = 512;
    // The seam's half width and the soft edge of every border, in the faces' `dot / distance` units.
    const SEAM: f32 = 0.004;
    const SOFT: f32 = 0.002;
    let (faces, pentagons) = ball_faces();
    let mix = |a: [f32; 3], b: [f32; 3], t: f32| -> [f32; 3] {
        std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
    };
    let smooth = |edge: f32, x: f32| ((x - edge) / SOFT).clamp(0.0, 1.0);
    png_rgba(W, H, |x, y| {
        let (u, v) = ((x as f32 + 0.5) / W as f32, (y as f32 + 0.5) / H as f32);
        let (sin_p, cos_p) = (u * TAU).sin_cos();
        let (sin_t, cos_t) = (v * PI).sin_cos();
        let n = [sin_t * cos_p, sin_t * sin_p, cos_t];
        let (mut best, mut second, mut face) = (f32::MIN, f32::MIN, 0);
        for (i, &(center, plane)) in faces.iter().enumerate() {
            let score = dot(n, center) / plane;
            if score > best {
                (second, best, face) = (best, score, i);
            } else if score > second {
                second = score;
            }
        }
        let base: [f32; 3] = if face < pentagons {
            [26.0, 26.0, 30.0]
        } else {
            [248.0, 248.0, 243.0]
        };
        let seam = [96.0, 98.0, 104.0];
        // 0 on an edge, 1 from the seam's width on.
        let away = smooth(SEAM, best - second);
        let c = mix(seam, base, away);
        [c[0] as u8, c[1] as u8, c[2] as u8, 255]
    })
}

/// A short thump as a 16-bit mono WAV (22,050 Hz, a tenth of a second): the kick.
fn kick_wav() -> Vec<u8> {
    const RATE: u32 = 22_050;
    let samples: Vec<i16> = (0..RATE / 10)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let tone = (TAU * (180.0 - 900.0 * t) * t).sin();
            (tone * (1.0 - t * 10.0).max(0.0) * 14_000.0) as i16
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

/// The `soccer` mod's content: the pitch's blocks, the goals (each team's posts and net carry its color), the two teams'
/// players, the ball and the kick's sound.
pub fn soccer() -> io::Result<Vec<ModFile>> {
    let mut files = vec![
        ("shapes/pitch.dts".to_string(), block("pitch")),
        ("shapes/pitch.png".to_string(), pitch_png()?),
        ("shapes/wall.dts".to_string(), block("wall")),
        ("shapes/wall.png".to_string(), wall_png()?),
        ("shapes/bar.png".to_string(), post_png(WHITE)?),
    ];
    for (team, color) in [("blue", BLUE), ("red", RED)] {
        for piece in GoalPiece::ALL {
            let path = format!("shapes/{}_{team}.dts", piece.name());
            files.push((path, piece.shape(team)));
        }
        files.push((format!("shapes/post_{team}.png"), post_png(color)?));
        files.push((format!("shapes/net_{team}.png"), net_png(color)?));
        files.push((
            format!("shapes/player_{team}.dts"),
            figure(&format!("player_{team}")),
        ));
        files.push((format!("shapes/player_{team}.png"), figure_png(color)?));
    }
    files.push(("shapes/ball.dts".to_string(), ball()));
    files.push(("shapes/ball.png".to_string(), ball_png()?));
    files.push(("sounds/kick.wav".to_string(), kick_wav()));
    Ok(files)
}

/// The `confetti` mod's content (a mod the demo's prefs turn off).
pub fn confetti() -> io::Result<Vec<ModFile>> {
    Ok(vec![(
        "art/confetti.png".to_string(),
        png_rgba(8, 8, |x, y| [255, (x * 32) as u8, (y * 32) as u8, 255])?,
    )])
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULES: &str = include_str!("../game/mods/soccer/lib/rules.lua");
    const SOCCER: &str = include_str!("../game/mods/soccer/server.cs");
    const BIG_BALL: &str = include_str!("../game/mods/big-ball/server.cs");
    const CLIENT: &str = include_str!("../game/client.cs");

    /// The number after `<name> =` on the first line of `text` that starts with it (Lua's `NAME = 1,`, a datablock's
    /// `name = 1;` or `name = "1 1 1";`, whose first word it takes).
    fn number(text: &str, name: &str) -> f32 {
        let prefix = format!("{name} = ");
        let line = text
            .lines()
            .map(str::trim)
            .find_map(|l| l.strip_prefix(&prefix))
            .unwrap_or_else(|| panic!("no {name}"));
        let word = line
            .trim_start_matches('"')
            .split([',', ';', ' ', '"'])
            .next()
            .unwrap_or_default();
        word.parse().unwrap_or_else(|_| panic!("{name}: {line}"))
    }

    #[test]
    fn the_mods_scripts_agree_with_the_pitchs_numbers() {
        for (name, value) in [
            ("HALF_WIDTH", HALF_WIDTH),
            ("HALF_LENGTH", HALF_LENGTH),
            ("GOAL_HALF", GOAL_HALF),
            ("GOAL_DEPTH", GOAL_DEPTH),
            ("GOAL_HEIGHT", GOAL_HEIGHT),
            ("POST", POST),
            ("FLOOR_WIDTH", FLOOR[0]),
            ("FLOOR_LENGTH", FLOOR[1]),
        ] {
            assert_eq!(number(RULES, name), value, "lib/rules.lua's {name}");
        }
        for (name, value) in [
            ("$Pitch::HalfWidth", HALF_WIDTH),
            ("$Pitch::HalfLength", HALF_LENGTH),
            ("$Pitch::GoalDepth", GOAL_DEPTH),
            ("$Pitch::GoalHeight", GOAL_HEIGHT),
        ] {
            assert_eq!(number(CLIENT, name), value, "client.cs's {name}");
        }
        assert_eq!(number(SOCCER, "radius"), BALL_RADIUS, "BallData's radius");
        // The big ball is the soccer mod's shape drawn at its radius.
        let (radius, scale) = (number(BIG_BALL, "radius"), number(BIG_BALL, "drawScale"));
        assert!(
            (scale * BALL_RADIUS - radius).abs() < 1e-3,
            "{scale} x {BALL_RADIUS} is not {radius}"
        );
    }
}
