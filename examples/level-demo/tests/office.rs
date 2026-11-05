//! The office level (`game/levels/office.bbmodel`): built into one interior that passes the builder's verification, and
//! walked through in the level demo's figure: in at the front door, along the corridor and an aisle of desks, through each
//! room's door, stopped by the furniture.

use torque::client::input::Key;
use torque_example_level_demo::client::LevelDemo;
use torque_example_level_demo::files::GameFiles;
use torque_import::{Options, bbmodel};
use torque_interior_builder::mesh::{MeshInput, MeshOptions};
use torque_interior_builder::verify::Counts;
use torque_interior_builder::verify::mesh::run_mesh;

/// The office's corner in the map: the model's floor runs from (0, 0) to (36, 24) in the generator's numbers.
const CORNER: [f32; 2] = [-18.0, -12.0];

fn enter(game: &mut LevelDemo, map: usize) -> bool {
    game.start_map(map).expect("the map starts");
    for _ in 0..600 {
        game.step(32);
        if game.in_world() && game.ghost_count() > 1 {
            return true;
        }
    }
    false
}

fn server_position(game: &LevelDemo) -> [f32; 3] {
    game.server
        .as_ref()
        .and_then(|s| s.figure_position())
        .expect("the server's figure")
}

fn hold(game: &mut LevelDemo, key: Key, ms: u32) {
    game.key(key, true);
    for _ in 0..ms / 32 {
        game.step(32);
    }
    game.key(key, false);
}

#[test]
fn the_office_builds_into_one_interior_that_passes_verification() {
    let files = GameFiles::bundled().expect("the game folder");
    let bytes = files.file("levels/office.bbmodel").expect("the office");
    let options = Options {
        box_hull: false,
        ..Options::default()
    };
    let model = bbmodel::read(bytes, &options, &mut |_| true).unwrap();
    let input = MeshInput::from_model(&model).unwrap();
    let r = run_mesh(&input, &MeshOptions::default(), Counts::scaled(0.05), 7).unwrap();
    assert!(r.clean(), "{}\n{:?}", r.line(), r.first);
    assert_eq!(r.pieces, 1, "{}", r.line());
}

/// Walks: from a spawn point (the office's numbers) facing `yaw`, `W` held for `ms`; the figure must end inside `want`
/// (x and y ranges in the office's numbers) on the floor.
#[test]
fn the_figure_walks_in_through_the_doors_and_is_stopped_by_furniture() {
    struct Walk {
        what: &'static str,
        from: [f32; 2],
        yaw: f32,
        ms: u32,
        want: [[f32; 2]; 2],
    }
    let walks = [
        // In at the front door, up to the reception counter (its front at y 9).
        Walk {
            what: "front door",
            from: [5.5, -3.0],
            yaw: 0.0,
            ms: 3500,
            want: [[4.8, 6.2], [7.6, 8.6]],
        },
        // East along the corridor to the plant at its end.
        Walk {
            what: "corridor",
            from: [13.0, 14.0],
            yaw: std::f32::consts::FRAC_PI_2,
            ms: 5000,
            want: [[25.0, 35.0], [13.5, 14.5]],
        },
        // East along the aisle between the rows of desks.
        Walk {
            what: "aisle",
            from: [12.6, 6.5],
            yaw: std::f32::consts::FRAC_PI_2,
            ms: 4000,
            want: [[20.0, 35.6], [5.6, 7.4]],
        },
        // North through the kitchen door to the table.
        Walk {
            what: "kitchen door",
            from: [7.0, 13.6],
            yaw: 0.0,
            ms: 2500,
            want: [[6.5, 7.5], [16.0, 22.8]],
        },
        // North through the meeting room door, stopped by a chair at the table.
        Walk {
            what: "meeting door",
            from: [15.0, 13.6],
            yaw: 0.0,
            ms: 2500,
            want: [[14.4, 15.6], [15.6, 17.8]],
        },
        // North through the print room door to the shelf.
        Walk {
            what: "print door",
            from: [23.0, 13.6],
            yaw: 0.0,
            ms: 3000,
            want: [[22.4, 23.6], [18.0, 23.2]],
        },
        // North through the private office door, between the visitors' chairs to the desk.
        Walk {
            what: "private door",
            from: [31.0, 13.6],
            yaw: 0.0,
            ms: 3000,
            want: [[30.4, 31.6], [16.0, 20.2]],
        },
    ];
    let mut files = GameFiles::bundled().expect("the game folder");
    torque_example_level_demo::levels::add_levels(&mut files).unwrap();
    let office =
        String::from_utf8_lossy(files.file("missions/office.mis").expect("the office map"))
            .into_owned();
    let mut failures = Vec::new();
    for (k, w) in walks.iter().enumerate() {
        let spawn = format!(
            "spawn = \"{} {} 0.2\"; spawnYaw = {};",
            w.from[0] + CORNER[0],
            w.from[1] + CORNER[1],
            w.yaw
        );
        let text = office.replace("spawn = \"-12.5 -15 0.2\"; spawnYaw = 0;", &spawn);
        files.insert(&format!("missions/zz-office-walk{k}.mis"), text.as_bytes());
    }
    let mut game = LevelDemo::new(files).expect("the client");
    for (k, w) in walks.iter().enumerate() {
        let path = format!("missions/zz-office-walk{k}.mis");
        let map = game
            .maps
            .iter()
            .position(|m| m.path == path)
            .expect("listed");
        assert!(enter(&mut game, map), "{} did not start", w.what);
        for _ in 0..30 {
            game.step(32);
        }
        hold(&mut game, Key::W, w.ms);
        for _ in 0..20 {
            game.step(32);
        }
        let p = server_position(&game);
        let at = [p[0] - CORNER[0], p[1] - CORNER[1]];
        let inside = (0..2).all(|i| at[i] >= w.want[i][0] && at[i] <= w.want[i][1]);
        if !inside || p[2].abs() > 0.15 {
            failures.push(format!(
                "{}: ended at {at:?} (z {}), want x {:?} y {:?}",
                w.what, p[2], w.want[0], w.want[1]
            ));
        }
        hold(&mut game, Key::Escape, 64);
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// The office as the example builds it: its materials' flags in the built `.dif`, as the importer reads them back.
#[test]
fn the_built_office_marks_its_glass_see_through_and_every_texture_nearest() {
    use torque::mapcore::Importer as _;
    use torque::mapcore::material::MaterialFlags;
    use torque_maps::dif::surface_flags;
    let mut files = GameFiles::bundled().expect("the game folder");
    torque_example_level_demo::levels::add_levels(&mut files).unwrap();
    let bytes =
        torque_render_scene::Assets::read(&files.kit, "kit/office.dif").expect("the office's .dif");
    let dif = torque_maps::dif::parse(&bytes).unwrap();
    let i = &dif.details[0];
    let mut glass = 0;
    for s in &i.surfaces {
        let name = &i.materials[s.texture as usize];
        assert!(s.flags & surface_flags::NEAREST != 0, "{name} not nearest");
        let see_through = s.flags & surface_flags::TRANSLUCENT != 0;
        assert_eq!(see_through, name == "office_glass", "{name}");
        glass += usize::from(see_through);
    }
    assert!(glass > 50, "{glass} glass surfaces");
    let read = torque_maps::legacy::IMPORTER.surfaces(&bytes).unwrap();
    assert_eq!(
        read.material_flags.get("office_glass"),
        Some(&MaterialFlags::TRANSLUCENT.with(MaterialFlags::NEAREST))
    );
    assert_eq!(
        read.material_flags.get("office_plaster"),
        Some(&MaterialFlags::NEAREST)
    );
}

/// Nothing in the built office hovers over the floor: every face of the `.dif` that looks down from less than 0.3 units above
/// the floor (the underside of a chair's base, a sofa, a table's foot) has a face looking up at its height under it (a rug, a
/// caster, the piece it stands on). The floor's top is at 0.
#[test]
fn every_piece_of_furniture_stands_on_the_floor_or_on_something() {
    use torque_maps::dif::PLANE_FLIPPED;
    let mut files = GameFiles::bundled().expect("the game folder");
    torque_example_level_demo::levels::add_levels(&mut files).unwrap();
    let bytes =
        torque_render_scene::Assets::read(&files.kit, "kit/office.dif").expect("the office's .dif");
    let dif = torque_maps::dif::parse(&bytes).unwrap();
    let i = &dif.details[0];
    // Each surface: its normal's z, its height (for a level face) and its rectangle in x and y.
    let faces: Vec<(f32, f32, [f32; 4])> = i
        .surfaces
        .iter()
        .map(|s| {
            let plane = &i.planes[usize::from(s.plane & !PLANE_FLIPPED)];
            let nz = if s.plane & PLANE_FLIPPED != 0 {
                -plane.normal[2]
            } else {
                plane.normal[2]
            };
            let (mut rect, mut z) = ([f32::MAX, f32::MAX, f32::MIN, f32::MIN], 0.0);
            let points = &i.windings[s.winding_start as usize..][..s.winding_count as usize];
            for &p in points {
                let [x, y, pz] = i.points[p as usize];
                rect = [
                    rect[0].min(x),
                    rect[1].min(y),
                    rect[2].max(x),
                    rect[3].max(y),
                ];
                z += pz / points.len() as f32;
            }
            (nz, z, rect)
        })
        .collect();
    let overlap = |a: [f32; 4], b: [f32; 4]| {
        a[0] < b[2] - 1e-3 && b[0] < a[2] - 1e-3 && a[1] < b[3] - 1e-3 && b[1] < a[3] - 1e-3
    };
    let mut hovering = Vec::new();
    for &(nz, z, rect) in &faces {
        if nz > -0.99 || z <= 1e-3 || z >= 0.3 {
            continue;
        }
        let held = faces
            .iter()
            .any(|&(up, uz, under)| up > 0.99 && (uz - z).abs() < 1e-3 && overlap(rect, under));
        if !held {
            hovering.push((z, rect));
        }
    }
    assert!(
        hovering.is_empty(),
        "{} faces hover over the floor, the first {:?}",
        hovering.len(),
        &hovering[..hovering.len().min(4)]
    );
}

/// A web build's pack carries the office built (its interior, textures and map), not its model: the browser builds nothing
/// when it starts, and the client starts on the pack as it is.
#[test]
fn the_web_pack_carries_the_office_built_not_its_model() {
    use torque_example_level_demo::files::web_pack;
    let pack = web_pack(GameFiles::bundled().expect("the game folder")).expect("the pack");
    let files = GameFiles::unpack(&pack).expect("the pack reads back");
    assert!(
        files.file("levels/office.bbmodel").is_none(),
        "the model is not carried"
    );
    assert!(
        files.file("levels/office.mis").is_none(),
        "the template is not carried"
    );
    let mission =
        String::from_utf8_lossy(files.file("missions/office.mis").expect("the office's map"))
            .into_owned();
    assert!(
        !mission.contains(torque_example_level_demo::levels::INTERIORS),
        "the interiors are placed"
    );
    assert!(mission.contains("kit/office"), "{mission}");
    assert!(
        files.kit.paths().any(|p| p == "kit/office.dif"),
        "the built interior is in the kit"
    );
    // the client starts on it with nothing left to build
    let game = LevelDemo::new(files).expect("the client");
    assert!(game.maps.iter().any(|m| m.path == "missions/office.mis"));
}

/// Standing in the corridor with its back to the meeting room's glass wall (1.4 units behind the figure, a pane's middle between
/// two posts), the camera behind it looks through the glass from inside the meeting room instead of stopping in front of it.
#[test]
fn the_camera_behind_the_figure_looks_through_the_meeting_rooms_glass() {
    let mut files = GameFiles::bundled().expect("the game folder");
    torque_example_level_demo::levels::add_levels(&mut files).unwrap();
    let office =
        String::from_utf8_lossy(files.file("missions/office.mis").expect("the office map"))
            .into_owned();
    let spawn = format!(
        "spawn = \"{} {} 0.2\"; spawnYaw = {};",
        11.0 + CORNER[0],
        13.6 + CORNER[1],
        std::f32::consts::PI
    );
    let text = office.replace("spawn = \"-12.5 -15 0.2\"; spawnYaw = 0;", &spawn);
    files.insert("missions/zz-office-glass.mis", text.as_bytes());
    let mut game = LevelDemo::new(files).expect("the client");
    let index = game
        .maps
        .iter()
        .position(|m| m.path.ends_with("zz-office-glass.mis"))
        .unwrap();
    assert!(enter(&mut game, index), "the office did not start");
    let mut eye = None;
    for _ in 0..90 {
        game.step(16);
        game.update_scene(0.016);
        eye = Some(game.drawn_eye());
    }
    let eye = eye.unwrap();
    let figure = game.figure().expect("the figure").position;
    // The glass's middle is at y 15 in the office's numbers.
    let glass = 15.0 + CORNER[1];
    assert!(figure[1] < glass - 1.0, "the figure at {figure:?}");
    assert!(
        eye.position[1] > glass + 1.0,
        "the camera at {:?}, the glass at y {glass}",
        eye.position
    );
}

/// The camera's line of sight passes a window's panes and the meeting room's glass, and is still stopped by the wall under, over
/// and beside a window, the window's mullion and the glass wall's post.
#[test]
fn the_camera_sees_through_the_panes_and_not_through_the_wall_around_them() {
    let mut files = GameFiles::bundled().expect("the game folder");
    torque_example_level_demo::levels::add_levels(&mut files).unwrap();
    let mut game = LevelDemo::new(files).expect("the client");
    let index = game
        .maps
        .iter()
        .position(|m| m.path.ends_with("office.mis"))
        .unwrap();
    assert!(enter(&mut game, index), "the office did not start");
    let at = |x: f32, y: f32, z: f32| [x + CORNER[0], y + CORNER[1], z];
    // The interiors are placed for the line of sight by the frames.
    let mut placed = false;
    for _ in 0..200 {
        game.step(16);
        game.update_scene(0.016);
        if game
            .sight_share(at(14.5, 1.0, 0.5), at(14.5, -1.0, 0.5))
            .is_some()
        {
            placed = true;
            break;
        }
    }
    assert!(placed, "the office's walls never stopped the line of sight");
    // The south wall's window at x 15 (14.2 to 15.8, its mullion in the middle, its glass from 0.9 to 2.6 up): lines from
    // inside (y 1) out through the wall (y -1). The meeting room's glass wall at y 15, its posts at x 10, 12 and 14.
    let lines = [
        (
            "the window's left pane",
            at(14.5, 1.0, 1.7),
            at(14.5, -1.0, 1.7),
            false,
        ),
        (
            "the window's right pane",
            at(15.5, 1.0, 1.7),
            at(15.5, -1.0, 1.7),
            false,
        ),
        (
            "the wall under the window",
            at(14.5, 1.0, 0.5),
            at(14.5, -1.0, 0.5),
            true,
        ),
        (
            "the wall over the window",
            at(14.5, 1.0, 2.9),
            at(14.5, -1.0, 2.9),
            true,
        ),
        (
            "the wall beside the window",
            at(13.5, 1.0, 1.7),
            at(13.5, -1.0, 1.7),
            true,
        ),
        (
            "the window's mullion",
            at(15.0, 1.0, 1.7),
            at(15.0, -1.0, 1.7),
            true,
        ),
        (
            "the meeting room's glass",
            at(11.0, 14.0, 1.7),
            at(11.0, 16.0, 1.7),
            false,
        ),
        (
            "the glass wall's post",
            at(12.0, 14.0, 1.7),
            at(12.0, 16.0, 1.7),
            true,
        ),
    ];
    let mut wrong = Vec::new();
    for (what, from, to, stopped) in lines {
        let hit = game.sight_share(from, to);
        if hit.is_some() != stopped {
            wrong.push(format!("{what}: {hit:?}"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
