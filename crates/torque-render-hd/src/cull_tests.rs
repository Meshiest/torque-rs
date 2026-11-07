use super::*;

/// The levels tile: wherever the eye is and whatever the two distances, each cell of an L2 block is drawn by exactly one of
/// itself, its L1 block and the L2 block (with some L1 blocks and the L2 block missing their meshes, the finer level drawn).
#[test]
fn every_place_is_drawn_at_one_level() {
    let mut seed = 0x9e37_79b9u32;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed >> 8) as f32 / (1u32 << 24) as f32
    };
    let unit = |key: [i32; 3], level: u32, lod: u32| CellBox {
        live: true,
        key,
        level,
        lod,
        ..CellBox::default()
    };
    for case in 0..2000 {
        // One L2 block at (-1, 2, 0) (negative keys too), its L1 blocks with meshes but for one, the L2 mesh or not.
        let b2 = [-1, 2, 0];
        let has2 = case % 3 != 0;
        let missing1 = [(case / 3) % 2, (case / 6) % 2, (case / 12) % 2];
        let l1_of = |c: [i32; 3]| c.map(|k| k.div_euclid(2));
        let has1 = |b: [i32; 3]| {
            b.iter()
                .zip(b2)
                .map(|(k, k2)| k - 2 * k2)
                .collect::<Vec<_>>()
                != missing1
        };
        let eye = [
            (rnd() - 0.5) * 2000.0 - 64.0,
            (rnd() - 0.5) * 2000.0 + 256.0,
            (rnd() - 0.5) * 400.0,
        ];
        let d1 = rnd() * 300.0;
        let d2 = d1 + rnd() * 600.0;
        let view = ViewPlanes {
            eye: [eye[0], eye[1], eye[2], -1.0],
            ..ViewPlanes::cascade(&[0.0; 16], None, VIEW_OPAQUE)
        }
        .with_lod(d1, d2);
        // Each block's last choice (the same for every unit under it).
        let stay2 = u32::from(case % 5 == 1) * LOD_STAY_L2;
        let stay1 = |b: [i32; 3]| u32::from((b[0] + b[1] + b[2] + case) % 3 == 0) * LOD_STAY_L1;
        let top = lod_selected(&view, &unit(b2, 2, stay2));
        for i in 0..64 {
            let c = [
                b2[0] * 4 + i % 4,
                b2[1] * 4 + (i / 4) % 4,
                b2[2] * 4 + i / 16,
            ];
            let b1 = l1_of(c);
            let flags =
                u32::from(has1(b1)) * LOD_HAS_L1 + u32::from(has2) * LOD_HAS_L2 + stay1(b1) + stay2;
            let cell = lod_selected(&view, &unit(c, 0, flags));
            let mid = has1(b1)
                && lod_selected(
                    &view,
                    &unit(b1, 1, u32::from(has2) * LOD_HAS_L2 + stay1(b1) + stay2),
                );
            let coarse = has2 && top;
            let drawn = u32::from(cell) + u32::from(mid) + u32::from(coarse);
            assert_eq!(
                drawn, 1,
                "case {case}: cell {c:?} drawn {drawn} times (eye {eye:?}, d1 {d1}, d2 {d2}; cell {cell}, L1 {mid}, L2 {coarse})"
            );
        }
    }
}

/// A block drawn coarse stays so until the eye comes the hysteresis nearer than the switch, then switches once; a walk
/// bobbing a metre about the switch does not flip it back and forth.
#[test]
fn a_level_switch_holds_until_the_eye_is_well_past_it() {
    let at = 200.0;
    assert!(!lod_coarse(199.0, at, false));
    assert!(lod_coarse(200.0, at, false));
    assert!(lod_coarse(200.0 * (1.0 - LOD_HYSTERESIS) + 0.5, at, true));
    assert!(!lod_coarse(200.0 * (1.0 - LOD_HYSTERESIS) - 0.5, at, true));
    // An eye walking out and back over the switch, bobbing two units each way every step.
    let mut stay = false;
    let mut flips = 0;
    let mut last = false;
    for k in 0..400 {
        let walk = if k < 200 { k as f32 } else { (400 - k) as f32 };
        let d = 100.0 + walk + 2.0 * ((k % 4) as f32 - 1.5);
        stay = lod_coarse(d, at, stay);
        if stay != last {
            flips += 1;
            last = stay;
        }
    }
    assert_eq!(flips, 2, "one switch out, one back");
}

/// The shader's hysteresis is the CPU's.
#[test]
fn the_shader_holds_the_same_hysteresis() {
    let wgsl = crate::shaders::FILES
        .iter()
        .find(|(n, _)| *n == "cull.wgsl")
        .unwrap()
        .1;
    assert!(wgsl.contains(&format!("at * (1.0 - {LOD_HYSTERESIS})")));
    assert_eq!(LOD_STAY_L1, 4);
    assert_eq!(LOD_STAY_L2, 8);
    assert!(wgsl.contains("(c.lod & 4u)") && wgsl.contains("(c.lod & 8u)"));
}

#[test]
fn the_grid_answers_as_a_walk_of_every_cell() {
    let mut grid = CellGrid::default();
    let mut boxes: BTreeMap<[i32; 3], ([[f32; 3]; 2], bool)> = BTreeMap::new();
    let mut s = 7u64;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    for n in 0..3000 {
        let k = [
            (next() % 60) as i32 - 30,
            (next() % 60) as i32 - 30,
            (next() % 6) as i32 - 2,
        ];
        if n % 5 == 4 {
            grid.remove(k);
            boxes.remove(&k);
            continue;
        }
        let o = k.map(|v| v as f32 * 32.0);
        let lo = [0, 1, 2].map(|i| o[i] + (next() % 8) as f32);
        let hi = [0, 1, 2].map(|i| lo[i] + 1.0 + (next() % 40) as f32);
        let r = next() % 9 == 0;
        grid.insert(k, [lo, hi], r);
        boxes.insert(k, ([lo, hi], r));
    }
    assert_eq!(grid.len(), boxes.len());
    // A half-space "seen" test, monotone in the boxes.
    for (eye, cut) in [
        ([0.0, 0.0, 10.0], 100.0f32),
        ([500.0, -300.0, 40.0], -200.0),
        ([0.0; 3], 1e9),
    ] {
        let seen = |lo: [f32; 3], _hi: [f32; 3]| lo[0] <= cut;
        let mut want: Option<(f32, f32)> = None;
        for (b, _) in boxes.values() {
            let (n, f) = reach2(eye, b[0], b[1]);
            let (n, f) = (n.sqrt(), f.sqrt());
            want = Some(want.map_or((n, f), |(a, c)| (a.min(n), c.max(f))));
        }
        // Asked for exactly: the extremes over every cell.
        assert_eq!(grid.reach(eye, None, (0.0, f32::INFINITY)).0, want);
        // Seeded and bounded: a nearest under the bound stays under it, a farthest past it stays past it, else exact.
        let (wn, wf) = want.expect("cells");
        for (seed, bounds) in [
            (Some((5.0, 60.0)), (16.0, 160.0)),
            (Some((50.0, 2000.0)), (16.0, 160.0)),
            (None, (16.0, 300.0)),
            (Some((0.5, 10.0)), (0.0, f32::INFINITY)),
        ] {
            let (sn, sf) = seed.unwrap_or((f32::INFINITY, 0.0));
            let (n, f) = (wn.min(sn), wf.max(sf));
            let (got, _) = grid.reach(eye, seed, bounds);
            let (gn, gf) = got.expect("a reach");
            assert!(
                gn >= n - 1e-3 && gf <= f + 1e-3,
                "within the cells and seed"
            );
            assert!(if n < bounds.0 {
                gn < bounds.0
            } else {
                (gn - n).abs() < 1e-3
            });
            assert!(if f > bounds.1 {
                gf > bounds.1
            } else {
                (gf - f).abs() < 1e-3
            });
        }
        let refl = boxes.values().any(|(b, r)| *r && seen(b[0], b[1]));
        assert_eq!(grid.reflective_seen(&seen), refl);
        for reach in [0.0, 30.0, 200.0] {
            let near: Vec<[i32; 3]> = boxes
                .iter()
                .filter(|(_, (b, _))| reach2(eye, b[0], b[1]).0 <= reach * reach)
                .map(|(k, _)| *k)
                .collect();
            assert_eq!(grid.near(eye, reach), near);
        }
    }
}

#[test]
fn a_frustum_keeps_what_it_sees_and_drops_what_lies_behind_a_plane() {
    // An orthographic box view over x, y in -10..10 and z in 0..10 (clip depth 0 at z = 10, 1 at z = 0).
    let mut m = [0.0f32; 16];
    m[0] = 0.1;
    m[5] = 0.1;
    m[10] = -0.1;
    m[14] = 1.0;
    m[15] = 1.0;
    let v = ViewPlanes::frustum(&m, [0.0; 3], Some(5.0), VIEW_OPAQUE);
    assert!(v.seen([-1.0; 3], [1.0; 3]));
    assert!(v.seen([9.0, 9.0, 1.0], [11.0, 11.0, 2.0]), "across an edge");
    assert!(!v.seen([10.5, 0.0, 1.0], [12.0, 1.0, 2.0]), "past x = 10");
    assert!(
        !v.seen([0.0, 0.0, 10.5], [1.0, 1.0, 12.0]),
        "past the far side"
    );
    assert!(v.near([4.0, 0.0, 0.0], [6.0, 1.0, 1.0]));
    assert!(!v.near([6.0, 0.0, 0.0], [7.0, 1.0, 1.0]));
    let c = ViewPlanes::cascade(&m, Some([0.0, 0.0, 0.5, 0.5]), VIEW_OPAQUE);
    assert_eq!(c.count, 5);
    assert!(c.seen([1.0, 1.0, 1.0], [2.0, 2.0, 2.0]));
    assert!(
        !c.seen([-3.0, 1.0, 1.0], [-1.0, 2.0, 2.0]),
        "left of the patch"
    );
    assert!(!c.seen([1.0, 6.0, 1.0], [2.0, 7.0, 2.0]), "above the patch");
    assert!(
        c.seen([1.0, 1.0, 40.0], [2.0, 2.0, 50.0]),
        "toward the sun: pancaked, kept"
    );
    assert!(
        !c.seen([1.0, 1.0, -50.0], [2.0, 2.0, -40.0]),
        "beyond the far side"
    );
}

#[test]
fn a_ground_under_the_eye_leaves_the_cells_walk_nothing_to_look_at() {
    // A valley's trees: a cell every 24 units over 1.5 km, the eye amid them.
    let mut grid = CellGrid::default();
    for x in -32..32 {
        for y in -32..32 {
            let o = [x as f32 * 24.0, y as f32 * 24.0];
            grid.insert(
                [x, y, 0],
                [[o[0], o[1], 0.0], [o[0] + 4.0, o[1] + 4.0, 12.0]],
                false,
            );
        }
    }
    let eye = [10.0, 10.0, 3.0];
    let (exact, every) = grid.reach(eye, None, (0.0, f32::INFINITY));
    // Unseeded, only the unions that cannot lower or raise the extremes found so far are passed over: about half.
    assert!(every * 3 >= grid.blocks(), "{every} of {}", grid.blocks());
    // The ground under the eye (2 units down) and past the cap (160): the cells cannot change the range.
    let ground = Some((2.0, 1100.0));
    let (seeded, visited) = grid.reach(eye, ground, (16.0, 160.0));
    assert_eq!(visited, 0, "{every} blocks before");
    assert_eq!(seeded, ground);
    // The ground but a short way off (a small floor): only the blocks that could reach past it or come nearer.
    let (r, visited) = grid.reach(eye, Some((2.0, 40.0)), (16.0, 160.0));
    assert!(visited > 0 && visited < every / 4, "{visited} of {every}");
    let (r, e) = (r.expect("a reach"), exact.expect("a reach"));
    assert!(r.0 <= 2.0 && r.1 > 160.0 && e.1 > 160.0, "{r:?} {e:?}");
}
