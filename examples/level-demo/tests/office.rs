//! The office level (`game/levels/office.bbmodel`): its building built into one interior that passes the builder's
//! verification, its furniture and desk items props that start asleep where the model put them and move only when pushed, and
//! walked through in the level demo's figure: in at the front door, along the corridor and an aisle of desks, through each
//! room's door, stopped by the fittings or pushing the furniture.

use std::collections::BTreeMap;

use torque::client::input::Key;
use torque_example_level_demo::client::LevelDemo;
use torque_example_level_demo::files::GameFiles;
use torque_example_level_demo::props::{LevelProp, PropKind, split};
use torque_import::modelcore::Model;
use torque_import::{Options, bbmodel};
use torque_interior_builder::mesh::{MeshInput, MeshOptions};
use torque_interior_builder::verify::Counts;
use torque_interior_builder::verify::mesh::run_mesh;
use torque_plugin_props::quant::Quantized;
use torque_plugin_props::world::Rest;
use torque_plugin_props::{ClientProps, PropLimits, ServerProps};

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

/// The office's model cut into its building and its props, as the example cuts it.
fn office_model() -> (Model, Vec<LevelProp>) {
    let files = GameFiles::bundled().expect("the game folder");
    let bytes = files.file("levels/office.bbmodel").expect("the office");
    let options = Options {
        box_hull: false,
        ..Options::default()
    };
    let model = bbmodel::read(bytes, &options, &mut |_| true).unwrap();
    split(model).expect("the office's props")
}

#[test]
fn the_office_builds_into_one_interior_that_passes_verification() {
    let (model, _) = office_model();
    let input = MeshInput::from_model(&model).unwrap();
    let r = run_mesh(&input, &MeshOptions::default(), Counts::scaled(0.05), 7).unwrap();
    assert!(r.clean(), "{}\n{:?}", r.line(), r.first);
    assert_eq!(r.pieces, 1, "{}", r.line());
}

/// Walks: from a spawn point (the office's numbers) facing `yaw`, `W` held for `ms`; the figure must end inside `want`
/// (x and y ranges in the office's numbers) on the floor.
#[test]
fn the_figure_walks_in_through_the_doors_stopped_by_the_fittings_or_pushing_the_furniture() {
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
        // North through the meeting room door, pushing a chair at the table and the table with it (the chair's front was at
        // y 17.8).
        Walk {
            what: "meeting door",
            from: [15.0, 13.6],
            yaw: 0.0,
            ms: 2500,
            want: [[14.4, 15.6], [18.0, 22.8]],
        },
        // North through the print room door to the shelf.
        Walk {
            what: "print door",
            from: [23.0, 13.6],
            yaw: 0.0,
            ms: 3000,
            want: [[22.4, 23.6], [18.0, 23.2]],
        },
        // North through the private office door, between the visitors' chairs, pushing the desk (its front was at y 20.6) back
        // against the chair behind it.
        Walk {
            what: "private door",
            from: [31.0, 13.6],
            yaw: 0.0,
            ms: 3000,
            want: [[30.4, 31.6], [20.4, 23.4]],
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
        // At full speed (Shift held: a sprint), as the walks' distances were measured.
        game.key(Key::ShiftLeft, true);
        hold(&mut game, Key::W, w.ms);
        game.key(Key::ShiftLeft, false);
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
        // Esc, then the pause menu's Leave game: back to the menu of maps.
        hold(&mut game, Key::Escape, 64);
        game.call("leaveGame", &[]).expect("leaveGame");
        game.step(32);
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

/// Each kind of item and how many the office has: every movable piece of furniture and every desk item.
const ITEMS: [(PropKind, usize); 21] = [
    (PropKind::Chair, 41),
    (PropKind::SideChair, 6),
    (PropKind::Armchair, 1),
    (PropKind::Sofa, 2),
    (PropKind::Bench, 1),
    (PropKind::Desk, 33),
    (PropKind::Table, 3),
    (PropKind::Cabinet, 7),
    (PropKind::Copier, 1),
    (PropKind::Fridge, 1),
    (PropKind::Cooler, 2),
    (PropKind::Bin, 1),
    (PropKind::Lamp, 1),
    (PropKind::Plant, 10),
    (PropKind::Boxes, 1),
    (PropKind::Monitor, 34),
    (PropKind::Keyboard, 32),
    (PropKind::Laptop, 2),
    (PropKind::Mug, 16),
    (PropKind::Magazines, 1),
    (PropKind::CoffeeMachine, 1),
];

/// What the building's boxes are named after: the floors, walls, windows, roof, ceiling lights, rugs, art and whiteboards,
/// the reception counter (its desk is the counter's own top), the kitchen's counter, sink and upper cabinets, the shelves,
/// the meeting room's wall screen and the cubicles' partitions.
const FITTINGS: [&str; 26] = [
    "floor",
    "wall",
    "pier",
    "lintel",
    "under",
    "over",
    "glass",
    "mullion",
    "sill",
    "door head",
    "roof",
    "parapet",
    "plinth",
    "canopy",
    "light",
    "rug",
    "art",
    "whiteboard",
    "reception",
    "counter",
    "sink",
    "upper cabinets",
    "shelf",
    "meeting screen",
    "middle",
    "divider",
];

#[test]
fn every_piece_of_furniture_and_every_desk_item_is_a_prop_and_the_fittings_stay_interior() {
    let (building, props) = office_model();
    let mut kinds: BTreeMap<PropKind, usize> = BTreeMap::new();
    for p in &props {
        *kinds.entry(p.kind).or_default() += 1;
    }
    assert_eq!(kinds, BTreeMap::from(ITEMS), "the items by kind");
    let mut names: Vec<&str> = props.iter().map(|p| p.name.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), props.len(), "each item's name is its own");
    // The building is its fittings alone, and has every kind of them.
    let strays: Vec<&str> = building
        .meshes
        .iter()
        .map(|m| m.name.as_str())
        .filter(|n| !FITTINGS.iter().any(|f| n.contains(f)))
        .collect();
    assert!(
        strays.is_empty(),
        "the building has boxes that are not fittings: {strays:?}"
    );
    for f in FITTINGS {
        assert!(
            building.meshes.iter().any(|m| m.name.contains(f)),
            "no {f} in the building"
        );
    }
    // An item is one mesh of all its boxes (twelve triangles a box): a monitor's foot, neck and screen, a plant's pot to its
    // tufts.
    for (name, boxes) in [("poda sw monitor", 3), ("lobby plant west", 7)] {
        let item = props.iter().find(|p| p.name == name).expect(name);
        let [mesh] = &item.shape.meshes[..] else {
            panic!("{name}: {} meshes", item.shape.meshes.len())
        };
        assert_eq!(mesh.triangles(), boxes * 12, "{name}");
    }
    // Under the props' caps: the server's, and the awake ones' were every item awake at once.
    let caps = PropLimits::default();
    assert!(
        props.len() < caps.per_server as usize && props.len() < caps.awake as usize,
        "{} props",
        props.len()
    );
}

/// The office's built shapes of its props: items of one kind and shape share one (the pods' desks, keyboards and mugs), and
/// every material is sampled nearest, a `_glow` one drawn unlit (self-illuminating), as the interior's are drawn.
#[test]
fn the_props_shapes_are_drawn_nearest_and_their_glow_unlit() {
    use torque::shapes::matlist::NEAREST;
    /// TGE's `TSMaterialList::SelfIlluminating`: drawn with lighting off.
    const SELF_ILLUMINATING: u32 = 0x20;
    let mut files = GameFiles::bundled().expect("the game folder");
    torque_example_level_demo::levels::add_levels(&mut files).unwrap();
    let shapes: Vec<String> = files
        .kit
        .paths()
        .filter(|p| p.starts_with("kit/office_prop_"))
        .map(str::to_string)
        .collect();
    let props = office_model().1.len();
    assert!(
        (21..props / 2).contains(&shapes.len()),
        "{} prop shapes for {props} props",
        shapes.len()
    );
    let mut glow = 0;
    for path in &shapes {
        let bytes = torque_render_scene::Assets::read(&files.kit, path).expect("the shape");
        let shape = torque::shapes::dts::parse(&bytes, &mut |_| true).expect("it reads");
        for m in &shape.shape.materials {
            let name = String::from_utf8_lossy(m.name.as_deref().unwrap_or_default());
            assert!(m.flags & NEAREST != 0, "{path}: {name} not nearest");
            let unlit = m.flags & SELF_ILLUMINATING != 0;
            assert_eq!(unlit, name.ends_with("_glow"), "{path}: {name}");
            glow += usize::from(unlit);
        }
    }
    assert!(glow >= 2, "the monitors' and the laptops' screens glow");
}

/// The office with the figure spawned at `at` (the office's numbers) facing `yaw`: the client, and its map's index.
fn office_from(at: [f32; 2], yaw: f32) -> (LevelDemo, usize) {
    let mut files = GameFiles::bundled().expect("the game folder");
    torque_example_level_demo::levels::add_levels(&mut files).unwrap();
    let office =
        String::from_utf8_lossy(files.file("missions/office.mis").expect("the office map"))
            .into_owned();
    let spawn = format!(
        "spawn = \"{} {} 0.2\"; spawnYaw = {yaw};",
        at[0] + CORNER[0],
        at[1] + CORNER[1],
    );
    let text = office.replace("spawn = \"-12.5 -15 0.2\"; spawnYaw = 0;", &spawn);
    files.insert("missions/zz-office-from.mis", text.as_bytes());
    let game = LevelDemo::new(files).expect("the client");
    let index = game
        .maps
        .iter()
        .position(|m| m.path.ends_with("zz-office-from.mis"))
        .expect("listed");
    (game, index)
}

/// The server's props: each one's state and rest, by object id.
fn server_props(game: &LevelDemo) -> BTreeMap<u32, (Quantized, Rest)> {
    let props = game
        .engine()
        .resource::<ServerProps>()
        .expect("the props plugin")
        .clone();
    props
        .with(|s| {
            s.world.as_ref().map(|w| {
                w.props()
                    .iter()
                    .map(|(&id, p)| (id, (p.q, p.rest)))
                    .collect()
            })
        })
        .flatten()
        .unwrap_or_default()
}

/// The client's book of props: each one's state as the server sent it, by ghost index.
fn client_props(game: &LevelDemo) -> BTreeMap<u32, Quantized> {
    let book = game
        .engine()
        .resource::<ClientProps>()
        .expect("the props plugin")
        .clone();
    book.with(|b| b.props().iter().map(|(&i, p)| (i, p.q)).collect())
        .unwrap_or_default()
}

/// The office running with the figure at `at` facing `yaw`, every prop sent to the client.
fn office_running(at: [f32; 2], yaw: f32) -> LevelDemo {
    let (mut game, index) = office_from(at, yaw);
    assert!(enter(&mut game, index), "the office did not start");
    let count = office_model().1.len();
    for _ in 0..600 {
        if client_props(&game).len() >= count {
            break;
        }
        game.step(32);
    }
    assert_eq!(
        client_props(&game).len(),
        count,
        "the props the client was sent"
    );
    game
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>().sqrt()
}

/// How high a turn leaves the up axis: 1 standing, 0 on its side.
fn upright(q: [f32; 4]) -> f32 {
    let [x, y, _, _] = q;
    1.0 - 2.0 * (x * x + y * y)
}

#[test]
fn the_props_start_asleep_where_the_model_put_them_and_nothing_moves_while_no_one_touches_them() {
    // The figure stays outside, in front of the door.
    let mut game = office_running([5.5, -3.0], 0.0);
    let start = server_props(&game);
    let items = office_model().1;
    assert_eq!(start.len(), items.len());
    // Each where the model has it (to the wire's precision), asleep.
    let unplaced: Vec<&str> = items
        .iter()
        .filter(|item| {
            !start.values().any(|(q, rest)| {
                let r = q.rotation();
                let turn: f32 = (0..4).map(|k| r[k] * item.rotation[k]).sum();
                *rest == Rest::Asleep
                    && distance(q.position(), item.position) < 2e-3
                    && turn.abs() > 0.9999
            })
        })
        .map(|item| item.name.as_str())
        .collect();
    assert!(
        unplaced.is_empty(),
        "not asleep where the model has them: {unplaced:?}"
    );
    let sent = client_props(&game);
    for _ in 0..300 {
        game.step(32);
    }
    assert_eq!(server_props(&game), start, "the server's props moved");
    assert_eq!(client_props(&game), sent, "the client's props moved");
}

#[test]
fn every_prop_rests_on_the_floor_a_rug_or_the_piece_under_it_when_woken() {
    let mut game = office_running([5.5, -3.0], 0.0);
    let start = server_props(&game);
    // Woken, each meets whatever is under it at once: the floor, a rug, a desk, the counter.
    let wake: String = start
        .keys()
        .map(|id| format!("propSetVelocity({id}, \"0 0 0\");"))
        .collect();
    game.server
        .as_mut()
        .expect("the server")
        .vm_mut()
        .eval(wake.as_bytes())
        .expect("propSetVelocity");
    for _ in 0..120 {
        game.step(32);
    }
    let after = server_props(&game);
    let mut moved = Vec::new();
    for (id, (q, _)) in &start {
        let (now, rest) = after[id];
        let shift = distance(q.position(), now.position());
        if shift > 0.01 || upright(now.rotation()) < 0.999 || rest != Rest::Asleep {
            moved.push((*id, shift, upright(now.rotation()), rest));
        }
    }
    assert!(
        moved.is_empty(),
        "{} props moved when woken: {moved:?}",
        moved.len()
    );
}

#[test]
fn the_figure_pushes_the_chair_it_walks_into() {
    // In the aisle west of the first pod's north-west chair, facing east along the row of chairs.
    let items = office_model().1;
    let chair = items
        .iter()
        .find(|p| p.name == "poda nw chair")
        .expect("the chair");
    let mut game = office_running([12.65, 5.15], std::f32::consts::FRAC_PI_2);
    let start = server_props(&game);
    let (&id, _) = start
        .iter()
        .min_by(|a, b| {
            let d = |q: &Quantized| distance(q.position(), chair.position);
            d(&a.1.0).total_cmp(&d(&b.1.0))
        })
        .expect("a prop");
    for _ in 0..30 {
        game.step(32);
    }
    hold(&mut game, Key::W, 1200);
    for _ in 0..40 {
        game.step(32);
    }
    let (q, _) = server_props(&game)[&id];
    let moved = q.position()[0] - chair.position[0];
    assert!(
        moved > 0.3,
        "the chair moved {moved} east, to {:?}",
        q.position()
    );
    assert!(
        upright(q.rotation()) > 0.98,
        "the chair tipped: {}",
        upright(q.rotation())
    );
    // The client was sent where it went.
    assert!(
        client_props(&game)
            .values()
            .any(|c| distance(c.position(), q.position()) < 0.01),
        "the client has no prop where the chair is"
    );
}

/// Runs `code` on the office's server and gives its result.
fn server_eval(game: &mut LevelDemo, code: &str) -> String {
    let out = game
        .server
        .as_mut()
        .expect("the server")
        .vm_mut()
        .eval(code.as_bytes())
        .expect("the server runs it");
    String::from_utf8_lossy(&out).into_owned()
}

/// The server's prop nearest `at` (the model's place of an item).
fn prop_at(game: &LevelDemo, at: [f32; 3]) -> u32 {
    let (&id, _) = server_props(game)
        .iter()
        .min_by(|a, b| {
            let d = |q: &Quantized| distance(q.position(), at);
            d(&a.1.0).total_cmp(&d(&b.1.0))
        })
        .expect("a prop");
    id
}

/// The office with the figure in the aisle north of the first pod's north-west chair, facing it and looking down at it:
/// the client, the chair's prop and the model's chair.
fn facing_the_chair() -> (LevelDemo, u32, LevelProp) {
    let items = office_model().1;
    let chair = items
        .into_iter()
        .find(|p| p.name == "poda nw chair")
        .expect("the chair");
    let x = chair.position[0] - CORNER[0];
    let mut game = office_running([x, 6.6], std::f32::consts::PI);
    let id = prop_at(&game, chair.position);
    // Looking down at the chair (a move's pitch, as the mouse gives it).
    game.set_global("$mvPitch", "0.6");
    for _ in 0..20 {
        game.step(32);
    }
    (game, id, chair)
}

/// The holder of prop `id` on the server (0 for none) and its hold's joint.
fn holder(game: &mut LevelDemo, id: u32) -> (u32, u32) {
    let figure = game.server.as_ref().and_then(|s| s.figure()).unwrap_or(0);
    let joint = server_eval(game, &format!("return {figure}.holdJoint;"));
    let props = game
        .engine()
        .resource::<ServerProps>()
        .expect("the props plugin")
        .clone();
    let held_by = props
        .with(|s| s.world.as_ref().and_then(|w| w.holder_of(id)))
        .flatten()
        .unwrap_or(0);
    (held_by, joint.parse().unwrap_or(0))
}

#[test]
fn a_click_picks_up_the_chair_it_looks_at_carries_it_on_a_walk_and_a_second_click_drops_it() {
    let (mut game, id, _) = facing_the_chair();
    let figure = game
        .server
        .as_ref()
        .and_then(|s| s.figure())
        .expect("the figure");
    game.call("grab", &["1"]).expect("the click");
    for _ in 0..10 {
        game.step(32);
    }
    let (held_by, joint) = holder(&mut game, id);
    let refused = game
        .server
        .as_ref()
        .and_then(|s| s.vm().global("$World::GrabRefused"))
        .map(|w| String::from_utf8_lossy(&w).into_owned())
        .unwrap_or_default();
    assert_eq!(held_by, figure, "not picked up: {refused:?}");
    assert!(joint != 0);
    // The arm held out: its thread plays `hold` already.
    assert_eq!(
        server_eval(
            &mut game,
            &format!("return {figure}.setArmThread(\"hold\");")
        ),
        "0"
    );
    // The client is sent it held, and predicts it.
    let book = game
        .engine()
        .resource::<ClientProps>()
        .expect("the props plugin")
        .clone();
    // A walk back up the aisle and along it, the chair kept at its point; the client predicts it, drawn where the server has
    // it within a few centimeters once the walk ends.
    let mut worst = 0.0f32;
    let mut predicted = 0;
    game.set_global("$mvPitch", "-0.6");
    for (key, ms) in [(Key::A, 900), (Key::S, 600), (Key::D, 900)] {
        game.key(key, true);
        for _ in 0..ms / 32 {
            game.step(32);
            let d: f32 = server_eval(&mut game, &format!("return jointGetDistance({joint});"))
                .parse()
                .unwrap_or(f32::MAX);
            worst = worst.max(d);
            predicted += book
                .with(|b| {
                    b.props()
                        .values()
                        .filter(|p| p.held && p.predicted.is_some())
                        .count()
                })
                .unwrap_or(0);
        }
        game.key(key, false);
    }
    assert!(
        worst < 0.05,
        "the held chair strayed {worst} from its hold point"
    );
    assert!(
        predicted > 30,
        "the client predicted the held chair {predicted} ticks"
    );
    // Upright still, and the figure walked: its held chair did not push it.
    let (q, _) = server_props(&game)[&id];
    assert!(upright(q.rotation()) > 0.99);
    // A second click drops it: it falls to the floor and sleeps, held by no one.
    game.call("grab", &["1"]).expect("the click");
    for _ in 0..120 {
        game.step(32);
    }
    let (held_by, _) = holder(&mut game, id);
    let (q, rest) = server_props(&game)[&id];
    assert_eq!(held_by, 0);
    assert_eq!(rest, Rest::Asleep);
    assert!(
        q.position()[2] < 0.6,
        "the dropped chair is on the floor: {:?}",
        q.position()
    );
    assert_eq!(
        server_eval(
            &mut game,
            &format!("return {figure}.setArmThread(\"look\");")
        ),
        "0",
        "the arm hangs again"
    );
}

/// A second figure on the server, standing at `at` (the office's numbers) facing north, its eye level: what `grabFor` gives
/// for it, and why it refused.
fn second_grabs(game: &mut LevelDemo, at: [f32; 2]) -> (String, String) {
    let [x, y] = [at[0] + CORNER[0], at[1] + CORNER[1]];
    server_eval(
        game,
        &format!(
            "if (!isObject(Second)) new Player(Second) {{ dataBlock = WorldFigureData; }};              Second.setTransform(\"{x} {y} 0.05 0 0 1 0\");"
        ),
    );
    // Its eye follows from its next tick.
    for _ in 0..3 {
        game.step(32);
    }
    let got = server_eval(game, "return grabFor(Second, 0);");
    let why = game
        .server
        .as_ref()
        .and_then(|s| s.vm().global("$World::GrabRefused"))
        .map(|w| String::from_utf8_lossy(&w).into_owned())
        .unwrap_or_default();
    (got, why)
}

#[test]
fn a_prop_out_of_reach_too_heavy_or_held_by_another_is_refused() {
    let (mut game, id, _) = facing_the_chair();
    let figure = game
        .server
        .as_ref()
        .and_then(|s| s.figure())
        .expect("the figure");
    // The lobby's west plant, its crown at eye height: out of reach 4 units south of it, picked up from 1.8.
    let (got, why) = second_grabs(&mut game, [0.8, 10.0]);
    assert_eq!((got.as_str(), why.as_str()), ("0", "nothing in reach"));
    let (got, why) = second_grabs(&mut game, [0.8, 12.2]);
    assert!(got != "0", "the plant in reach was refused: {why}");
    server_eval(&mut game, "dropHeld(Second);");
    // Too heavy: the fridge, 1.1 units south of it.
    let (got, why) = second_grabs(&mut game, [7.5, 22.0]);
    assert_eq!((got.as_str(), why.as_str()), ("0", "too heavy"));
    // A held prop: the first figure takes the chair, and the second cannot, by the props plugin's own rule.
    assert_ne!(
        server_eval(&mut game, &format!("return grabFor({figure}, 0);")),
        "0"
    );
    let taken = server_eval(
        &mut game,
        &format!("return jointHold(Second, {id}, \"0 1 1\");"),
    );
    assert!(taken.is_empty() || taken == "0", "{taken}");
    assert_eq!(holder(&mut game, id).0, figure);
}

#[test]
fn the_holder_dying_drops_what_it_holds() {
    let (mut game, id, _) = facing_the_chair();
    let figure = game
        .server
        .as_ref()
        .and_then(|s| s.figure())
        .expect("the figure");
    assert_ne!(
        server_eval(&mut game, &format!("return grabFor({figure}, 0);")),
        "0"
    );
    game.step(32);
    assert_eq!(holder(&mut game, id).0, figure);
    server_eval(&mut game, &format!("{figure}.setDamageLevel(100);"));
    game.step(32);
    assert_eq!(
        holder(&mut game, id).0,
        0,
        "the dead figure still holds the chair"
    );
}

#[test]
fn a_held_prop_that_is_frozen_is_let_go() {
    let (mut game, id, _) = facing_the_chair();
    let figure = game
        .server
        .as_ref()
        .and_then(|s| s.figure())
        .expect("the figure");
    assert_ne!(
        server_eval(&mut game, &format!("return grabFor({figure}, 0);")),
        "0"
    );
    game.step(32);
    server_eval(&mut game, &format!("propFreeze({id});"));
    for _ in 0..3 {
        game.step(32);
    }
    let (held_by, joint) = holder(&mut game, id);
    assert_eq!(
        (held_by, joint),
        (0, 0),
        "the figure let go of the frozen chair"
    );
    assert_eq!(
        server_eval(
            &mut game,
            &format!("return {figure}.setArmThread(\"look\");")
        ),
        "0",
        "the arm hangs again"
    );
}

#[test]
fn the_sleeping_furniture_is_drawn_still_and_a_pushed_chair_moves_again() {
    let Ok(gpu) = torque::app::window::offscreen_gpu() else {
        eprintln!("skipped: no GPU");
        return;
    };
    let mut game = office_running([12.65, 5.15], std::f32::consts::FRAC_PI_2);
    let mut view = game.view(&gpu);
    let mut frames = |game: &mut LevelDemo, n: u32| {
        for _ in 0..n {
            game.step(32);
            let _ = game.frame(&mut view);
        }
    };
    frames(&mut game, 5);
    let props = office_model().1.len();
    assert_eq!(
        game.resting_props(),
        props,
        "every sleeping prop drawn still"
    );
    // Walking into the chair wakes it: it is drawn as a moving shape while it moves, and still again once it sleeps.
    game.key(Key::W, true);
    frames(&mut game, 20);
    game.key(Key::W, false);
    assert!(game.resting_props() < props, "the pushed chair moves");
    // Stepped back from it (a prop the figure touches is predicted, so drawn moving), it settles.
    game.key(Key::S, true);
    frames(&mut game, 15);
    game.key(Key::S, false);
    frames(&mut game, 120);
    assert_eq!(
        game.resting_props(),
        props,
        "asleep again, drawn still again"
    );
}

#[test]
fn clicks_reach_the_server_no_faster_than_the_commands_declared_rate() {
    let mut game = office_running([5.5, -3.0], 0.0);
    let ran = |game: &LevelDemo| -> i32 {
        game.server
            .as_ref()
            .and_then(|s| s.vm().global("$World::GrabCommands"))
            .map_or(0, |w| String::from_utf8_lossy(&w).parse().unwrap_or(0))
    };
    let clicks = |game: &mut LevelDemo| {
        for _ in 0..20 {
            game.call("grab", &["1"]).expect("the click");
        }
        for _ in 0..10 {
            game.step(32);
        }
    };
    // A burst of 20 clicks over the network: the burst of 8 runs, the rest are dropped.
    clicks(&mut game);
    assert_eq!(ran(&game), 8);
    // A second later the bucket holds about 4 more.
    for _ in 0..22 {
        game.step(32);
    }
    clicks(&mut game);
    let more = ran(&game) - 8;
    assert!((3..=6).contains(&more), "{more} more ran a second later");
}
