//! A level's props: the furniture and desk items of a Blockbench level, each built into a shape of its own and spawned where
//! the model placed it as a prop of `torque-plugin-props`, asleep, so a figure that walks into one pushes it. [`crate::levels`]
//! builds the rest of the model (the building and its fixed fittings) into interiors.
//!
//! **The convention.** An outliner group named `prop <kind> <name>` ([`PROP_GROUP`], e.g. `prop chair poda sw chair`) is one
//! item: every element in it, and in groups inside it, is one shape and one collision box. `<kind>` is a [`PropKind`] word,
//! which gives the item's mass. The box is the item's bounds in the group's own frame (its origin and turn), so a chair the
//! model turned has its box turned with it: a generator puts an item's turn on its group, not on its elements. A prop group
//! inside another is part of the outer item. A group of any other name is the level's building.
//!
//! **What is built** ([`build`]). Each item's meshes, moved so the box's middle is the shape's origin and merged into one
//! (one object, a draw a material), become a `.dts` (`torque-import`'s writer) of the model's materials, sampled nearest as Blockbench draws them and, for a material whose
//! name ends in [`GLOW`], self-illuminating (drawn unlit), as [`crate::levels::LevelStyle`] draws an interior's. Items of one
//! kind and one shape share the file (`kit/<level>_prop_<n>.dts`) and a `PropData` datablock (`<level>Prop<n>`: its `size` the
//! box, the kind's mass, [`FRICTION`], [`RESTITUTION`]). The map's [`PROPS`] line takes the datablocks and one
//! `propSleep(propSpawn(..))` an item, in the model's order.
//!
//! **Collision.** A kind that is [`PropKind::shaped`] (a chair, a desk, a table, a sofa: pieces with a gap under or in them)
//! collides as its pieces, not as its box: its shape also holds a `Collision-1` detail with one box hull for each of the item's
//! meshes (a chair's base, post, seat and back; a desk's top and sides), and its datablock says `collision = "shape";` (the box
//! is still `size`, which players and shots see). A chair then tucks under a desk and a prop sits in a sofa's seat. The other
//! kinds are boxes. The look is the same: the hulls are never drawn.
//!
//! **At rest.** Every item is spawned asleep where the model has it: nothing moves until a figure (or a prop it pushed)
//! touches it, and a desk's items rest on the desk, a chair on the floor. The map's interiors are what the props land on.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::str::FromStr;

use torque_import::dts;
use torque_import::modelcore::math::{self, Mat4, Quat, Vec3};
use torque_import::modelcore::{Detail, Mesh, Model, Node, Primitive};

use crate::files::GameFiles;
use crate::levels::GLOW;

/// What the name of an outliner group that is one prop starts with (module docs).
pub const PROP_GROUP: &str = "prop ";

/// The line of a level's map the props' datablocks and spawns take the place of.
pub const PROPS: &str = "%props%";

/// Every prop's friction (Rapier averages it with the map's 0.8): enough that a pushed item stops within a few centimeters
/// of where the push ends, too little for a pushed chair to tip on its own feet.
pub const FRICTION: f32 = 0.4;

/// Every prop's bounce: furniture does not bounce.
pub const RESTITUTION: f32 = 0.05;

/// The step an item's own coordinates are snapped to (units): items of one shape placed apart, whose coordinates float
/// rounding left a bit apart, then write one file.
pub const SHAPE_STEP: f32 = 1.0 / 65_536.0;

/// The kinds of item: each variant and the word a prop group names it by, in one table ([`PropKind::ALL`], [`PropKind::word`]).
macro_rules! prop_kinds {
    ($($(#[$doc:meta])* $kind:ident => $word:literal,)*) => {
        /// A kind of item, as the model's prop groups name it: what its mass is.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum PropKind {
            $($(#[$doc])* $kind,)*
        }

        impl PropKind {
            /// Every kind, in order.
            pub const ALL: &[PropKind] = &[$(PropKind::$kind,)*];

            /// The word a prop group names it by.
            pub fn word(self) -> &'static str {
                match self {
                    $(PropKind::$kind => $word,)*
                }
            }
        }
    };
}

prop_kinds! {
    /// An office chair on its swivel base.
    Chair => "chair",
    /// A four-legged chair (a bistro chair, a visitor's).
    SideChair => "side_chair",
    Armchair => "armchair",
    Sofa => "sofa",
    Bench => "bench",
    Desk => "desk",
    Table => "table",
    Cabinet => "cabinet",
    Copier => "copier",
    Fridge => "fridge",
    Cooler => "cooler",
    Bin => "bin",
    Lamp => "lamp",
    Plant => "plant",
    /// Boxes of paper on the floor.
    Boxes => "boxes",
    Monitor => "monitor",
    Keyboard => "keyboard",
    Laptop => "laptop",
    Mug => "mug",
    Magazines => "magazines",
    CoffeeMachine => "coffee_machine",
}

impl PropKind {
    /// Whether an item of this kind collides as its pieces ([module docs](self)).
    pub fn shaped(self) -> bool {
        matches!(
            self,
            PropKind::Chair
                | PropKind::SideChair
                | PropKind::Armchair
                | PropKind::Sofa
                | PropKind::Desk
                | PropKind::Table
        )
    }

    /// The mass (kilograms) of an item of this kind whose box is `size` (units): a desk's and a table's by the area of its
    /// top, every other kind's its own.
    pub fn mass(self, size: Vec3) -> f32 {
        let top = size[0] * size[1];
        match self {
            PropKind::Desk => 18.0 * top,
            PropKind::Table => 15.0 * top,
            PropKind::Chair => 12.0,
            PropKind::SideChair => 5.0,
            PropKind::Armchair => 30.0,
            PropKind::Sofa => 60.0,
            PropKind::Bench => 15.0,
            PropKind::Cabinet => 45.0,
            PropKind::Copier => 90.0,
            PropKind::Fridge => 80.0,
            PropKind::Cooler => 20.0,
            PropKind::Bin => 3.0,
            PropKind::Lamp => 6.0,
            PropKind::Plant => 12.0,
            PropKind::Boxes => 15.0,
            PropKind::Monitor => 5.0,
            PropKind::Keyboard => 1.0,
            PropKind::Laptop => 2.0,
            PropKind::Mug => 0.3,
            PropKind::Magazines => 0.5,
            PropKind::CoffeeMachine => 6.0,
        }
    }
}

impl FromStr for PropKind {
    type Err = String;

    fn from_str(word: &str) -> Result<PropKind, String> {
        PropKind::ALL
            .iter()
            .copied()
            .find(|k| k.word() == word)
            .ok_or_else(|| format!("no prop kind {word:?}"))
    }
}

/// One item of a level, taken out of its model ([`split`]).
#[derive(Debug, Clone, PartialEq)]
pub struct LevelProp {
    /// The name its group gives it, after the kind.
    pub name: String,
    pub kind: PropKind,
    /// Where its box's middle is in the level, and its turn (a unit quaternion, `x y z w`).
    pub position: Vec3,
    pub rotation: Quat,
    /// The box's half extents along the item's own axes.
    pub half: Vec3,
    /// Its meshes about the box's middle, in the item's own axes: the shape it is drawn as, and (for a kind that is
    /// [`PropKind::shaped`]) the collision hulls it also holds.
    pub shape: Model,
    /// The boxes (`[min, max]`, about the box's middle, in the item's own axes) the item's meshes take up, one each: the hulls
    /// of `shape`'s `Collision-1` detail. Empty for a kind that is not shaped.
    pub hulls: Vec<[Vec3; 2]>,
}

/// The prop group `node` is part of: the outermost of it and its ancestors whose name starts with [`PROP_GROUP`].
fn item_of(model: &Model, node: usize) -> Option<usize> {
    let mut found = None;
    let mut at = Some(node);
    // A checked model has no cycle; the count bounds the walk all the same.
    for _ in 0..=model.nodes.len() {
        let Some(i) = at else { break };
        let n = model.nodes.get(i)?;
        if n.name.starts_with(PROP_GROUP) {
            found = Some(i);
        }
        at = n.parent;
    }
    found
}

/// The item's kind and name from its group's name (`prop <kind> <name>`).
fn kind_and_name(group: &str) -> Result<(PropKind, String), String> {
    let rest = group.strip_prefix(PROP_GROUP).unwrap_or(group);
    let (word, name) = rest.split_once(' ').unwrap_or((rest, ""));
    let kind = word
        .parse()
        .map_err(|e| format!("the group {group:?}: {e}"))?;
    Ok((kind, name.trim().to_string()))
}

/// The translation by `t`.
fn moved(t: Vec3) -> Mat4 {
    math::trs(t, math::NO_TURN, [1.0; 3])
}

/// `model` cut in two (module docs): the model without its items' meshes (the level's building, for its interiors), and its
/// items in the order of their groups. A group's kind it does not know is an error.
pub fn split(mut model: Model) -> Result<(Model, Vec<LevelProp>), String> {
    let world = model.rest_world();
    let items: Vec<Option<usize>> = model
        .meshes
        .iter()
        .map(|m| item_of(&model, m.node))
        .collect();
    // Each item's meshes (by group node, in node order), and the building's, its detail levels' meshes renumbered.
    let mut taken: BTreeMap<usize, Vec<Mesh>> = BTreeMap::new();
    let mut kept = Vec::with_capacity(model.meshes.len());
    let mut new_index: Vec<Option<usize>> = Vec::with_capacity(model.meshes.len());
    for (mesh, item) in std::mem::take(&mut model.meshes).into_iter().zip(items) {
        match item {
            Some(group) => {
                new_index.push(None);
                taken.entry(group).or_default().push(mesh);
            }
            None => {
                new_index.push(Some(kept.len()));
                kept.push(mesh);
            }
        }
    }
    for mesh in &mut kept {
        mesh.lod_of = mesh
            .lod_of
            .and_then(|o| new_index.get(o).copied().flatten());
    }
    model.meshes = kept;
    let mut props = Vec::with_capacity(taken.len());
    for (group, meshes) in taken {
        props.push(item(&model, &world, group, meshes)?);
    }
    Ok((model, props))
}

/// The item of group `group` from its `meshes` (in the model's frame of each mesh's node).
fn item(
    model: &Model,
    world: &[Mat4],
    group: usize,
    mut meshes: Vec<Mesh>,
) -> Result<LevelProp, String> {
    let group_name = &model.nodes[group].name;
    let (kind, name) = kind_and_name(group_name)?;
    let to_group = math::inverse(&world[group]);
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for mesh in &mut meshes {
        if mesh.skin.is_some() {
            return Err(format!("the prop {group_name:?} has a skinned mesh"));
        }
        // From the mesh's node into the group's frame (Blockbench's groups move and turn, never scale); a mesh on the group's
        // own node is in it already.
        if mesh.node != group {
            let into = math::mul(&to_group, &world[mesh.node]);
            for p in &mut mesh.positions {
                *p = math::point(&into, *p);
            }
            for n in &mut mesh.normals {
                *n = math::vector(&into, *n);
            }
        }
        for p in &mesh.positions {
            lo = std::array::from_fn(|k| lo[k].min(p[k]));
            hi = std::array::from_fn(|k| hi[k].max(p[k]));
        }
    }
    if lo[0] > hi[0] {
        return Err(format!("the prop {group_name:?} has no vertices"));
    }
    let middle: Vec3 = std::array::from_fn(|k| (lo[k] + hi[k]) * 0.5);
    let mut half: Vec3 = std::array::from_fn(|k| (hi[k] - lo[k]) * 0.5);
    let snap = |v: f32| (v / SHAPE_STEP).round() * SHAPE_STEP;
    for p in meshes.iter_mut().flat_map(|m| &mut m.positions) {
        *p = std::array::from_fn(|k| snap(p[k] - middle[k]));
        // The box is what the datablock's `size` says: it holds every point as snapped, so no hull pokes out of it.
        half = std::array::from_fn(|k| half[k].max(p[k].abs()));
    }
    let (position, rotation, _) = math::decompose(&math::mul(&world[group], &moved(middle)));
    let hulls = if kind.shaped() {
        meshes.iter().filter_map(bounds_of).collect()
    } else {
        Vec::new()
    };
    let shape = shape_model(model, merged(kind, meshes), &hulls);
    Ok(LevelProp {
        name,
        kind,
        position,
        rotation: math::normalize_quat(rotation),
        half,
        shape,
        hulls,
    })
}

/// An item's `meshes` (in its own frame) as one mesh named after its `kind`: their vertices end to end and one primitive a
/// material, so a prop is one object of its shape, drawn in one draw a material, and items of one kind and shape write the
/// same file.
fn merged(kind: PropKind, meshes: Vec<Mesh>) -> Mesh {
    let mut out = Mesh {
        name: kind.word().to_string(),
        detail: meshes.first().and_then(|m| m.detail),
        ..Mesh::default()
    };
    let mut by_material: Vec<Primitive> = Vec::new();
    for mesh in meshes {
        let (base, count) = (out.positions.len() as u32, mesh.positions.len());
        out.positions.extend(mesh.positions);
        // The writer's own stand-ins for a mesh without them, so the merged lists stay one a vertex.
        if mesh.normals.len() == count {
            out.normals.extend(mesh.normals);
        } else {
            out.normals
                .extend(std::iter::repeat_n([0.0, 0.0, 1.0], count));
        }
        if mesh.uv0.len() == count {
            out.uv0.extend(mesh.uv0);
        } else {
            out.uv0.extend(std::iter::repeat_n([0.0; 2], count));
        }
        for p in mesh.primitives {
            let at = match by_material.iter().position(|q| q.material == p.material) {
                Some(at) => at,
                None => {
                    by_material.push(Primitive {
                        material: p.material,
                        indices: Vec::new(),
                    });
                    by_material.len() - 1
                }
            };
            by_material[at]
                .indices
                .extend(p.indices.iter().map(|i| i + base));
        }
    }
    out.primitives = by_material;
    out
}

/// The box a mesh takes up (`[min, max]`), none for a mesh with no vertices.
fn bounds_of(mesh: &Mesh) -> Option<[Vec3; 2]> {
    let first = *mesh.positions.first()?;
    let (lo, hi) = mesh.positions.iter().fold((first, first), |(lo, hi), p| {
        (
            std::array::from_fn(|k| lo[k].min(p[k])),
            std::array::from_fn(|k| hi[k].max(p[k])),
        )
    });
    Some([lo, hi])
}

/// The collision mesh of the box `[lo, hi]`: its eight corners and twelve triangles, no material (a hull of a shape's
/// `Collision-1` detail, `detail`).
fn hull_mesh(detail: usize, [lo, hi]: [Vec3; 2]) -> Mesh {
    let corner =
        |i: u32| -> Vec3 { std::array::from_fn(|k| if i & (1 << k) == 0 { lo[k] } else { hi[k] }) };
    let quads: [[u32; 4]; 6] = [
        [0, 2, 3, 1],
        [4, 5, 7, 6],
        [0, 1, 5, 4],
        [2, 6, 7, 3],
        [0, 4, 6, 2],
        [1, 3, 7, 5],
    ];
    let indices = quads
        .iter()
        .flat_map(|&[a, b, c, d]| [a, b, c, a, c, d])
        .collect();
    Mesh {
        name: "Col-1".into(),
        detail: Some(detail),
        positions: (0..8).map(corner).collect(),
        normals: (0..8).map(|_| [0.0, 0.0, 1.0]).collect(),
        uv0: vec![[0.0; 2]; 8],
        primitives: vec![Primitive {
            material: None,
            indices,
        }],
        ..Mesh::default()
    }
}

/// A shape's model of an item's `mesh` (in its own frame, [`merged`]): one node, the model's detail levels, and the
/// materials it uses (no images: a shape names its textures by its materials' names, and the level's textures are in the kit
/// beside it), and a `Collision-1` detail with a hull for each of `hulls`.
fn shape_model(model: &Model, mut mesh: Mesh, hulls: &[[Vec3; 2]]) -> Model {
    let mut materials = Vec::new();
    let mut index: BTreeMap<usize, usize> = BTreeMap::new();
    for p in &mut mesh.primitives {
        p.material = p.material.and_then(|m| {
            let source = model.materials.get(m)?;
            Some(*index.entry(m).or_insert_with(|| {
                let mut material = source.clone();
                material.base_texture = None;
                material.normal_texture = None;
                material.metal_rough_texture = None;
                material.emissive_texture = None;
                material.unlit |= material.name.ends_with(GLOW);
                materials.push(material);
                materials.len() - 1
            }))
        });
    }
    let mut details = model.details.clone();
    let mut meshes = vec![mesh];
    if !hulls.is_empty() {
        details.push(Detail {
            name: "Collision-1".into(),
            size: -1.0,
        });
        let at = details.len() - 1;
        meshes.extend(hulls.iter().map(|&h| hull_mesh(at, h)));
    }
    Model {
        nodes: vec![Node::new(&meshes[0].name, None, [0.0; 3])],
        meshes,
        details,
        materials,
        images: Vec::new(),
        animations: Vec::new(),
        source: model.source.clone(),
        notes: Vec::new(),
    }
}

/// A level's name as a script's identifier: its letters, digits and underscores (`level` before a leading digit).
fn identifier(level: &str) -> String {
    let word: String = level
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if word.starts_with(|c: char| !c.is_ascii_alphabetic()) || word.is_empty() {
        format!("level{word}")
    } else {
        word
    }
}

/// `props` of level `level` built (module docs): their shapes written into the kit of `files`, and the script that takes the
/// map's [`PROPS`] line (the datablocks, then the spawns).
pub fn build(files: &mut GameFiles, level: &str, props: &[LevelProp]) -> Result<String, String> {
    let id = identifier(level);
    let mut script = String::new();
    let mut spawns = String::new();
    // Each shape built once a kind: its datablock's name.
    let mut datablocks: BTreeMap<(PropKind, Vec<u8>), String> = BTreeMap::new();
    for prop in props {
        let shape = dts::to_dts(&prop.shape).map_err(|e| format!("{level}: {}: {e}", prop.name))?;
        let bytes = torque::shapes::write::write_dts(&shape);
        let next = datablocks.len() + 1;
        let data = datablocks
            .entry((prop.kind, bytes))
            .or_insert_with_key(|(_, bytes)| {
                let file = format!("{id}_prop_{next}.dts");
                files.kit.insert(&file, bytes.clone());
                let size = prop.half.map(|h| h * 2.0);
                let name = format!("{id}Prop{next}");
                let _ = writeln!(
                    script,
                    "datablock PropData({name}) {{ shapeFile = \"kit/{file}\"; {}size = \"{} {} {}\"; mass = {}; \
                     friction = {FRICTION}; restitution = {RESTITUTION}; }};",
                    if prop.kind.shaped() {
                        "collision = \"shape\"; "
                    } else {
                        ""
                    },
                    size[0],
                    size[1],
                    size[2],
                    prop.kind.mass(size)
                );
                name
            });
        let [x, y, z] = prop.position;
        let [ax, ay, az, angle] = torque_plugin_props::server::axis_angle(prop.rotation);
        let _ = writeln!(
            spawns,
            "propSleep(propSpawn({data}, \"{x} {y} {z} {ax} {ay} {az} {angle}\"));"
        );
    }
    script.push_str(&spawns);
    Ok(script)
}

#[cfg(test)]
mod tests {
    use super::*;
    use torque_import::modelcore::Material;

    #[test]
    fn every_kind_reads_back_from_its_word_and_an_unknown_one_is_an_error() {
        for &k in PropKind::ALL {
            assert_eq!(k.word().parse(), Ok(k));
            assert!(k.mass([1.0; 3]) > 0.0);
        }
        assert!("sofabed".parse::<PropKind>().is_err());
        assert_eq!(
            kind_and_name("prop side_chair visitor chair a"),
            Ok((PropKind::SideChair, "visitor chair a".to_string()))
        );
        assert!(kind_and_name("prop stool a").is_err());
    }

    /// A box of `size` about `at` on node `node`, of material 0.
    fn cube(node: usize, at: Vec3, size: Vec3) -> Mesh {
        let positions = (0..8)
            .map(|c| {
                std::array::from_fn(|k| {
                    at[k] + if c & (1 << k) == 0 { -0.5 } else { 0.5 } * size[k]
                })
            })
            .collect();
        Mesh {
            name: "box".into(),
            node,
            detail: Some(0),
            positions,
            primitives: vec![Primitive {
                material: Some(0),
                indices: vec![0, 1, 2, 1, 3, 2],
            }],
            ..Mesh::default()
        }
    }

    /// A room group with a wall and a chair turned a quarter about z at (4, 0): the chair's group carries the turn.
    fn room() -> Model {
        let quarter = [
            0.0,
            0.0,
            std::f32::consts::FRAC_1_SQRT_2,
            std::f32::consts::FRAC_1_SQRT_2,
        ];
        let mut chair = Node::new("prop chair a chair", Some(1), [4.0, 0.0, 0.0]);
        chair.rotation = quarter;
        Model {
            nodes: vec![
                Node::new("root", None, [0.0; 3]),
                Node::new("room", Some(0), [0.0; 3]),
                chair,
            ],
            meshes: vec![
                cube(1, [0.0, 5.0, 1.5], [10.0, 0.2, 3.0]),
                // Its seat 0.6 along x and 0.4 along y of its own, its back.
                cube(2, [0.0, 0.0, 0.25], [0.6, 0.4, 0.5]),
                cube(2, [0.0, -0.25, 0.75], [0.6, 0.1, 0.5]),
            ],
            details: vec![Detail {
                name: "detail2".into(),
                size: 2.0,
            }],
            materials: vec![Material {
                name: "office_screen_glow".into(),
                ..Material::default()
            }],
            ..Model::default()
        }
    }

    #[test]
    fn an_item_is_cut_out_with_its_box_in_its_own_turned_frame() {
        let (building, props) = split(room()).unwrap();
        assert_eq!(building.meshes.len(), 1, "the wall stays");
        let [chair] = &props[..] else {
            panic!("{props:?}")
        };
        assert_eq!(
            (chair.kind, chair.name.as_str()),
            (PropKind::Chair, "a chair")
        );
        let near = |a: Vec3, b: Vec3| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5);
        assert!(near(chair.half, [0.3, 0.25, 0.5]), "{:?}", chair.half);
        // The box's middle is a twentieth back along the chair's own -y, turned to +x in the level.
        assert!(
            near(chair.position, [4.05, 0.0, 0.5]),
            "{:?}",
            chair.position
        );
        assert!(chair.rotation[2] > 0.7);
        // The shape is about its box's middle, and its glow material is drawn unlit.
        let (lo, hi) = chair.shape.meshes.iter().flat_map(|m| &m.positions).fold(
            ([f32::MAX; 3], [f32::MIN; 3]),
            |(lo, hi), p| {
                (
                    std::array::from_fn(|k| lo[k].min(p[k])),
                    std::array::from_fn(|k| hi[k].max(p[k])),
                )
            },
        );
        assert!(
            near(lo, [-0.3, -0.25, -0.5]) && near(hi, [0.3, 0.25, 0.5]),
            "{lo:?} {hi:?}"
        );
        assert!(chair.shape.materials[0].unlit);
        chair.shape.check().unwrap();
    }

    /// A shaped kind's item takes a collision hull for each mesh it was made of, and the datablock says so; a kind of boxes gets
    /// none.
    #[test]
    fn a_shaped_kind_gets_a_hull_for_each_mesh_and_says_so_in_its_datablock() {
        let (_, props) = split(room()).unwrap();
        let [chair] = &props[..] else {
            panic!("{props:?}")
        };
        assert!(chair.kind.shaped());
        assert_eq!(chair.hulls.len(), 2, "the seat and the back");
        let near = |a: Vec3, b: Vec3| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5);
        // The seat is 0.6 x 0.4 x 0.5 about the box's middle (the box is 0.5 deep, 1.0 tall, its middle a twentieth forward of the
        // seat's).
        assert!(
            near(chair.hulls[0][0], [-0.3, -0.15, -0.5]),
            "{:?}",
            chair.hulls[0]
        );
        assert!(
            near(chair.hulls[0][1], [0.3, 0.25, 0.0]),
            "{:?}",
            chair.hulls[0]
        );
        let at = chair
            .shape
            .details
            .iter()
            .position(|d| d.name == "Collision-1" && d.size < 0.0)
            .expect("a collision detail");
        let hulls: Vec<&Mesh> = chair
            .shape
            .meshes
            .iter()
            .filter(|m| m.detail == Some(at))
            .collect();
        assert_eq!(hulls.len(), 2);
        assert!(
            hulls
                .iter()
                .all(|m| m.positions.len() == 8 && m.triangles() == 12)
        );
        assert!(
            hulls
                .iter()
                .all(|m| m.primitives.iter().all(|p| p.material.is_none()))
        );
        chair.shape.check().unwrap();
        // The drawn mesh is the one it was, and the hulls add no drawn detail.
        assert_eq!(chair.shape.meshes[0].triangles(), 4);
        assert!(chair.shape.details[at].size < 0.0);
        let mut files = GameFiles::bundled().unwrap();
        let script = build(&mut files, "room", &props).unwrap();
        assert!(
            script.contains("collision = \"shape\"; size = "),
            "{script}"
        );
        // Every kind that is not shaped is a box.
        for &kind in PropKind::ALL {
            assert_eq!(
                kind.shaped(),
                matches!(
                    kind,
                    PropKind::Chair
                        | PropKind::SideChair
                        | PropKind::Armchair
                        | PropKind::Sofa
                        | PropKind::Desk
                        | PropKind::Table
                ),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn a_level_name_becomes_a_script_identifier() {
        assert_eq!(identifier("office"), "office");
        assert_eq!(identifier("my-office 2"), "myoffice2");
        assert_eq!(identifier("2nd"), "level2nd");
    }
}
