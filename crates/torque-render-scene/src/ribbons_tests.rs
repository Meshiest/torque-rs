use super::*;
use crate::textures::tests::png;

struct Files(std::collections::HashMap<String, Vec<u8>>);

impl crate::Assets for Files {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.0.get(path).cloned()
    }
}

fn data() -> RibbonData {
    RibbonData {
        sizes: [0.1, 0.05, 0.0, 0.0],
        colors: [
            [1.0, 1.0, 1.0, 1.0],
            [0.5, 0.8, 1.0, 0.5],
            [0.2, 0.4, 1.0, 0.0],
            [0.0; 4],
        ],
        positions: [0.0, 0.5, 1.0, -1.0],
        lifetime_ms: 100.0,
        texture: Some("kit/spark".to_string()),
        alpha: false,
    }
}

#[test]
fn the_keys_run_from_the_head_to_the_tail() {
    let d = data();
    assert_eq!(d.at(0.0), (0.1, [1.0; 4]));
    let (size, color) = d.at(0.25);
    assert!((size - 0.075).abs() < 1e-6);
    assert!((color[3] - 0.75).abs() < 1e-6);
    let (size, color) = d.at(1.0);
    assert!(
        size.abs() < 1e-6 && color[3].abs() < 1e-6,
        "the tail fades to nothing"
    );
}

#[test]
fn points_age_away_and_the_head_stays_while_the_path_goes_on() {
    let mut r = Ribbon::new(data());
    for i in 0..5 {
        r.add_point([0.0, i as f32, 0.0]);
        r.advance(30.0);
    }
    // Ages 150, 120, 90, 60, 30 (newest first): the two past 100 ms are gone.
    assert_eq!(r.points().count(), 3);
    assert_eq!(r.points().next(), Some([0.0, 4.0, 0.0]));
    // A point closer than MIN_STEP moves the head.
    r.add_point([0.0, 4.001, 0.0]);
    assert_eq!(r.points().count(), 3);
    assert_eq!(r.points().next(), Some([0.0, 4.001, 0.0]));
    // Ended, it ages away whole.
    r.dead = true;
    r.add_point([0.0, 9.0, 0.0]);
    r.advance(200.0);
    assert!(r.is_empty());
}

#[test]
fn the_strip_faces_the_eye_and_narrows_and_fades_to_its_tail() {
    let mut files = std::collections::HashMap::new();
    files.insert("kit/spark.png".to_string(), png(8, 8, 255));
    let files = Files(files);
    let mut set = TextureSet::new(&files);
    let mut r = Ribbon::new(data());
    for i in 0..5 {
        r.add_point([0.0, i as f32, 0.0]);
        r.advance(15.0);
    }
    // The eye straight above the path: the strip lies flat across x.
    let d = r.draw([0.0, 2.0, 10.0], &mut set, "trail").unwrap();
    assert_eq!(d.blend, Blend::Additive);
    assert!(d.depth_test && !d.depth_write);
    assert_eq!(
        d.vertices.len(),
        4 * 6,
        "two triangles between each pair of points"
    );
    for v in &d.vertices {
        assert!(
            v.position[2].abs() < 1e-6,
            "across the path, square to the eye: {:?}",
            v.position
        );
    }
    // The head (y = 4, the newest) is the widest and brightest; the oldest the narrowest and faintest.
    let at = |y: f32| {
        d.vertices
            .iter()
            .filter(|v| (v.position[1] - y).abs() < 1e-6)
            .map(|v| (v.position[0].abs(), v.color[3]))
            .next()
            .unwrap()
    };
    let (head, tail) = (at(4.0), at(0.0));
    assert!(head.0 > tail.0 && head.1 > tail.1, "{head:?} {tail:?}");
    // Fewer than two points: nothing.
    assert!(
        Ribbon::new(data())
            .draw([0.0; 3], &mut set, "trail")
            .is_none()
    );
}

#[test]
fn a_trails_particle_data_makes_its_ribbon() {
    let p = ParticleData {
        drag: 0.0,
        wind: 0.0,
        gravity: 0.0,
        inherited_vel: 0.0,
        constant_accel: 0.0,
        lifetime_ms: 160,
        lifetime_variance_ms: 32,
        spin_speed: 0.0,
        spin_min: 0.0,
        spin_max: 0.0,
        inv_alpha: false,
        colors: data().colors,
        sizes: [0.2, 0.1, 0.0, 0.0],
        times: [0.0, 0.35, 1.0, 2.0],
        textures: vec!["kit/spark".to_string()],
        animate: false,
        fps: 0,
        extras: crate::particles::ParticleExtras::default(),
    };
    let r = RibbonData::of_particle(&p);
    assert_eq!(r.sizes, [0.1, 0.05, 0.0, 0.0], "half the particle's width");
    assert_eq!(r.positions, p.times);
    assert_eq!(r.lifetime_ms, 160.0);
    assert_eq!(r.texture.as_deref(), Some("kit/spark"));
    assert!(!r.alpha);
}
