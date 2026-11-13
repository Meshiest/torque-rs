use super::*;
use crate::decals::{DecalData, DecalManager};
use crate::hd::HdScene;
use crate::textures::tests::png;
use crate::{Assets, Blend};

struct Files(std::collections::HashMap<String, Vec<u8>>);

impl Assets for Files {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.0.get(path).cloned()
    }
}

fn fields(list: &[(&str, u32, Value)]) -> Vec<(String, u32, Value)> {
    list.iter()
        .map(|(n, i, v)| (n.to_string(), *i, v.clone()))
        .collect()
}

fn read(f: &[(String, u32, Value)]) -> Option<DistortionData> {
    DistortionData::read(|name, i| {
        f.iter()
            .find(|(n, k, _)| n == name && *k == i)
            .map(|(_, _, v)| v)
    })
}

#[test]
fn a_datablocks_distortion_is_read_in_range_and_none_when_it_changes_nothing() {
    let f = fields(&[
        ("distortion", 0, Value::F32(1e9)),
        ("distortionEdge", 0, Value::F32(2.0)),
        ("distortionMap", 0, Value::Str(b"fx\\haze".to_vec())),
        ("distortionScroll", 1, Value::F32(0.5)),
        ("distortionTint", 3, Value::F32(0.25)),
        ("distortionOnly", 0, Value::Bool(true)),
    ]);
    let d = read(&f).expect("it bends");
    assert_eq!(d.look.strength, crate::hd::distortion::MAX_STRENGTH);
    assert_eq!(d.look.edge, 2.0);
    assert_eq!(d.look.scroll, [0.0, 0.5]);
    assert_eq!(d.look.tint, [1.0, 1.0, 1.0, 0.25]);
    assert_eq!(d.map.as_deref(), Some("fx/haze"));
    assert!(d.only);
    // Fields that do not bend, a tint alone among them: none (a datablock of a server on Torque's protocol has none at all).
    assert_eq!(
        read(&fields(&[("distortionEdge", 0, Value::F32(3.0))])),
        None
    );
    assert_eq!(
        read(&fields(&[("distortionTint", 3, Value::F32(1.0))])),
        None
    );
    assert_eq!(read(&[]), None);
}

#[test]
fn a_bending_decal_is_a_distorter_that_fades_with_it_and_one_that_only_bends_draws_no_texture() {
    let mut files = std::collections::HashMap::new();
    files.insert("kit/hole.png".to_string(), png(8, 8, 30));
    files.insert("kit/haze.png".to_string(), png(4, 4, 128));
    let files = Files(files);
    let mut set = TextureSet::new(&files);
    let bend = |only| DecalData {
        size: [2.0, 1.0],
        texture: Some("kit/hole".to_string()),
        blend: Blend::Alpha,
        distortion: read(&fields(&[
            ("distortion", 0, Value::F32(0.3)),
            ("distortionMap", 0, Value::Str(b"kit/haze".to_vec())),
            ("distortionOnly", 0, Value::Bool(only)),
        ])),
    };
    let mut m = DecalManager::new(8, 10.0, 2.0);
    m.add(&bend(false), [0.0, 5.0, 1.0], [0.0, -1.0, 0.0], 0.0);
    m.add(&bend(true), [3.0, 5.0, 1.0], [0.0, -1.0, 0.0], 0.0);
    m.advance(9.0);
    let eye = [0.0, 0.0, 1.0];
    assert_eq!(
        m.draws(eye, &mut set).len(),
        1,
        "the one that only bends draws no texture"
    );
    let d = m.distorters(eye, |_, _| true, &mut set);
    assert_eq!(d.len(), 2);
    for d in &d {
        assert_eq!(d.draw.vertices.len(), 6);
        assert!(
            d.draw.vertices.iter().all(|v| v.color[3] == 128),
            "half faded"
        );
        assert!(d.look.map.is_some(), "its map found");
        assert!(d.is_drawn());
    }
    // In a scene its map is the scene's table's.
    let mut scene = HdScene::new(());
    let _ = scene.textures.intern(crate::Texture {
        format: crate::TexelFormat::Rgba8,
        path: "other".into(),
        width: 1,
        height: 1,
        levels: std::sync::Arc::new(vec![vec![0; 4]]),
    });
    scene.set_distorters(d, set);
    let table = scene.distorters[0].look.map.expect("a map").0;
    assert_eq!(scene.textures.textures[table].path, "kit/haze.png");
}

/// Only the bending decals the view may show are copied, the nearest first, and never more than the frame's distortion budget.
#[test]
fn the_bending_decals_are_culled_to_the_view_and_capped_nearest_first() {
    use crate::hd::distortion::MAX_DISTORTERS;
    let files = Files(std::collections::HashMap::new());
    let mut set = TextureSet::new(&files);
    let bend = DecalData {
        size: [1.0, 1.0],
        texture: None,
        blend: Blend::Alpha,
        distortion: read(&fields(&[("distortion", 0, Value::F32(0.3))])),
    };
    let count = MAX_DISTORTERS + 50;
    let mut m = DecalManager::new(count, 10.0, 2.0);
    for i in 0..count {
        m.add(&bend, [0.0, 5.0 + i as f32, 1.0], [0.0, -1.0, 0.0], 0.0);
    }
    let eye = [0.0, 0.0, 1.0];
    let all = m.distorters(eye, |_, _| true, &mut set);
    assert_eq!(all.len(), MAX_DISTORTERS, "held to the budget");
    let nearest = |d: &HdDistorter| d.centre[1];
    assert!(
        all.windows(2).all(|w| nearest(&w[0]) <= nearest(&w[1])),
        "the nearest first"
    );
    assert!(
        all.iter()
            .all(|d| d.centre[1] < 5.0 + MAX_DISTORTERS as f32)
    );
    // A view that shows those past y = 100 only.
    let seen = m.distorters(eye, |c, r| c[1] + r > 100.0, &mut set);
    assert!(!seen.is_empty());
    assert!(
        seen.iter().all(|d| d.centre[1] > 99.0),
        "only those in the view"
    );
}
