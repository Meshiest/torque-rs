//! The fuzz: synthetic `.dts` and `.dsq` files made by the writer ([`crate::synth`], never a shape from disk), damaged by
//! [`crate::synth::mutate`] (bit flips, truncation, inflated counts, indices out of range, 2^31 sizes), and pushed through the reader, the writer, the semantic check,
//! the preload facts and the sequence import.
//!
//! What a case must do, whatever the damage: **not panic**; refuse or accept; if it is accepted and its guard words are right, the
//! writer, fed the model only, gives the very bytes that were read (the structural round trip holds for any file the reader
//! accepts, not only exporter-made ones); whatever is accepted writes to bytes that read to the same bytes again; the budget the
//! parse asks for stays proportional to the file; a refused import leaves the shape as it was. Over the whole run, the largest
//! request made through the one allocation helper ([`crate::largest_request`]) must not pass [`crate::MAX_ALLOC_BYTES`].
//!
//! The three named cases run first, each as one file or one loop: **mesh cloning** (thousands of meshes that
//! share one huge parent), **frame products** (counts whose product overflows), **sequence growth** (the same `.dsq` imported into
//! one kept shape until the caps stop it).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::collide::{Collision, box_planes};
use crate::dsq;
use crate::dts;
use crate::dts::MAX_KEYFRAMES;
use crate::math;
use crate::preload::{SLOTS, Slots, collision_slots};
use crate::shape::{Mesh, Primitive, Shape};
use crate::synth::{self, Rng};
use crate::write::{write_dsq, write_dts};
use crate::{
    MAX_ALLOC_BYTES, MAX_SEQUENCES, ShapeError, bounds, largest_request, take_request_mark,
};

/// How many cases of each kind, and the seed they derive from.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub dts: usize,
    pub dsq: usize,
    /// The collision queries (rays and boxes) to ask of generated hostile meshes (`hostile_mesh_case`): the pass makes shapes
    /// until it has asked this many.
    pub queries: usize,
    pub seed: u64,
}

/// A case that takes longer than this (wall clock) counts as a hang. Every loop here is charged, so a case takes milliseconds;
/// the limit is far above any loaded machine's reading.
pub const CASE_TIME_LIMIT: Duration = Duration::from_secs(20);

/// What a run saw.
#[derive(Debug, Default, Clone)]
pub struct Summary {
    pub dts_cases: usize,
    pub dts_parsed: usize,
    pub dts_refused: usize,
    /// Accepted with right guard words, and written back to the very bytes.
    pub dts_exact: usize,
    /// Accepted with a wrong guard word (the writer puts the right one back): checked for idempotence only.
    pub dts_wrong_guards: usize,
    pub dsq_cases: usize,
    pub dsq_parsed: usize,
    pub dsq_refused: usize,
    pub dsq_exact: usize,
    pub imports_ok: usize,
    pub imports_refused: usize,
    /// The collision queries: shapes whose collision details were prepared and refused, the rays and boxes asked, how many
    /// hit, and the queries a budget that ran out ended.
    pub collision_built: usize,
    pub collision_refused: usize,
    pub collision_rays: usize,
    pub collision_boxes: usize,
    pub collision_hits: usize,
    pub collision_ended: usize,
    /// The hostile-mesh pass: shapes made and edited (`hostile_mesh_case`), the queries asked of them (rays and boxes), how
    /// many hit, how many a budget ended, and the builds refused.
    pub hostile_shapes: usize,
    pub hostile_refused: usize,
    pub hostile_queries: usize,
    pub hostile_hits: usize,
    pub hostile_ended: usize,
    /// The longest any one case took, and the cases over [`CASE_TIME_LIMIT`].
    pub longest_case: Duration,
    pub hangs: usize,
    /// The three named cases, each refused (or capped) as designed.
    pub cloning_refused: bool,
    pub frame_products_refused: usize,
    pub frame_products_tried: usize,
    pub growth_imports_ok: usize,
    pub growth_stopped_at: usize,
    pub keyframe_growth_stopped: bool,
    pub largest_request: usize,
    pub max_units_per_byte: f64,
    /// The largest request of one parse against the size of its file (bytes asked per byte of file).
    pub largest_per_file_ratio: f64,
    /// Cases that panicked, and that broke an invariant; the first of each with its seed.
    pub panics: usize,
    pub failures: usize,
    pub first_failure: Option<String>,
}

impl Summary {
    fn fail(&mut self, what: String) {
        self.failures += 1;
        if self.first_failure.is_none() {
            self.first_failure = Some(what);
        }
    }

    /// The one-line summary of the run.
    pub fn line(&self) -> String {
        format!(
            "shapes fuzz: dts {} cases ({} read, {} refused; {} re-encoded to the same bytes, {} with a wrong guard), dsq {} cases ({} read, {} refused, {} re-encoded to the same bytes; {} imports made, {} refused); collision {} prepared, {} refused, {} rays and {} boxes ({} hit, {} ended by their budget); hostile meshes {} made ({} refused), {} queries ({} hit, {} ended by their budget); named cases: mesh cloning {}, frame products {} of {} refused, sequence growth stopped at {} sequences after {} imports, keyframe growth {}; largest allocation request {} bytes (cap {}), most work asked {:.1} units a byte, largest request {:.1} bytes per byte of file; longest case {} ms ({} over {} s); {} panics, {} broken invariants",
            self.dts_cases,
            self.dts_parsed,
            self.dts_refused,
            self.dts_exact,
            self.dts_wrong_guards,
            self.dsq_cases,
            self.dsq_parsed,
            self.dsq_refused,
            self.dsq_exact,
            self.imports_ok,
            self.imports_refused,
            self.collision_built,
            self.collision_refused,
            self.collision_rays,
            self.collision_boxes,
            self.collision_hits,
            self.collision_ended,
            self.hostile_shapes,
            self.hostile_refused,
            self.hostile_queries,
            self.hostile_hits,
            self.hostile_ended,
            if self.cloning_refused {
                "refused"
            } else {
                "NOT REFUSED"
            },
            self.frame_products_refused,
            self.frame_products_tried,
            self.growth_stopped_at,
            self.growth_imports_ok,
            if self.keyframe_growth_stopped {
                "stopped"
            } else {
                "NOT STOPPED"
            },
            self.largest_request,
            MAX_ALLOC_BYTES,
            self.max_units_per_byte,
            self.largest_per_file_ratio,
            self.longest_case.as_millis(),
            self.hangs,
            CASE_TIME_LIMIT.as_secs(),
            self.panics,
            self.failures,
        )
    }

    /// True when nothing went wrong: no panic, no broken invariant, every named case held, the allocation cap held.
    pub fn clean(&self) -> bool {
        self.panics == 0
            && self.failures == 0
            && self.hangs == 0
            && self.cloning_refused
            && self.frame_products_refused == self.frame_products_tried
            && self.keyframe_growth_stopped
            && self.growth_stopped_at <= MAX_SEQUENCES
            && self.growth_stopped_at > MAX_SEQUENCES - 100
            && self.largest_request <= MAX_ALLOC_BYTES
    }
}

fn free(_: u64) -> bool {
    true
}

/// Two shapes are the same by what they print (a derived `==` would call a shape with a NaN in it different from itself).
fn same_shape(a: &Shape, b: &Shape) -> bool {
    format!("{a:?}") == format!("{b:?}")
}

/// A parse of a file of `len` bytes asks the allocation helper for at most `64 * len + 4096` bytes in any one request: a count in
/// the file never sizes an allocation the file's bytes could not back (an item in memory is at most 24 times the bytes it takes
/// in the file, a name's `Vec` against its one byte).
fn check_requests(s: &mut Summary, kind: &str, seed: u64, len: usize) {
    let mark = take_request_mark();
    s.largest_per_file_ratio = s
        .largest_per_file_ratio
        .max(mark as f64 / len.max(1) as f64);
    if mark > 64 * len + 4096 {
        s.fail(format!(
            "{kind} seed {seed:#x}: one request asked for {mark} bytes for a file of {len}"
        ));
    }
}

/// Runs one case under the panic net and the clock: a panic is counted with its seed, a case over [`CASE_TIME_LIMIT`] is a hang.
fn guarded(s: &mut Summary, what: &str, seed: u64, case: impl FnOnce(&mut Summary)) {
    let start = Instant::now();
    let caught = catch_unwind(AssertUnwindSafe(|| case(s)));
    let took = start.elapsed();
    s.longest_case = s.longest_case.max(took);
    if took > CASE_TIME_LIMIT {
        s.hangs += 1;
        s.fail(format!(
            "{what} seed {seed:#x} took {} ms (limit {} s)",
            took.as_millis(),
            CASE_TIME_LIMIT.as_secs()
        ));
    }
    if caught.is_err() {
        s.panics += 1;
        if s.first_failure.is_none() {
            s.first_failure = Some(format!("{what} seed {seed:#x} panicked"));
        }
    }
}

/// Runs the cases named by `config`.
pub fn run(config: Config) -> Summary {
    let mut s = Summary::default();
    named_cases(&mut s);
    for i in 0..config.dts {
        let seed = config.seed ^ (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        s.dts_cases += 1;
        guarded(&mut s, "dts", seed, |s| dts_case(seed, s));
    }
    for i in 0..config.dsq {
        let seed = config.seed ^ 0xd5c ^ (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        s.dsq_cases += 1;
        guarded(&mut s, "dsq", seed, |s| dsq_case(seed, s));
    }
    // The hostile meshes: shapes made from hand-edited models (the reader would refuse most of them), asked rays and boxes until
    // the wanted number of queries was asked. A pass that cannot make enough shapes stops after a bounded number of tries; its
    // count then falls short and the caller says so.
    let mut i = 0u64;
    while s.hostile_queries < config.queries && i < 4 * config.queries as u64 + 1_000 {
        let seed = config.seed ^ 0x4057_11e5 ^ i.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        i += 1;
        guarded(&mut s, "hostile mesh", seed, |s| hostile_mesh_case(seed, s));
    }
    s.largest_request = largest_request();
    s
}

fn dts_case(seed: u64, s: &mut Summary) {
    let mut rng = Rng(seed);
    let consistent = rng.one_in(2);
    let size = 1 + rng.below(3);
    let model = synth::model(&mut rng, consistent, size);
    let original = write_dts(&model);
    let bytes = if rng.one_in(8) {
        original
    } else {
        synth::mutate(&mut rng, &original)
    };

    let mut units = 0u64;
    take_request_mark();
    let result = dts::parse_with_info(&bytes, &mut |u| {
        units += u;
        true
    });
    check_requests(s, "dts", seed, bytes.len());
    let ratio = units as f64 / bytes.len().max(1) as f64;
    if units > 8 * bytes.len() as u64 + 4096 {
        s.fail(format!(
            "dts seed {seed:#x}: parse asked for {units} units for {} bytes",
            bytes.len()
        ));
    }
    s.max_units_per_byte = s.max_units_per_byte.max(ratio.min(1e9));
    // A budget that runs out part-way must end the parse with an error, never a partial shape.
    if rng.one_in(4) {
        let mut calls = 0;
        let limit = 1 + rng.below(6);
        let r = dts::parse(&bytes, &mut |_| {
            calls += 1;
            calls <= limit
        });
        if r.is_ok() && calls > limit {
            s.fail(format!(
                "dts seed {seed:#x}: a refused charge still gave a shape"
            ));
        }
    }
    let (model, info) = match result {
        Ok(p) => p,
        Err(_) => {
            s.dts_refused += 1;
            return;
        }
    };
    s.dts_parsed += 1;
    let written = write_dts(&model);
    if info.guards.wrong == 0 {
        if written == bytes {
            s.dts_exact += 1;
        } else {
            s.fail(format!(
                "dts seed {seed:#x}: an accepted file does not write back to its bytes (first difference at {})",
                written.iter().zip(&bytes).position(|(a, b)| a != b).unwrap_or(written.len().min(bytes.len()))
            ));
        }
    } else {
        s.dts_wrong_guards += 1;
    }
    match dts::parse(&written, &mut free) {
        Ok(again) => {
            if write_dts(&again) != written {
                s.fail(format!(
                    "dts seed {seed:#x}: the writer's output does not write back to itself"
                ));
            }
        }
        Err(e) => s.fail(format!(
            "dts seed {seed:#x}: the writer's output is refused: {e}"
        )),
    }
    // Everything that reads a parsed shape must cope with indices that point nowhere.
    let _ = bounds::check(&model, &mut free);
    let _ = collision_slots(&model.shape.names, &model.shape.details);
    for dl in 0..model.shape.details.len().min(4) {
        let _ = bounds::detail_bounds(&model.shape, dl, &mut free);
    }
    let _ = model.shape.vertex_total();
    let mut shape = Shape::for_server(model);
    collision_case(&shape, &mut rng, s);
    if rng.one_in(4) {
        let names: Vec<Vec<u8>> = shape.names.iter().take(3).cloned().collect();
        let refs: Vec<&[u8]> = names.iter().map(Vec::as_slice).collect();
        let count = 1 + rng.below(4);
        let file = synth::dsq(&mut rng, &refs, count);
        let before = shape.clone();
        match shape.import_sequences(&file, &mut free) {
            Ok(()) => s.imports_ok += 1,
            Err(_) => {
                s.imports_refused += 1;
                if !same_shape(&shape, &before) {
                    s.fail(format!(
                        "dts seed {seed:#x}: a refused import changed the shape"
                    ));
                }
            }
        }
    }
}

/// A float for a query: mostly ordinary numbers near the meshes (which hold numbers within +-10), sometimes the values that break
/// arithmetic.
fn query_float(rng: &mut Rng) -> f32 {
    match rng.below(24) {
        0 => f32::NAN,
        1 => f32::INFINITY,
        2 => f32::NEG_INFINITY,
        3 => 1e30,
        4 => -1e30,
        5 => 0.0,
        6 => f32::MIN_POSITIVE,
        _ => (rng.below(40_001) as f32 - 20_000.0) / 1000.0,
    }
}

fn query_point(rng: &mut Rng) -> [f32; 3] {
    [query_float(rng), query_float(rng), query_float(rng)]
}

/// The collision queries over a shape that parsed: its slots by name and a made-up set that asks every detail, prepared
/// under a budget that is sometimes too small; then rays and boxes of every kind of number. Nothing may panic; a query that
/// returns has paid for its work, one the budget refused says so.
fn collision_case(shape: &Shape, rng: &mut Rng, s: &mut Summary) {
    let named = collision_slots(&shape.names, &shape.details);
    let mut all = Slots {
        collision: vec![-1; SLOTS],
        los: vec![-1; SLOTS],
    };
    for i in 0..SLOTS.min(shape.details.len()) {
        all.collision[i] = i as i32;
        all.los[i] = ((i + 1) % shape.details.len()) as i32;
    }
    for slots in [named, all] {
        let limit = if rng.one_in(3) {
            rng.below(2_000) as u64
        } else {
            u64::MAX
        };
        let mut paid = 0u64;
        let mut refused = false;
        let mut charge = |units: u64| {
            paid = paid.saturating_add(units);
            let ok = paid <= limit;
            refused |= !ok;
            ok
        };
        let built = Collision::build(shape, &slots, &mut charge);
        let collision = match built {
            Ok(c) => c,
            Err(e) => {
                s.collision_refused += 1;
                if matches!(e, ShapeError::OutOfBudget) != refused
                    && !matches!(e, ShapeError::Cap { .. })
                {
                    s.fail(format!(
                        "collision build ended with {e} (refused charge: {refused})"
                    ));
                }
                continue;
            }
        };
        if refused {
            s.fail("a collision build returned a result after a refused charge".into());
        }
        s.collision_built += 1;
        for _ in 0..8 {
            let limit = if rng.one_in(4) {
                rng.below(64) as u64
            } else {
                u64::MAX
            };
            let mut paid = 0u64;
            let mut refused = false;
            let mut charge = |units: u64| {
                paid = paid.saturating_add(units);
                let ok = paid <= limit;
                refused |= !ok;
                ok
            };
            if rng.one_in(2) {
                s.collision_rays += 1;
                let (a, b) = (query_point(rng), query_point(rng));
                match collision.cast_ray(&slots, a, b, &mut charge) {
                    Ok(hit) => {
                        s.collision_hits += usize::from(hit.is_some());
                        if refused {
                            s.fail("a ray returned a result after a refused charge".into());
                        }
                    }
                    Err(_) => {
                        s.collision_ended += 1;
                        if !refused {
                            s.fail("a ray ended without a refused charge".into());
                        }
                    }
                }
            } else {
                s.collision_boxes += 1;
                let mut world = math::IDENTITY;
                if rng.one_in(2) {
                    for k in [3, 7, 11] {
                        world[k] = query_float(rng);
                    }
                    if rng.one_in(3) {
                        let q = crate::synth::Rng(rng.bits()).bits() as i16;
                        world =
                            math::mat_mul(&world, &math::node_matrix([0, 0, q, 32767], [0.0; 3]));
                    }
                }
                let (lo, hi) = (query_point(rng), query_point(rng));
                match collision.box_hit(&slots, &world, &box_planes(lo, hi), &mut charge) {
                    Ok(hit) => {
                        s.collision_hits += usize::from(hit);
                        if refused {
                            s.fail("a box returned a result after a refused charge".into());
                        }
                    }
                    Err(_) => {
                        s.collision_ended += 1;
                        if !refused {
                            s.fail("a box ended without a refused charge".into());
                        }
                    }
                }
            }
        }
    }
}

/// The floats that break arithmetic, for the meshes.
const HOSTILE_FLOATS: [f32; 10] = [
    f32::NAN,
    f32::INFINITY,
    f32::NEG_INFINITY,
    1e30,
    -1e30,
    f32::MAX,
    f32::MIN,
    f32::MIN_POSITIVE,
    -0.0,
    1e-30,
];

fn hostile_float(rng: &mut Rng) -> f32 {
    HOSTILE_FLOATS[rng.below(HOSTILE_FLOATS.len())]
}

/// A number for a count, an index or a start: the edges of the integer types and the neighbours of the arrays.
fn hostile_int(rng: &mut Rng) -> i32 {
    const EDGES: [i32; 12] = [
        -1,
        0,
        1,
        2,
        7,
        8,
        9,
        255,
        65_535,
        65_536,
        i32::MAX,
        i32::MIN,
    ];
    if rng.one_in(4) {
        rng.bits() as i32
    } else {
        EDGES[rng.below(EDGES.len())]
    }
}

/// A mesh index list of `triangles` triangles over `verts` vertices, in one primitive of triangles.
fn scatter_triangles(rng: &mut Rng, mesh: &mut Mesh, verts: usize, triangles: usize) {
    mesh.indices = (0..3 * triangles)
        .map(|_| rng.below(verts.max(1)) as u16)
        .collect();
    mesh.primitives = vec![Primitive {
        start: 0,
        num_elements: mesh.indices.len().min(i16::MAX as usize) as u16,
        material: 0,
    }];
}

/// One damage to the meshes or the tables around them, made on the model itself. The reader would refuse most of these files
/// (`Shape::validate`); the collision code is handed the shapes all the same, so that it holds without that check in front of it.
fn hostile_edit(rng: &mut Rng, shape: &mut Shape) {
    let at = rng.below(shape.meshes.len().max(1));
    match rng.below(14) {
        // Positions that are not numbers, infinite, enormous or tiny.
        0 | 1 => {
            if let Some(Some(m)) = shape.meshes.get_mut(at) {
                let mut v = m.positions.to_vec();
                for _ in 0..1 + rng.below(6) {
                    if v.is_empty() {
                        break;
                    }
                    let (k, axis) = (rng.below(v.len()), rng.below(3));
                    v[k][axis] = hostile_float(rng);
                }
                m.positions = Arc::from(v);
            }
        }
        // Degenerate meshes: every vertex the same point, or all on a line, or all on a plane.
        2 => {
            if let Some(Some(m)) = shape.meshes.get_mut(at) {
                let mut v = m.positions.to_vec();
                let point = [rng.below(5) as f32 - 2.0, 0.5, -1.5];
                let mode = rng.below(3);
                for (k, p) in v.iter_mut().enumerate() {
                    *p = match mode {
                        0 => point,
                        1 => [k as f32, 0.0, 0.0],
                        _ => [p[0], p[1], 0.0],
                    };
                }
                m.positions = Arc::from(v);
            }
        }
        // Index lists that point past the vertices.
        3 | 4 => {
            if let Some(Some(m)) = shape.meshes.get_mut(at) {
                for _ in 0..1 + rng.below(8) {
                    if m.indices.is_empty() {
                        break;
                    }
                    let k = rng.below(m.indices.len());
                    m.indices[k] = match rng.below(4) {
                        0 => u16::MAX,
                        1 => (m.positions.len() + rng.below(3)) as u16,
                        2 => 0x8000,
                        _ => rng.bits() as u16,
                    };
                }
            }
        }
        // Primitives that start past the indices, run past them, or have a count that reads as negative.
        5 | 6 => {
            if let Some(Some(m)) = shape.meshes.get_mut(at) {
                for p in &mut m.primitives {
                    if rng.one_in(2) {
                        p.start = hostile_int(rng) as u16;
                    }
                    if rng.one_in(2) {
                        p.num_elements = hostile_int(rng) as u16;
                    }
                    if rng.one_in(3) {
                        p.material = rng.bits() as u32;
                    }
                }
                if rng.one_in(3) {
                    m.primitives.push(Primitive {
                        start: u16::MAX,
                        num_elements: i16::MAX as u16,
                        material: 0x4000_0000,
                    });
                }
            }
        }
        // Counts that do not match the arrays.
        7 => {
            if let Some(Some(m)) = shape.meshes.get_mut(at) {
                match rng.below(4) {
                    0 => m.num_verts = hostile_int(rng) as u32 as usize,
                    1 => m.verts_per_frame = hostile_int(rng),
                    2 => m.num_frames = hostile_int(rng),
                    _ => {
                        let keep = rng.below(m.positions.len() + 1);
                        m.positions = Arc::from(m.positions[..keep].to_vec());
                    }
                }
            }
        }
        // Poses: a rotation that is not a unit quaternion, a translation that is not a number.
        8 => {
            for q in &mut shape.default_rotations {
                if rng.one_in(2) {
                    *q = [rng.bits() as i16, rng.bits() as i16, 0, 0];
                }
            }
            if let Some(t) = shape.default_translations.first_mut() {
                t[rng.below(3)] = hostile_float(rng);
            }
        }
        // The node tree: a node that is its own parent, one whose parent comes later, one that does not exist.
        9 => {
            let count = shape.nodes.len();
            if let Some(n) = shape.nodes.get_mut(rng.below(count.max(1))) {
                n.parent = [0, 1, -1, 99, i32::MAX, i32::MIN][rng.below(6)];
            }
        }
        // The links from details, objects and the sub-shape to the tables.
        10 => {
            let count = shape.details.len();
            if let Some(d) = shape.details.get_mut(rng.below(count.max(1))) {
                match rng.below(3) {
                    0 => d.sub_shape = hostile_int(rng),
                    1 => d.object_detail = hostile_int(rng),
                    _ => d.name = hostile_int(rng),
                }
            }
            if let Some(o) = shape.objects.get_mut(0) {
                match rng.below(3) {
                    0 => o.node = hostile_int(rng),
                    1 => o.start_mesh_index = hostile_int(rng),
                    _ => o.num_meshes = hostile_int(rng),
                }
            }
            if let Some(ss) = shape.sub_shapes.get_mut(0) {
                match rng.below(3) {
                    0 => ss.first_object = hostile_int(rng),
                    1 => ss.num_objects = hostile_int(rng),
                    _ => ss.first_node = hostile_int(rng),
                }
            }
        }
        // A mesh this crate does not keep.
        11 => {
            if let Some(slot) = shape.meshes.get_mut(at) {
                *slot = None;
            }
        }
        // Many triangles over a few vertices: the hull and the box test at the size of the cap.
        12 => {
            if let Some(Some(m)) = shape.meshes.get_mut(at) {
                let (verts, triangles) = (m.positions.len(), 1 + rng.below(6_000));
                scatter_triangles(rng, m, verts, triangles);
            }
        }
        // Many distinct planes: up to a thousand scattered vertices and thousands of triangles over them.
        _ => {
            if let Some(Some(m)) = shape.meshes.get_mut(at) {
                let verts = 2 + rng.below(1_000);
                let v: Vec<[f32; 3]> = (0..verts)
                    .map(|_| {
                        [
                            rng.below(2_001) as f32 / 100.0 - 10.0,
                            rng.below(2_001) as f32 / 100.0 - 10.0,
                            rng.below(2_001) as f32 / 100.0 - 10.0,
                        ]
                    })
                    .collect();
                m.num_verts = verts;
                m.verts_per_frame = verts as i32;
                m.positions = Arc::from(v);
                let triangles = 1 + rng.below(4_000);
                scatter_triangles(rng, m, verts, triangles);
            }
        }
    }
}

/// The hostile-mesh pass: a shape with real solids in one to four of its collision and line-of-sight details, damaged by one to
/// three [`hostile_edit`]s, then the queries of [`collision_case`] (rays and boxes of every kind of number, some under a budget that
/// runs out). Nothing may panic, and a query that returns has paid for its work.
fn hostile_mesh_case(seed: u64, s: &mut Summary) {
    let mut rng = Rng(seed);
    let unit = [-1.0, -1.0, -1.0, 1.0, 1.0, 1.0];
    let small = [0.0, 0.0, 0.0, 0.5, 0.5, 0.5];
    let wide = [-3.0, -2.0, -1.0, 4.0, 5.0, 6.0];
    let named = [
        ("Collision-1", unit),
        ("LOS-9", small),
        ("Collision-2", wide),
        ("LOS-10", unit),
    ];
    let model = synth::solid_shape(
        [-10.0, -10.0, -10.0, 10.0, 10.0, 10.0],
        &named[..1 + rng.below(named.len())],
        if rng.one_in(2) {
            [0, 0, 0, 32767]
        } else {
            [0, 0, 23169, 23169]
        },
        [rng.below(5) as f32, 0.0, 0.0],
        rng.one_in(2),
    );
    let mut shape = Shape::for_server(model);
    for _ in 0..1 + rng.below(3) {
        hostile_edit(&mut rng, &mut shape);
    }
    s.hostile_shapes += 1;
    let before = (
        s.collision_refused,
        s.collision_rays + s.collision_boxes,
        s.collision_hits,
        s.collision_ended,
    );
    collision_case(&shape, &mut rng, s);
    s.hostile_refused += s.collision_refused - before.0;
    s.hostile_queries += s.collision_rays + s.collision_boxes - before.1;
    s.hostile_hits += s.collision_hits - before.2;
    s.hostile_ended += s.collision_ended - before.3;
}

fn dsq_case(seed: u64, s: &mut Summary) {
    let mut rng = Rng(seed);
    let wanted: [&[u8]; 3] = [b"root", b"arm", b"ghost"];
    let take = 1 + rng.below(2) + usize::from(rng.one_in(6));
    let count = rng.below(6);
    let file = synth::dsq(&mut rng, &wanted[..take.min(3)], count);
    let original = write_dsq(&file);
    let bytes = if rng.one_in(8) {
        original
    } else {
        synth::mutate(&mut rng, &original)
    };

    let mut units = 0u64;
    take_request_mark();
    let result = dsq::parse(&bytes, &mut |u| {
        units += u;
        true
    });
    check_requests(s, "dsq", seed, bytes.len());
    if units > 8 * bytes.len() as u64 + 4096 {
        s.fail(format!(
            "dsq seed {seed:#x}: parse asked for {units} units for {} bytes",
            bytes.len()
        ));
    }
    let parsed = match result {
        Ok(p) => p,
        Err(_) => {
            s.dsq_refused += 1;
            return;
        }
    };
    s.dsq_parsed += 1;
    let written = write_dsq(&parsed);
    if written == bytes {
        s.dsq_exact += 1;
    } else {
        s.fail(format!(
            "dsq seed {seed:#x}: an accepted file does not write back to its bytes"
        ));
    }
    let mut shape = synth::base_shape();
    let before = shape.clone();
    match shape.import_sequences(&parsed, &mut free) {
        Ok(()) => s.imports_ok += 1,
        Err(_) => {
            s.imports_refused += 1;
            if !same_shape(&shape, &before) {
                s.fail(format!(
                    "dsq seed {seed:#x}: a refused import changed the shape"
                ));
            }
        }
    }
    if shape.sequences.len() > MAX_SEQUENCES {
        s.fail(format!(
            "dsq seed {seed:#x}: {} sequences",
            shape.sequences.len()
        ));
    }
}

/// The three named cases.
fn named_cases(s: &mut Summary) {
    // (1) Mesh cloning: 16,000 meshes sharing one 300,000-vertex parent claim 4.8 billion vertices in a few hundred kilobytes.
    let bytes = synth::cloning_shape(300_000, 16_000);
    let mut units = 0u64;
    let result = dts::parse(&bytes, &mut |u| {
        units += u;
        true
    });
    s.cloning_refused = matches!(
        result,
        Err(ShapeError::Cap {
            what: "shape vertices",
            ..
        })
    );
    if !s.cloning_refused {
        s.fail(format!("mesh cloning: {:?}", result.err()));
    }
    // (2) Frame products that overflow 32 and 64 bits.
    let products: [(i32, i32, i32); 7] = [
        (i32::MAX, i32::MAX, i32::MAX),
        (65_536, 65_536, 65_536),
        (0x10000, 0x10000, 2),
        (i32::MAX, 1, 1),
        (1, i32::MAX, 1),
        (-1, 1, 1),
        (1, i32::MIN, 1),
    ];
    for (f, m, v) in products {
        s.frame_products_tried += 1;
        let bytes = synth::frame_product_shape(f, m, v);
        if dts::parse(&bytes, &mut free).is_err() {
            s.frame_products_refused += 1;
        } else {
            s.fail(format!("frame product {f} x {m} x {v} was accepted"));
        }
    }
    // (3) Sequence growth: the same file imported into one kept shape until the shape's total stops it.
    let mut shape = synth::base_shape();
    let file = synth::growth_dsq(100);
    let mut charged = 0u64;
    loop {
        let before = shape.clone();
        let mut spent = 0u64;
        let r = shape.import_sequences(&file, &mut |u| {
            spent += u;
            true
        });
        match r {
            Ok(()) => {
                s.growth_imports_ok += 1;
                if spent < 16 * 100 {
                    s.fail(format!(
                        "an import of 100 sequences was charged {spent} units"
                    ));
                }
                charged += spent;
            }
            Err(ShapeError::Cap { .. }) => {
                if !same_shape(&shape, &before) {
                    s.fail("a capped import changed the shape".into());
                }
                s.growth_stopped_at = shape.sequences.len();
                break;
            }
            Err(e) => {
                s.fail(format!("sequence growth ended with {e}"));
                break;
            }
        }
        if s.growth_imports_ok > MAX_SEQUENCES {
            s.fail("sequence growth never stopped".into());
            break;
        }
    }
    let _ = charged;
    // The keyframe arrays are capped in the same way: 150,000 rotations an import.
    let mut shape = synth::base_shape();
    let mut file = synth::growth_dsq(1);
    file.node_rotations = vec![[1, 2, 3, 4]; MAX_KEYFRAMES / 7];
    for _ in 0..16 {
        if shape.import_sequences(&file, &mut free).is_err() {
            s.keyframe_growth_stopped = true;
            break;
        }
    }
    if shape.node_rotations.len() > MAX_KEYFRAMES {
        s.keyframe_growth_stopped = false;
        s.fail("node rotations grew past the cap".into());
    }
}
