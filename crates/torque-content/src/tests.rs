use std::rc::Rc;

use torque_engine::assets::{AssetError, Assets, Origin};
use torque_engine::{Engine, EngineBuilder, PluginGroup, PluginType};
use torque_import::modelcore::Model;
use torque_import::{Options, caps, dts, obj};
use torque_mapcore::Importer;
use torque_mapcore::collision::Heightfield;
use torque_maps::legacy::IMPORTER;
use torque_maps::synth::{self, DifSpec};
use torque_maps::{dif, dml, ter};
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::Vm;

use super::*;

fn free(_: u64) -> bool {
    true
}

/// A crate of one box with a box hull, as a `.glb`.
#[cfg(feature = "gltf")]
fn crate_glb() -> Vec<u8> {
    use torque_shape_builder::{FULL_UV, MeshData, ShapeBuilder, material_flags};
    let mut b = ShapeBuilder::new();
    let node = b.node("crate", None, [0.0; 3]);
    let drawn = b.detail("detail1", 1.0);
    let collision = b.detail("Collision-1", -1.0);
    let wood = b.material("wood", material_flags::S_WRAP | material_flags::T_WRAP);
    let (min, max) = ([-0.5; 3], [0.5; 3]);
    let mut mesh = MeshData::new(Some(wood));
    mesh.add_box(min, max, |_| FULL_UV);
    b.object("crate", node, drawn, mesh);
    b.object("Col-1", node, collision, MeshData::hull_box(min, max));
    let model = dts::from_dts(&b.build()).unwrap();
    torque_import::gltf::write_glb(&model).unwrap()
}

/// A quad and a triangle in two groups, with a material whose `.mtl` is not read.
const OBJ: &[u8] = b"mtllib side.mtl\no floor\nv 0 0 0\nv 1 0 0\nv 1 0 -1\nv 0 0 -1\nvt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\n\
    usemtl stone\nf 1/1 2/2 3/3 4/4\ng post\nv 0 1 0\nf 1 2 5\n";

/// A Blockbench file of one cube a block across.
const BBMODEL: &str = r#"{"meta":{"format_version":"4.10","model_format":"free"},"resolution":{"width":16,"height":16},
"elements":[{"name":"box","type":"cube","uuid":"u-box","from":[0,0,0],"to":[16,16,16],"origin":[0,0,0],
"faces":{"north":{"uv":[0,0,16,16]},"east":{"uv":[0,0,16,16]},"south":{"uv":[0,0,16,16]},"west":{"uv":[0,0,16,16]},
"up":{"uv":[0,0,8,8]},"down":{"uv":[8,8,16,16]}}}],"outliner":["u-box"],"textures":[]}"#;

fn small_dif() -> Vec<u8> {
    synth::dif(DifSpec {
        version: 0,
        plain_surfaces: true,
        everything: true,
        border: 0,
    })
}

fn small_ter() -> Vec<u8> {
    synth::ter(3, &["grass", "rock"])
}

const DML: &[u8] = b"sky_front\nsky_right\nsky_back\nsky_left\nsky_top\nsky_bottom\nclouds/a\n";

/// The fixtures, one a format, at their paths.
fn fixtures() -> MemFiles {
    let files = MemFiles::new();
    #[cfg(feature = "gltf")]
    files.insert("shapes/crate.glb", &crate_glb());
    files.insert("shapes/floor.obj", OBJ);
    files.insert("shapes/block.bbmodel", BBMODEL.as_bytes());
    files.insert("interiors/box.dif", &small_dif());
    files.insert("terrains/hills.ter", &small_ter());
    files.insert("skies/day.dml", DML);
    files
}

/// An engine of `plugins` and a script engine over `files` with its asset cache.
fn engine_with<M>(
    plugins: impl torque_engine::Plugins<M>,
    files: MemFiles,
) -> (Engine, Vm, Rc<Assets>) {
    let engine = EngineBuilder::new().add_plugins(plugins).build().unwrap();
    let mut vm = Vm::new();
    vm.set_file_host(files);
    let assets = engine.assets(&mut vm);
    (engine, vm, assets)
}

/// Every format turned on.
fn every_format() -> PluginGroupBuilder {
    let group = ContentFormatPlugins.enable::<ObjFormatPlugin>();
    #[cfg(feature = "gltf")]
    let group = group.enable::<BlockbenchFormatPlugin>();
    #[cfg(feature = "map-builder")]
    let group = group.enable::<QuakeMapFormatPlugin>();
    group
}

#[test]
fn the_group_has_every_format_and_turns_on_those_the_engine_reads_today() {
    let group = ContentFormatPlugins.build();
    let ids: Vec<&str> = group.ids().map(|id| id.as_str()).collect();
    let mut want = Vec::new();
    if cfg!(feature = "gltf") {
        want.push("torque.format-gltf");
    }
    want.push("torque.format-obj");
    if cfg!(feature = "gltf") {
        want.push("torque.format-blockbench");
    }
    want.extend([
        "torque.format-dif",
        "torque.format-ter",
        "torque.format-map-layer",
    ]);
    if cfg!(feature = "map-builder") {
        want.push("torque.format-quake-map");
    }
    assert_eq!(ids, want);
    let engine = EngineBuilder::new()
        .add_plugins(ContentFormatPlugins)
        .build()
        .unwrap();
    let mut on = Vec::new();
    #[cfg(feature = "gltf")]
    on.push(GltfFormatPlugin::ID);
    on.extend([DifFormatPlugin::ID, TerFormatPlugin::ID, MapLayerPlugin::ID]);
    assert_eq!(engine.plugins(), on);
    assert!(engine.warnings().is_empty(), "{:?}", engine.warnings());
}

#[test]
fn a_format_turns_off_and_on_by_its_id_as_a_game_toml_does() {
    let engine = EngineBuilder::new()
        .add_plugins(ContentFormatPlugins)
        .disable_id("torque.format-dif")
        .enable_id("torque.format-obj")
        .build()
        .unwrap();
    assert!(!engine.is_plugin_added::<DifFormatPlugin>());
    assert!(engine.is_plugin_added::<ObjFormatPlugin>());
    let mut vm = Vm::new();
    vm.set_file_host(fixtures());
    let assets = engine.assets(&mut vm);
    let error = assets
        .load::<dif::Dif>(vm.file_host(), "interiors/box.dif")
        .unwrap_err();
    assert!(matches!(error, AssetError::NoLoader { .. }), "{error}");
    assert!(
        assets
            .load::<ImportedShape>(vm.file_host(), "shapes/floor.obj")
            .is_ok()
    );
}

#[test]
fn every_loader_declares_its_cap_and_trust() {
    let (engine, _, _) = engine_with(every_format(), MemFiles::new());
    let mut seen: Vec<(String, u64, bool)> = engine
        .asset_loaders()
        .map(|l| {
            (
                l.extensions.join(","),
                l.spec.max_bytes,
                l.spec.untrusted_ok,
            )
        })
        .collect();
    seen.sort();
    let mut want = vec![
        ("dif".to_string(), torque_maps::MAX_DIF_BYTES as u64, true),
        ("dif".to_string(), torque_maps::MAX_DIF_BYTES as u64, true),
        ("dml".to_string(), torque_maps::MAX_DML_BYTES as u64, true),
        ("obj".to_string(), caps::MAX_FILE_BYTES as u64, false),
        ("ter".to_string(), torque_maps::MAX_TER_BYTES as u64, true),
        ("ter".to_string(), torque_maps::MAX_TER_BYTES as u64, true),
    ];
    if cfg!(feature = "gltf") {
        want.push(("bbmodel".into(), caps::MAX_FILE_BYTES as u64, false));
        want.push(("glb,gltf".into(), caps::MAX_NATIVE_FILE_BYTES as u64, true));
    }
    #[cfg(feature = "map-builder")]
    want.push((
        "map".into(),
        torque_interior_builder::map::MAX_MAP_BYTES as u64,
        false,
    ));
    want.sort();
    assert_eq!(seen, want);
}

#[cfg(feature = "gltf")]
#[test]
fn a_glb_through_the_registry_is_the_dts_a_datablock_reads() {
    let (_engine, vm, assets) = engine_with(ContentFormatPlugins, fixtures());
    let shape = assets
        .load::<ImportedShape>(vm.file_host(), "shapes/crate.glb")
        .unwrap();
    let today =
        torque_import::native::shape_file(b"shapes/crate.glb", crate_glb(), &mut free).unwrap();
    assert_eq!(shape.dts, today);
    let parsed = torque_shapes::dts::parse(&shape.dts, &mut free).unwrap();
    assert_eq!(parsed.shape.nodes.len(), 1);
    assert_eq!(parsed.shape.objects.len(), 2);
}

/// The shape `torque-convert` writes of `model`.
fn converted(model: &Model) -> Vec<u8> {
    torque_shapes::write::write_dts(&dts::to_dts(model).unwrap())
}

#[test]
fn an_obj_through_the_registry_is_the_dts_the_converter_writes() {
    let (_engine, vm, assets) = engine_with(every_format(), fixtures());
    let shape = assets
        .load::<ImportedShape>(vm.file_host(), "shapes/floor.obj")
        .unwrap();
    let model = obj::read(OBJ, &mut |_, _| None, &Options::default(), &mut free).unwrap();
    assert_eq!(model.meshes.len(), 3, "two groups and the box hull");
    assert_eq!(shape.dts, converted(&model));
}

#[cfg(feature = "gltf")]
#[test]
fn a_blockbench_model_through_the_registry_is_the_dts_the_converter_writes() {
    let (_engine, vm, assets) = engine_with(every_format(), fixtures());
    let shape = assets
        .load::<ImportedShape>(vm.file_host(), "shapes/block.bbmodel")
        .unwrap();
    let model =
        torque_import::bbmodel::read(BBMODEL.as_bytes(), &Options::default(), &mut free).unwrap();
    assert_eq!(model.meshes[0].primitives[0].indices.len(), 36);
    assert_eq!(shape.dts, converted(&model));
}

#[test]
fn a_dif_through_the_registry_is_the_interior_the_server_reads() {
    let (_engine, vm, assets) = engine_with(ContentFormatPlugins, fixtures());
    let interior = assets
        .load::<dif::Dif>(vm.file_host(), "interiors/box.dif")
        .unwrap();
    let today = dif::parse(&small_dif()).unwrap();
    assert_eq!(*interior, today);
    assert_eq!(interior.details.len(), today.details.len());
    assert!(!interior.sub_objects.is_empty());
}

#[test]
fn a_ter_through_the_registry_is_the_terrain_the_server_reads() {
    let (_engine, vm, assets) = engine_with(ContentFormatPlugins, fixtures());
    let terrain = assets
        .load::<ter::Terrain>(vm.file_host(), "terrains/hills.ter")
        .unwrap();
    assert_eq!(*terrain, ter::parse(&small_ter()).unwrap());
    assert_eq!(terrain.materials.len(), 2);
    assert_eq!(terrain.materials[1].name, "rock");
}

#[test]
fn the_map_layer_through_the_registry_is_what_the_legacy_importer_makes() {
    let (_engine, vm, assets) = engine_with(ContentFormatPlugins, fixtures());
    let files = vm.file_host();
    let model = assets.load::<MapModel>(files, "interiors/box.dif").unwrap();
    let bytes = small_dif();
    assert_eq!(model.surfaces, IMPORTER.surfaces(&bytes).unwrap());
    assert_eq!(model.convexes, IMPORTER.convexes(&bytes).unwrap());
    assert_eq!(
        model.named_files,
        IMPORTER
            .named_files(torque_mapcore::FileKind::Model, &bytes)
            .unwrap()
    );
    assert!(!model.convexes.hulls.is_empty());
    let field = assets
        .load::<Heightfield>(files, "terrains/hills.ter")
        .unwrap();
    assert_eq!(*field, IMPORTER.heightfield(&small_ter()).unwrap());
    let sky = assets.load::<SkyList>(files, "skies/day.dml").unwrap();
    assert_eq!(sky.0, dml::parse(DML).unwrap());
    assert_eq!(sky.0.len(), 7);
    assert_eq!(sky.0[6], "a");
    // The `.dif` is cached once a type: the file as data and as the map layer.
    assert!(assets.load::<dif::Dif>(files, "interiors/box.dif").is_ok());
    assert_eq!(assets.len(), 4);
}

#[cfg(feature = "map-builder")]
#[test]
fn a_quake_map_through_the_registry_is_the_interior_the_converter_builds() {
    const CUBE: &str = r#"{
"classname" "worldspawn"
{
( -64 -64 -16 ) ( -64 -63 -16 ) ( -64 -64 -15 ) rock 0 0 0 1 1
( -64 -64 -16 ) ( -64 -64 -15 ) ( -63 -64 -16 ) rock 0 0 0 1 1
( -64 -64 -16 ) ( -63 -64 -16 ) ( -64 -63 -16 ) rock 0 0 0 1 1
( 64 64 16 ) ( 64 65 16 ) ( 65 64 16 ) rock 0 0 0 1 1
( 64 64 16 ) ( 65 64 16 ) ( 64 64 17 ) rock 0 0 0 1 1
( 64 64 16 ) ( 64 64 17 ) ( 64 65 16 ) rock 0 0 0 1 1
}
}
{
"classname" "info_player_start"
"origin" "32 0 24"
}
"#;
    use torque_interior_builder::map::{self, MapOptions};
    let files = MemFiles::new();
    files.insert("maps/box.map", CUBE.as_bytes());
    let (_engine, vm, assets) = engine_with(every_format(), files);
    let built = assets
        .load::<BuiltInterior>(vm.file_host(), "maps/box.map")
        .unwrap();
    let parsed = map::parse(CUBE, &MapOptions::default()).unwrap();
    let today = torque_interior_builder::build(&parsed.brushes, &Default::default()).unwrap();
    assert_eq!(built.bytes, today.bytes().unwrap());
    assert_eq!(built.objects, parsed.objects);
    assert_eq!(built.objects.len(), 1);
    assert_eq!(built.built.dif.details[0].hulls.hulls.len(), 1);
    // The texture size was not known: a warning, as the converter prints.
    assert_eq!(built.warnings, parsed.warnings);
}

#[test]
fn a_file_over_its_loaders_cap_is_not_read() {
    let files = MemFiles::new();
    files.insert(
        "skies/long.dml",
        &vec![b'a'; torque_maps::MAX_DML_BYTES + 1],
    );
    #[cfg(feature = "gltf")]
    files.insert("shapes/huge.glb", &vec![0; caps::MAX_NATIVE_FILE_BYTES + 1]);
    let (_engine, vm, assets) = engine_with(ContentFormatPlugins, files);
    let error = assets
        .load::<SkyList>(vm.file_host(), "skies/long.dml")
        .unwrap_err();
    assert_eq!(
        error,
        AssetError::OverCap {
            path: "skies/long.dml".into(),
            len: torque_maps::MAX_DML_BYTES as u64 + 1,
            max: torque_maps::MAX_DML_BYTES as u64,
        }
    );
    #[cfg(feature = "gltf")]
    {
        let error = assets
            .load::<ImportedShape>(vm.file_host(), "shapes/huge.glb")
            .unwrap_err();
        assert!(matches!(error, AssetError::OverCap { .. }), "{error}");
    }
    assert_eq!(assets.reads(), 0);
}

#[test]
fn untrusted_files_reach_only_the_hardened_loaders() {
    let (_engine, vm, assets) = engine_with(every_format(), fixtures());
    assets.set_origins(|_| Origin::Untrusted);
    let files = vm.file_host();
    let refused = |path: &str, error: AssetError| {
        assert_eq!(
            error,
            AssetError::Untrusted {
                path: path.to_owned()
            }
        );
    };
    refused(
        "shapes/floor.obj",
        assets
            .load::<ImportedShape>(files, "shapes/floor.obj")
            .unwrap_err(),
    );
    #[cfg(feature = "gltf")]
    {
        refused(
            "shapes/block.bbmodel",
            assets
                .load::<ImportedShape>(files, "shapes/block.bbmodel")
                .unwrap_err(),
        );
        assert!(
            assets
                .load::<ImportedShape>(files, "shapes/crate.glb")
                .is_ok()
        );
    }
    #[cfg(feature = "map-builder")]
    refused(
        "maps/none.map",
        assets
            .load::<BuiltInterior>(files, "maps/none.map")
            .unwrap_err(),
    );
    assert!(assets.load::<dif::Dif>(files, "interiors/box.dif").is_ok());
    assert!(assets.load::<MapModel>(files, "interiors/box.dif").is_ok());
    assert!(
        assets
            .load::<ter::Terrain>(files, "terrains/hills.ter")
            .is_ok()
    );
    assert!(
        assets
            .load::<Heightfield>(files, "terrains/hills.ter")
            .is_ok()
    );
    assert!(assets.load::<SkyList>(files, "skies/day.dml").is_ok());
}

/// A small deterministic generator for the fuzz.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// `seed` cut short, with bytes flipped and words overwritten by large counts.
fn mutate(seed: &[u8], rng: &mut Rng) -> Vec<u8> {
    let mut b = seed.to_vec();
    match rng.below(4) {
        0 => b.truncate(rng.below(b.len())),
        1 => {
            for _ in 0..1 + rng.below(8) {
                let at = rng.below(b.len());
                b[at] ^= 1 << rng.below(8);
            }
        }
        2 => {
            let at = rng.below(b.len().saturating_sub(4));
            if at + 4 <= b.len() {
                b[at..at + 4].copy_from_slice(&[0xff, 0xff, 0xff, 0x7f]);
            }
        }
        _ => {
            let at = rng.below(b.len());
            b.insert(at, rng.next() as u8);
        }
    }
    b
}

/// The fuzz of the loaders untrusted files reach: mutated fixtures loaded through the registry, each a value or an error and
/// never a loader's panic (which the registry would report as "its loader panicked"). The case count comes from
/// `TORQUE_CONTENT_FUZZ_CASES` (default 600).
#[test]
fn the_untrusted_loaders_never_panic_on_mutated_files() {
    let cases: usize = std::env::var("TORQUE_CONTENT_FUZZ_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(600);
    let seeds: Vec<(&str, Vec<u8>)> = vec![
        ("dif", small_dif()),
        ("ter", small_ter()),
        ("dml", DML.to_vec()),
    ];
    #[cfg(feature = "gltf")]
    let seeds = [seeds, vec![("glb", crate_glb())]].concat();
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let (_engine, _vm, assets) = engine_with(ContentFormatPlugins, MemFiles::new());
    assets.set_origins(|_| Origin::Untrusted);
    let mut loaded = 0;
    for case in 0..cases {
        let (ext, seed) = &seeds[case % seeds.len()];
        let bytes = mutate(seed, &mut rng);
        let files = MemFiles::new();
        let path = format!("fuzz/{case}.{ext}");
        files.insert(&path, &bytes);
        let results: Vec<Result<(), AssetError>> = match *ext {
            "dif" => vec![
                assets.load::<dif::Dif>(&files, &path).map(drop),
                assets.load::<MapModel>(&files, &path).map(drop),
            ],
            "ter" => vec![
                assets.load::<ter::Terrain>(&files, &path).map(drop),
                assets.load::<Heightfield>(&files, &path).map(drop),
            ],
            "dml" => vec![assets.load::<SkyList>(&files, &path).map(drop)],
            _ => vec![assets.load::<ImportedShape>(&files, &path).map(drop)],
        };
        for result in results {
            match result {
                Ok(()) => loaded += 1,
                Err(AssetError::Parse { message, .. }) => {
                    assert_ne!(message, "its loader panicked", "{path}");
                }
                Err(other) => panic!("{path}: {other}"),
            }
        }
        assets.clear();
    }
    assert!(loaded > 0, "some mutated files still read");
}
