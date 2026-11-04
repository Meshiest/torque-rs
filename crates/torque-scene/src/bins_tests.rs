use super::*;

/// The grid these tests use: 64 x 64 cells of 8 units, with one type bit (0x0000_0080) left to a spatial index.
const FINE: BinGeometry = BinGeometry::new(64, 8.0, 0xffff_ff7f);

struct Objs(HashMap<u32, (u32, WorldBox)>);

impl Geometry for Objs {
    fn sphere(&self, _: u32) -> ([f32; 3], f32) {
        ([0.0; 3], 0.0)
    }
    fn world_box(&self, o: u32) -> WorldBox {
        self.0.get(&o).map(|e| e.1).unwrap_or_default()
    }
    fn type_mask(&self, o: u32) -> u32 {
        self.0.get(&o).map_or(0, |e| e.0)
    }
    fn collision_disabled(&self, _: u32) -> bool {
        false
    }
}

fn bx(c: [f32; 3], h: f32) -> WorldBox {
    (
        [c[0] - h, c[1] - h, c[2] - h],
        [c[0] + h, c[1] + h, c[2] + h],
    )
}

/// A box that spans the ring in x and 63 cells in y (4,032 cells), at y offset `dy`.
fn ring_box(dy: f32) -> WorldBox {
    ([-1000.0, dy, 0.0], [1000.0, dy + 500.0, 1.0])
}

#[test]
fn a_ring_wide_move_is_charged_per_cell_and_entry_and_a_refused_one_stays_put() {
    let mut bins = Bins::with_geometry(FINE);
    for id in 1..=80 {
        assert!(bins.insert(id, &ring_box(0.0)));
    }
    assert_eq!(bins.cell_entries(), 80 * 4_032);
    // One move of one of them: it leaves 4,032 cells (each list's filter looks at the 80 entries) and enters 4,032.
    let mut paid = 0u64;
    assert!(bins.check_charged(1, &ring_box(8.0), &mut |u| {
        paid += u;
        true
    }));
    let cells = 2 * 4_032u64;
    assert!(
        paid >= ENTRY_WORK + 2 * CELL_WORK * cells + ENTRY_WORK * 80 * 4_032,
        "{paid}"
    );
    // A loop of 20,000 moves (14 s if moves were free) on a network script's budget (5 * 10^6 steps) stops in a few
    // hundred moves, and a refused move leaves the object where it was.
    let mut left = 5_000_000u64 * 64;
    let mut moves = 0;
    for i in 0..20_000 {
        let mut charge = |u: u64| match left.checked_sub(u) {
            Some(l) => {
                left = l;
                true
            }
            None => {
                left = 0;
                false
            }
        };
        let target = ring_box(if i % 2 == 0 { 0.0 } else { 8.0 });
        if !bins.check_charged(2, &target, &mut charge) && left == 0 {
            break;
        }
        moves += 1;
    }
    assert!(moves < 1_000, "the budget ended the loop after {moves}");
    let before = bins.placed[&2];
    assert!(!bins.check_charged(2, &ring_box(40.0), &mut |_| false));
    assert_eq!(bins.cell_entries(), 80 * 4_032);
    assert_eq!(bins.placed[&2], before, "a refused move stays put");
}

#[test]
fn the_cells_hold_at_most_the_cap_and_an_object_past_it_goes_to_the_overflow_list() {
    let mut bins = Bins::with_geometry(FINE);
    let fit = MAX_BIN_CELL_ENTRIES / 4_032;
    for id in 0..fit as u32 + 3 {
        assert!(bins.insert(id, &ring_box(0.0)));
    }
    assert!(bins.cell_entries() <= MAX_BIN_CELL_ENTRIES);
    assert_eq!(bins.cell_entries(), fit * 4_032);
    assert_eq!(bins.overflow.len(), 3);
    // The overflow objects are still found by a walk, and every removal keeps the count right.
    let objs: HashMap<u32, (u32, WorldBox)> = (0..fit as u32 + 3)
        .map(|id| (id, (0x1, ring_box(0.0))))
        .collect();
    let mut found = HashSet::new();
    bins.walk([0.0, 1.0, 0.0, 1.0, 2.0, 1.0], 0x1, &Objs(objs), &mut |o| {
        found.insert(o);
    });
    assert_eq!(found.len(), fit + 3);
    for id in 0..fit as u32 + 3 {
        assert!(bins.remove(id));
    }
    assert_eq!(bins.cell_entries(), 0);
    assert!(bins.is_empty());
}

#[test]
fn a_cell_is_an_eighth_of_the_coordinate_wrapped_at_512() {
    assert_eq!(FINE.range(0.0, 0.0), (0, 0));
    assert_eq!(FINE.range(7.9, 8.1), (0, 1));
    assert_eq!(FINE.range(100.0, 100.0), (12, 12));
    // Negative coordinates wrap from the top: -4 is 508, cell 63.
    assert_eq!(FINE.range(-4.0, -4.0), (63, 63));
    // A span across the wrap ends 0x40 above the start's ring position.
    let (s, e) = FINE.range(-2.0, 2.0);
    assert_eq!((s, e), (63, 64));
    // More than 512 wide is the whole ring.
    assert_eq!(FINE.range(0.0, 600.0), (0, 0x3f));
    // A coordinate that is not a number is taken as 0.
    assert_eq!(FINE.range(f32::NAN, 1.0), (0, 0));
    // 512.0 itself wraps to 0 through fmod; an infinity is 0.
    assert_eq!(FINE.range(512.0, 512.0), (0, 0));
    assert_eq!(FINE.range(f32::INFINITY, f32::INFINITY), (0, 0));
}

#[test]
fn a_world_sized_object_goes_to_the_overflow_list_and_comes_last() {
    let mut bins = Bins::with_geometry(FINE);
    let mut objs = HashMap::new();
    let huge: WorldBox = ([-1e9; 3], [1e9; 3]);
    objs.insert(1, (0x1, huge));
    objs.insert(2, (0x1, bx([10.0, 10.0, 0.0], 1.0)));
    bins.insert(1, &huge);
    bins.insert(2, &objs[&2].1);
    let mut order = Vec::new();
    bins.walk(
        bx([10.0, 10.0, 0.0], 5.0)
            .0
            .iter()
            .chain(&bx([10.0, 10.0, 0.0], 5.0).1)
            .copied()
            .collect::<Vec<_>>()
            .try_into()
            .unwrap(),
        0xffff_ffff,
        &Objs(objs),
        &mut |o| order.push(o),
    );
    assert_eq!(order, vec![2, 1]);
}

#[test]
fn cells_walk_y_outer_x_inner_and_each_list_from_its_head() {
    let mut bins = Bins::with_geometry(FINE);
    let mut objs = HashMap::new();
    // Four unit objects: (x cell, y cell) = (3,0), (0,0), (0,0) again later, (1,1).
    let spots = [
        (1, [25.0, 1.0, 0.0]),
        (2, [1.0, 1.0, 0.0]),
        (3, [3.0, 1.0, 0.0]),
        (4, [9.0, 9.0, 0.0]),
    ];
    for (id, c) in spots {
        let b = bx(c, 0.25);
        objs.insert(id, (0x1, b));
        bins.insert(id, &b);
    }
    let mut order = Vec::new();
    bins.walk(
        [0.0, 0.0, -1.0, 40.0, 40.0, 1.0],
        0xffff_ffff,
        &Objs(objs),
        &mut |o| order.push(o),
    );
    // Row 0: cell x=0 holds 3 then 2 (head first), x=3 holds 1; row 1: x=1 holds 4.
    assert_eq!(order, vec![3, 2, 1, 4]);
}

#[test]
fn a_moved_object_jumps_to_the_front_of_its_bins() {
    let mut bins = Bins::with_geometry(FINE);
    let mut objs = HashMap::new();
    for id in [1, 2] {
        let b = bx([0.5, 0.5, 0.0], 0.25);
        objs.insert(id, (0x1, b));
        bins.insert(id, &b);
    }
    // Object 1 moves within its cell: unchanged numbers, no change of place.
    assert!(!bins.check(1, &bx([0.6, 0.6, 0.0], 0.1)));
    // Object 1 moves to another cell and back: it re-enters at the head.
    assert!(bins.check(1, &bx([9.0, 9.0, 0.0], 0.1)));
    assert!(bins.check(1, &bx([0.5, 0.5, 0.0], 0.25)));
    let mut order = Vec::new();
    bins.walk(
        [0.0, 0.0, -1.0, 1.0, 1.0, 1.0],
        0xffff_ffff,
        &Objs(objs),
        &mut |o| order.push(o),
    );
    assert_eq!(order, vec![1, 2]);
    assert!(bins.remove(1));
    assert!(!bins.contains(1));
    assert!(!bins.remove(1));
}

#[test]
fn the_walk_skips_masks_of_index_only_bits_and_tests_type_and_box() {
    let mut bins = Bins::with_geometry(FINE);
    let mut objs = HashMap::new();
    let b = bx([2.0, 2.0, 0.0], 0.5);
    objs.insert(7, (0x0000_0400, b));
    bins.insert(7, &b);
    let g = Objs(objs);
    let q = [0.0, 0.0, -1.0, 5.0, 5.0, 1.0];
    let mut n = 0;
    bins.walk(q, 0x0000_0080, &g, &mut |_| n += 1);
    assert_eq!(n, 0);
    bins.walk(q, 0x0000_0800, &g, &mut |_| n += 1);
    assert_eq!(n, 0);
    bins.walk(q, 0x0000_0400, &g, &mut |_| n += 1);
    assert_eq!(n, 1);
    // The box test is non-strict: touching counts.
    bins.walk([2.5, 0.0, -1.0, 9.0, 5.0, 1.0], 0x400, &g, &mut |_| n += 1);
    assert_eq!(n, 2);
    bins.walk([2.6, 0.0, -1.0, 9.0, 5.0, 1.0], 0x400, &g, &mut |_| n += 1);
    assert_eq!(n, 2);
}

#[test]
fn an_object_with_its_collision_disabled_is_not_found() {
    struct Off(Objs, u32);
    impl Geometry for Off {
        fn sphere(&self, o: u32) -> ([f32; 3], f32) {
            self.0.sphere(o)
        }
        fn world_box(&self, o: u32) -> WorldBox {
            self.0.world_box(o)
        }
        fn type_mask(&self, o: u32) -> u32 {
            self.0.type_mask(o)
        }
        fn collision_disabled(&self, o: u32) -> bool {
            o == self.1
        }
    }
    let mut bins = Bins::with_geometry(FINE);
    let mut objs = HashMap::new();
    for (id, x) in [(1u32, 10.0f32), (2, 12.0)] {
        let b = bx([x, 10.0, 0.0], 1.0);
        objs.insert(id, (0x1u32, b));
        bins.insert(id, &b);
    }
    let q = bx([11.0, 10.0, 0.0], 8.0);
    let q = [q.0[0], q.0[1], q.0[2], q.1[0], q.1[1], q.1[2]];
    let mut found = Vec::new();
    bins.walk(q, 0xffff_ffff, &Off(Objs(objs), 1), &mut |o| found.push(o));
    assert_eq!(found, vec![2]);
}

fn ray_order(bins: &Bins, objs: &Objs, a: [f32; 3], b: [f32; 3]) -> Vec<u32> {
    let mut order = Vec::new();
    bins.cast_ray(a, b, 0xffff_ffff, objs, &mut |_| true, &mut |o| {
        order.push(o);
        Ok(())
    })
    .unwrap();
    order
}

fn placed(spots: &[(u32, [f32; 3], f32)]) -> (Bins, Objs) {
    let mut bins = Bins::with_geometry(FINE);
    let mut objs = HashMap::new();
    for &(id, c, h) in spots {
        let b = bx(c, h);
        objs.insert(id, (0x1, b));
        bins.insert(id, &b);
    }
    (bins, Objs(objs))
}

#[test]
fn a_ray_meets_the_overflow_list_first_without_a_box_test() {
    let huge: WorldBox = ([-1e9; 3], [1e9; 3]);
    let (mut bins, mut objs) = placed(&[(2, [10.0, 10.0, 0.0], 1.0)]);
    bins.insert(1, &huge);
    // The overflow object's world box is far from the ray: it is still handed over (the overflow list has no box test).
    objs.0.insert(1, (0x1, ([5e8; 3], [6e8; 3])));
    assert_eq!(
        ray_order(&bins, &objs, [0.0, 10.0, 0.0], [20.0, 10.0, 0.0]),
        vec![1, 2]
    );
    // A mask of the index's bits only looks at nothing.
    let mut n = 0;
    bins.cast_ray(
        [0.0; 3],
        [20.0, 10.0, 0.0],
        0x0000_0080,
        &objs,
        &mut |_| true,
        &mut |_| {
            n += 1;
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(n, 0);
}

#[test]
fn a_vertical_ray_walks_its_column_by_ascending_y_whichever_way_it_points() {
    // Same x cell (x in 0..8), y cells 0, 1 and 3.
    let (bins, objs) = placed(&[
        (1, [1.0, 25.0, 0.0], 0.5),
        (2, [1.0, 13.0, 0.0], 0.5),
        (3, [1.0, 1.0, 0.0], 0.5),
    ]);
    assert_eq!(
        ray_order(&bins, &objs, [1.0, 0.0, 0.0], [1.0, 30.0, 0.0]),
        vec![3, 2, 1]
    );
    assert_eq!(
        ray_order(&bins, &objs, [1.0, 30.0, 0.0], [1.0, 0.0, 0.0]),
        vec![3, 2, 1]
    );
    // A box the segment does not cross is not handed over.
    assert_eq!(
        ray_order(&bins, &objs, [1.0, 0.0, 5.0], [1.0, 30.0, 5.0]),
        Vec::<u32>::new()
    );
}

#[test]
fn a_horizontal_ray_walks_its_row_by_ascending_x() {
    let (bins, objs) = placed(&[(1, [25.0, 1.0, 0.0], 0.5), (2, [5.0, 1.0, 0.0], 0.5)]);
    assert_eq!(
        ray_order(&bins, &objs, [30.0, 1.0, 0.0], [0.0, 1.0, 0.0]),
        vec![2, 1]
    );
}

#[test]
fn a_diagonal_ray_steps_x_cells_then_the_y_cells_each_covers() {
    // On the diagonal y = x: the cells (0,0), (1,1), (2,2); and one in (2,0), off the ray's cells, with a box the segment
    // does not reach either.
    let (bins, objs) = placed(&[
        (1, [21.0, 21.0, 0.0], 0.5),
        (2, [9.0, 9.0, 0.0], 0.5),
        (3, [1.0, 1.0, 0.0], 0.5),
        (4, [21.0, 1.0, 0.0], 0.5),
    ]);
    assert_eq!(
        ray_order(&bins, &objs, [24.0, 24.0, 0.0], [0.5, 0.5, 0.0]),
        vec![3, 2, 1]
    );
    // Across the wrap at 512 and over several segments of 512: no panic, and the objects near the start still come first.
    assert_eq!(
        ray_order(&bins, &objs, [0.5, 0.5, 0.0], [2000.0, 1500.0, 0.0]).first(),
        Some(&3)
    );
}

#[test]
fn a_walk_that_cannot_progress_is_stopped() {
    let (bins, objs) = placed(&[(1, [1.0, 1.0, 0.0], 0.5)]);
    // Past 2^34, `cur + 512 == cur` in f32: the walk would never end; it stops with Exhausted, whatever the budget.
    let far = 2.0f32.powi(35);
    let r = bins.cast_ray(
        [far, 0.0, 0.0],
        [far * 4.0, 1e6, 0.0],
        0xffff_ffff,
        &objs,
        &mut |_| true,
        &mut |_| Ok(()),
    );
    assert_eq!(r, Err(Exhausted));
    // A long ray within range is paid for cell by cell: a small budget stops it.
    let mut left = 10_000u64;
    let r = bins.cast_ray(
        [-1e7, -1e7, 0.0],
        [1e7, 1e7, 0.0],
        0xffff_ffff,
        &objs,
        &mut |u| {
            left = left.saturating_sub(u);
            left > 0
        },
        &mut |_| Ok(()),
    );
    assert_eq!(r, Err(Exhausted));
    // NaN and infinite inputs (the caller's gate drops them; the walk alone still ends).
    for p in [
        [f32::NAN; 3],
        [f32::INFINITY, 0.0, 0.0],
        [f32::NEG_INFINITY, f32::INFINITY, 0.0],
    ] {
        let mut budget = 1_000_000u64;
        let _ = bins.cast_ray(
            p,
            [3.0, 3.0, 0.0],
            0xffff_ffff,
            &objs,
            &mut |u| {
                budget = budget.saturating_sub(u);
                budget > 0
            },
            &mut |_| Ok(()),
        );
    }
}

#[test]
fn find_stops_at_the_callers_limit_and_pays_for_each_cell() {
    let (bins, objs) = placed(&[
        (1, [1.0, 1.0, 0.0], 0.5),
        (2, [1.5, 1.5, 0.0], 0.5),
        (3, [2.0, 2.0, 0.0], 0.5),
    ]);
    let mut got = Vec::new();
    bins.find(
        [0.0, 0.0, -1.0, 4.0, 4.0, 1.0],
        0xffff_ffff,
        &objs,
        &mut |_| true,
        &mut |o| {
            got.push(o);
            got.len() < 2
        },
    )
    .unwrap();
    assert_eq!(got.len(), 2);
    let r = bins.find(
        [-1e9, -1e9, -1.0, 1e9, 1e9, 1.0],
        0xffff_ffff,
        &objs,
        &mut |_| false,
        &mut |_| true,
    );
    assert_eq!(r, Err(Exhausted));
}

#[test]
fn the_object_point_divides_the_stored_numerator_by_the_scale() {
    let mut m = [0.0f32; 16];
    m[0] = 1.0;
    m[5] = 1.0;
    m[10] = 1.0;
    m[15] = 1.0;
    m[3] = -2.0;
    assert_eq!(
        object_point(&m, [2.0, 1.0, 4.0], [4.0, 3.0, 8.0]),
        [1.0, 3.0, 2.0]
    );
}

#[test]
fn the_float_just_below_the_wrap_is_the_wrap_less_a_hundredth() {
    assert_eq!(FINE.below_wrap.to_bits(), 0x43ff_feb8);
    assert_eq!(BinGeometry::default().wrap(), 1024.0);
    assert_eq!(BinGeometry::default().range(-64.0, -64.0), (15, 15));
}
