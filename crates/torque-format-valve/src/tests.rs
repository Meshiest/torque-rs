//! The loaders through the engine's asset registry, over synthetic files.

use torque_engine::EngineBuilder;
use torque_engine::assets::Assets;

use crate::files::SourceFiles;
use crate::vmt::Vmt;
use crate::{Texture, ValveFormatPlugins, vpk, vtf};

#[test]
fn the_group_loads_textures_and_materials_from_a_vpk() {
    let engine = EngineBuilder::new()
        .add_plugins(ValveFormatPlugins)
        .build()
        .unwrap();
    let tex = vtf::write(0, 1, 1, 4, &[vec![10, 20, 30, 40]]);
    let mat = b"LightmappedGeneric { $basetexture concrete/wall }".to_vec();
    let dir = vpk::write_dir(
        &[
            ("materials/concrete/wall.vtf", &tex),
            ("materials/concrete/wall.vmt", &mat),
        ],
        2,
        8,
    );
    let parsed = vpk::VpkDir::parse(dir).unwrap();
    let mut files = SourceFiles::new();
    let mut held = Vec::new();
    for p in parsed.paths() {
        let e = *parsed.entry(p).unwrap();
        held.push((
            p.to_string(),
            parsed.read_with(&e, &mut |_, _, _| unreachable!()).unwrap(),
        ));
    }
    files.mount_memory_first("vpk", held);
    let assets = Assets::new(&engine);
    let m = assets
        .load::<Vmt>(&files, "materials/concrete/wall.vmt")
        .unwrap();
    let path = m.base_texture().unwrap();
    let t = assets.load::<Texture>(&files, &path).unwrap();
    assert_eq!(t.levels[0].rgba, vec![10, 20, 30, 40]);
    assert!(
        assets
            .load::<Texture>(&files, "materials/none.vtf")
            .is_err()
    );
}

#[test]
fn a_texture_reads_its_sprite_sheet() {
    let t = crate::read_texture(&crate::fuzz::sheet_texture_for_tests()).unwrap();
    assert_eq!(t.levels[0].rgba, vec![1, 2, 3, 4]);
    assert_eq!(t.sheet.unwrap().rect(0), [0.0, 0.0, 1.0, 1.0]);
}

#[test]
fn a_model_loads_each_include_once_and_never_itself() {
    let engine = EngineBuilder::new()
        .add_plugins(ValveFormatPlugins)
        .build()
        .unwrap();
    // 16 includes naming the model itself and 16 naming one missing model (in two spellings).
    let mut b = crate::synth::mdl(8);
    let own = b.len();
    b.extend_from_slice(b"Models\\X.mdl\0");
    let none = b.len();
    b.extend_from_slice(b"models/none.mdl\0");
    let table = b.len();
    for k in 0..32 {
        let at = table + k * 8;
        let to = if k % 2 == 0 { own } else { none };
        b.extend_from_slice(&0i32.to_le_bytes());
        b.extend_from_slice(&(to as i32 - at as i32).to_le_bytes());
    }
    b[336..340].copy_from_slice(&32i32.to_le_bytes());
    b[340..344].copy_from_slice(&(table as i32).to_le_bytes());
    let mut files = SourceFiles::new();
    files.mount_memory_first(
        "mem",
        vec![
            ("models/x.mdl".to_string(), b),
            ("models/x.vvd".to_string(), crate::synth::vvd()),
            (
                "models/x.dx90.vtx".to_string(),
                crate::synth::vtx(1, 3, false),
            ),
        ],
    );
    let assets = Assets::new(&engine);
    let m = crate::load_model(
        &assets,
        &files,
        "models/x.mdl",
        &crate::model::Choice::default(),
        &mut |_| true,
    )
    .unwrap();
    assert_eq!(m.model.animations.len(), 1);
    let notes: Vec<_> = m
        .model
        .notes
        .iter()
        .filter(|n| n.contains("included"))
        .collect();
    assert_eq!(notes.len(), 1, "{notes:?}");
}
