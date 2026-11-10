use super::*;

fn triangle() -> Draw {
    let v = |x: f32, y: f32| Vertex {
        position: [x, y, 0.0],
        normal: [0.0, 0.0, 1.0],
        color: [255; 4],
        uv: [0.0; 2],
    };
    Draw {
        label: "triangle".to_string(),
        vertices: vec![v(0.0, 0.0), v(1.0, 0.0), v(0.0, 1.0)],
        lighting: true,
        tex_env: TexEnv::None,
        texture: None,
        blend: Blend::Opaque,
        cull: Cull::BackCw,
        depth_test: true,
        depth_write: true,
        fog: true,
        lights: Vec::new(),
        vertex_key: None,
    }
}

fn at(p: [f32; 3]) -> Mat {
    let mut m = Mat::IDENTITY;
    m.set_column3(p);
    m
}

#[test]
fn a_batch_is_cut_into_chunks_by_where_its_copies_stand() {
    let batch = StaticBatch {
        draws: vec![triangle()],
        textures: Vec::new(),
        transforms: vec![
            at([1.0, 1.0, 0.0]),
            at([2.0, 3.0, 0.0]),
            at([BATCH_CHUNK + 1.0, 0.0, 0.0]),
            at([-1.0, 0.0, 0.0]),
            at([f32::NAN, 0.0, 0.0]),
        ],
    };
    let chunks = batch.chunks();
    assert_eq!(chunks.len(), 3, "{chunks:?}");
    assert!(chunks.contains(&vec![0, 1]));
    assert!(chunks.contains(&vec![2]));
    assert!(chunks.contains(&vec![3]));
    // The copies of a chunk are in the world: each of the mesh's draws once, every copy's triangles in it.
    let draws = batch.chunk_draws(&[0, 1]);
    assert_eq!(draws.len(), 1);
    assert_eq!(draws[0].vertices.len(), 6);
    assert_eq!(draws[0].vertices[3].position, [2.0, 3.0, 0.0]);
}

#[test]
fn a_full_chunk_is_cut_again() {
    let mut mesh = triangle();
    let one = mesh.vertices.clone();
    for _ in 1..1024 {
        mesh.vertices.extend_from_slice(&one);
    }
    let copies = 2 * MAX_CHUNK_TRIANGLES / 1024 + 3;
    let batch = StaticBatch {
        draws: vec![mesh],
        textures: Vec::new(),
        transforms: (0..copies)
            .map(|i| at([i as f32 * 0.01, 0.0, 0.0]))
            .collect(),
    };
    let chunks = batch.chunks();
    assert_eq!(chunks.len(), 3);
    assert_eq!(chunks.iter().map(Vec::len).sum::<usize>(), copies);
}

#[test]
fn a_copy_turned_turns_its_normals() {
    let turn = Mat::from_axis_angle([1.0, 0.0, 0.0], std::f32::consts::FRAC_PI_2);
    let batch = StaticBatch {
        draws: vec![triangle()],
        textures: Vec::new(),
        transforms: vec![turn],
    };
    let n = batch.chunk_draws(&[0])[0].vertices[0].normal;
    assert!(
        (n[0].abs() + (n[1].abs() - 1.0).abs() + n[2].abs()) < 1e-5,
        "{n:?}"
    );
}

struct Counter(Rc<std::cell::Cell<f32>>);

impl EmitterKind for Counter {
    fn advance(&mut self, seconds: f32) {
        self.0.set(self.0.get() + seconds);
    }

    fn draw(&mut self, _frame: &EmitterFrame, out: &mut EmitterDraws<'_, '_>) {
        let sprite = Sprite {
            position: [0.0, 10.0, 0.0],
            size: 1.0,
            spin: 0.0,
            color: [255, 128, 0, 255],
        };
        assert!(out.sprites(&[sprite, sprite], None, Blend::Additive));
    }
}

struct NoFiles;

impl torque_render_scene::Assets for NoFiles {
    fn read(&self, _path: &str) -> Option<Vec<u8>> {
        None
    }
}

#[test]
fn emitter_kinds_run_by_the_frame_step_and_draw_sorted_by_their_distance() {
    let time = Rc::new(std::cell::Cell::new(0.0));
    let mut kinds = EmitterKinds {
        kinds: vec![("count".to_string(), Box::new(Counter(Rc::clone(&time))))],
        fixed: Some(0.5),
        last: None,
        warned: false,
    };
    let frame = EmitterFrame {
        eye: [0.0; 3],
        right: [1.0, 0.0, 0.0],
        up: [0.0, 0.0, 1.0],
    };
    let assets = NoFiles;
    let mut set = TextureSet::new(&assets);
    let mut draws = Vec::new();
    kinds.frame(&frame, &mut set, &mut draws);
    // A step longer than the longest is cut to it.
    assert_eq!(time.get(), MAX_EMITTER_STEP);
    assert_eq!(draws.len(), 1);
    assert!((draws[0].0 - 10.0).abs() < 1e-4);
    assert_eq!(draws[0].1.vertices.len(), 12);
    assert_eq!(draws[0].1.blend, Blend::Additive);
}

#[test]
fn a_second_emitter_kind_of_a_name_and_adds_past_the_cap_are_left_out() {
    let kind = |name: &str| EmitterKindEntry {
        name: name.to_string(),
        make: Rc::new(|| Box::new(Counter(Rc::default()))),
    };
    let adds = RenderAdds {
        passes: Vec::new(),
        batches: Vec::new(),
        kinds: vec![kind("a"), kind("b"), kind("a")],
    };
    let mut warnings = Vec::new();
    let kept = adds.kept(|w| warnings.push(w));
    let names: Vec<&str> = kept.kinds.iter().map(EmitterKindEntry::name).collect();
    assert_eq!(names, ["a", "b"]);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
}

fn cube_mesh() -> Draw {
    let mut d = triangle();
    let one = d.vertices.clone();
    for _ in 1..12 {
        d.vertices.extend_from_slice(&one);
    }
    d
}

#[test]
fn a_million_copies_of_a_cube_lay_out_no_more_than_a_batch_s_triangles() {
    let batch = StaticBatch {
        draws: vec![cube_mesh()],
        textures: Vec::new(),
        transforms: (0..1_000_000)
            .map(|i| at([(i % 1000) as f32, (i / 1000) as f32, 0.0]))
            .collect(),
    };
    let layout = batch.layout(usize::MAX);
    let kept: usize = layout.chunks.iter().map(Vec::len).sum();
    assert_eq!(kept, MAX_BATCH_TRIANGLES / 12);
    assert_eq!(layout.triangles, kept * 12);
    assert!(layout.triangles <= MAX_BATCH_TRIANGLES);
    assert_eq!(layout.left_out, 1_000_000 - kept);
    // The copies kept are the first ones, in order.
    let mut first: Vec<usize> = layout.chunks.concat();
    first.sort_unstable();
    assert_eq!(first, (0..kept).collect::<Vec<_>>());
}

#[test]
fn a_batch_lays_out_within_the_budget_it_is_given() {
    let batch = StaticBatch {
        draws: vec![cube_mesh()],
        textures: Vec::new(),
        transforms: (0..100).map(|i| at([i as f32, 0.0, 0.0])).collect(),
    };
    let layout = batch.layout(12 * 30 + 5);
    assert_eq!(layout.triangles, 12 * 30);
    assert_eq!(layout.left_out, 70);
    // A mesh larger than the budget lays out nothing.
    let none = batch.layout(11);
    assert!(none.chunks.is_empty());
    assert_eq!((none.triangles, none.left_out), (0, 100));
}

struct Flood;

impl EmitterKind for Flood {
    fn advance(&mut self, _seconds: f32) {}

    fn draw(&mut self, _frame: &EmitterFrame, out: &mut EmitterDraws<'_, '_>) {
        let sprite = Sprite {
            position: [0.0, 10.0, 0.0],
            size: 1.0,
            spin: 0.0,
            color: [255; 4],
        };
        assert!(out.sprites(
            &vec![sprite; MAX_SPRITES_PER_CALL + 5],
            None,
            Blend::Additive
        ));
    }
}

#[test]
fn a_call_past_the_sprite_cap_draws_the_cap_and_warns_once() {
    let mut kinds = EmitterKinds {
        kinds: vec![("flood".to_string(), Box::new(Flood))],
        fixed: Some(0.0),
        last: None,
        warned: false,
    };
    let frame = EmitterFrame {
        eye: [0.0; 3],
        right: [1.0, 0.0, 0.0],
        up: [0.0, 0.0, 1.0],
    };
    let assets = NoFiles;
    let mut set = TextureSet::new(&assets);
    let mut draws = Vec::new();
    let warning = kinds.frame(&frame, &mut set, &mut draws);
    assert!(warning.is_some_and(|w| w.contains("flood") && w.contains(" 5 ")));
    assert_eq!(draws[0].1.vertices.len(), MAX_SPRITES_PER_CALL * 6);
    draws.clear();
    assert_eq!(kinds.frame(&frame, &mut set, &mut draws), None);
}
