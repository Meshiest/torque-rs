use super::*;

#[test]
fn the_noise_tiles_over_its_period() {
    let g = GradientTable::new(5, SEED);
    let c = CellTable::new(6, SEED);
    for p in [
        [0.03125f32, 0.1875, 0.34375],
        [0.390625, 0.0625, 0.875],
        [0.84375, 0.25, 0.5],
    ] {
        let q = [p[0] + 1.0, p[1] - 1.0, p[2] + 2.0];
        let a = g.at(p);
        assert!((a - g.at(q)).abs() < 1e-4, "{p:?}");
        assert!(a.abs() <= 1.2, "{a}");
        let b = c.at(p);
        assert!((b - c.at(q)).abs() < 1e-4);
        assert!((0.0..=1.0).contains(&b));
    }
}

#[test]
fn the_volumes_tile_at_their_edges_and_repeat_run_to_run() {
    // Neighbouring texels across the wrap differ no more than neighbours inside: the volume is seamless.
    let n = 16;
    let t = [
        CellTable::new(2, 1),
        CellTable::new(4, 2),
        CellTable::new(8, 3),
    ];
    let f = |p: [f32; 3]| [fbm3(p, &t[0], &t[1], &t[2]), 0.0, 0.0, 1.0];
    let v = volume(n, n, &f);
    let at = |x: usize, y: usize, z: usize| v[((z * n + y) * n + x) * 4] as i32;
    let mut inside = 0;
    let mut across = 0;
    for z in 0..n {
        for y in 0..n {
            inside = inside.max((at(n / 2, y, z) - at(n / 2 - 1, y, z)).abs());
            across = across.max((at(0, y, z) - at(n - 1, y, z)).abs());
        }
    }
    assert!(across <= inside + 8, "across {across}, inside {inside}");
    assert_eq!(v, volume(n, n, &f));
}

#[test]
fn mips_halve_down_to_one_texel_and_average() {
    let v: Vec<u8> = (0..4 * 4 * 4 * 4).map(|i| (i % 251) as u8).collect();
    let m = mips(&v, 4, 4);
    assert_eq!(
        m.iter().map(|l| (l.0, l.1)).collect::<Vec<_>>(),
        [(4, 4), (2, 2), (1, 1)]
    );
    let flat = mips(&[0, 0, 0, 0, 4, 4, 4, 4, 8, 8, 8, 8, 12, 12, 12, 12], 2, 1);
    assert_eq!(flat[1], (1, 1, vec![6, 6, 6, 6]));
}

#[test]
fn blue_noise_ranks_every_texel_once_and_has_little_low_frequency_energy() {
    let n = 32;
    let r = blue_noise(n);
    let mut seen = vec![false; n * n];
    for &k in &r {
        assert!(!seen[k]);
        seen[k] = true;
    }
    // A 4 x 4 box average of the ranks (as 0..1): white noise leaves a variance of 1/12/16; blue noise far less.
    let v: Vec<f32> = r.iter().map(|&k| k as f32 / (n * n) as f32).collect();
    let mut var = 0.0;
    for by in 0..n / 4 {
        for bx in 0..n / 4 {
            let mut s = 0.0;
            for y in 0..4 {
                for x in 0..4 {
                    s += v[(by * 4 + y) * n + bx * 4 + x];
                }
            }
            let m = s / 16.0 - 0.5;
            var += m * m;
        }
    }
    var /= (n * n / 16) as f32;
    assert!(var < 0.25 / 12.0 / 16.0, "{var}");
    assert_eq!(r, blue_noise(n));
}

#[test]
fn coverage_comes_from_the_layers_or_a_default() {
    let none = CloudSky::from_layers(Vec::new(), [0.0; 3], [0.3; 3], [0.6; 3]);
    assert_eq!(none.coverage, DEFAULT_COVERAGE);
    assert_eq!(none.tint, [1.0; 3]);
    assert_eq!(none.drift, DEFAULT_DRIFT);
    let layer = |alpha: f32, grey: f32| CloudLayer {
        alpha,
        color: [grey; 3],
        height: 0.3,
        speed: 0.0,
    };
    let light = CloudSky::from_layers(vec![layer(0.2, 1.0)], [0.0; 3], [0.3; 3], [0.6; 3]);
    let heavy = CloudSky::from_layers(
        vec![layer(0.5, 0.45), layer(0.5, 0.5), layer(1.0, 0.5)],
        [0.0; 3],
        [0.3; 3],
        [0.6; 3],
    );
    assert!(light.coverage < heavy.coverage);
    assert!(heavy.coverage >= 0.6 && heavy.coverage <= 0.7);
    assert!(
        heavy.tint[0] < 0.7 && heavy.tint[0] > 0.3,
        "{:?}",
        heavy.tint
    );
    assert!((light.tint[0] - 1.0).abs() < 1e-3);
    let windy = CloudSky::from_layers(Vec::new(), [1.0, 0.0, 0.0], [0.3; 3], [0.6; 3]);
    assert_eq!(windy.drift, [WIND_UNITS, 0.0]);
}

#[test]
fn the_ladder_has_clouds_from_medium_and_opted_in_only_when_asked() {
    let opt_in = crate::hd::ImageSettings {
        clouds_opt_in: true,
        ..crate::hd::ImageSettings::DEFAULT
    };
    assert!(
        crate::hd::ImageSettings::DEFAULT
            .clouds(Quality::Low, true)
            .is_none()
    );
    for q in [Quality::Medium, Quality::High, Quality::Ultra] {
        assert!(crate::hd::ImageSettings::DEFAULT.clouds(q, false).is_some());
        assert!(opt_in.clouds(q, false).is_none());
    }
    assert!(opt_in.clouds(Quality::Ultra, true).is_some());
    assert!(opt_in.clouds(Quality::High, true).is_none());
    let m = crate::hd::ImageSettings::DEFAULT
        .clouds(Quality::Medium, false)
        .unwrap();
    let h = crate::hd::ImageSettings::DEFAULT
        .clouds(Quality::High, false)
        .unwrap();
    let u = crate::hd::ImageSettings::DEFAULT
        .clouds(Quality::Ultra, false)
        .unwrap();
    assert!(m.scale > h.scale && !m.detail && h.detail);
    assert!(u.steps > h.steps && u.ground_shadows && !h.ground_shadows);
    // The layers by quality: the deck alone, then the cirrus over it, then all three.
    let names = |q: &CloudQuality| -> Vec<&str> {
        drawn_layers(&FAIR_WEATHER, q.layers)
            .iter()
            .map(|l| l.name)
            .collect()
    };
    assert_eq!(names(&m), ["cumulus"]);
    assert_eq!(names(&h), ["cumulus", "cirrus"]);
    assert_eq!(names(&u), ["cumulus", "altocumulus", "cirrus"]);
}

#[test]
fn layers_are_drawn_lowest_first_each_in_its_own_wind_and_cover() {
    let all = drawn_layers(&FAIR_WEATHER, u32::MAX);
    assert_eq!(all.len(), MAX_LAYERS);
    for pair in all.windows(2) {
        // Lowest first, and no two slabs overlap (the march composites them front to back).
        assert!(
            pair[0].top <= pair[1].base,
            "{} over {}",
            pair[0].name,
            pair[1].name
        );
    }
    assert!(all.iter().all(|l| l.top >= l.base));
    assert_eq!(all[0], CUMULUS);
    assert_eq!(CUMULUS.base, SLAB.base);
    // A layer's wind is the sky's turned by its own angle and scaled by its speed.
    let sky = [3.0f32, 4.0];
    for l in &FAIR_WEATHER {
        let d = layer_drift(sky, l);
        let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
        assert!((len - 5.0 * l.wind_speed).abs() < 1e-3, "{}", l.name);
        let turn = d[1].atan2(d[0]) - sky[1].atan2(sky[0]);
        assert!((turn - l.wind_turn).abs() < 1e-4, "{}", l.name);
    }
    assert_eq!(layer_drift(sky, &CUMULUS), sky);
    // The higher layers move faster than the deck.
    const {
        assert!(
            ALTOCUMULUS.wind_speed > CUMULUS.wind_speed
                && CIRRUS.wind_speed > ALTOCUMULUS.wind_speed
        )
    };
    // Coverage follows the sky's and stays within its bounds.
    assert!(layer_coverage(0.3, &ALTOCUMULUS) < layer_coverage(0.6, &ALTOCUMULUS));
    for l in &FAIR_WEATHER {
        for c in [0.0, 0.47, 1.0, 5.0] {
            assert!((0.0..=0.95).contains(&layer_coverage(c, l)));
        }
    }
    assert_eq!(drawn_layers(&FAIR_WEATHER, 0), Vec::new());
}

#[test]
fn the_weather_map_leaves_the_altocumulus_clear_rowed_and_loose_patches() {
    // b (the shader's ELEMENT_PATCHES and element_present): clear under 0.1, rows from 0.32 to 0.7, loose cells over 0.86.
    let w = &noise().weather;
    let n = (WEATHER_SIZE * WEATHER_SIZE) as f32;
    let share =
        |f: &dyn Fn(u8) -> bool| w.as_chunks::<4>().0.iter().filter(|t| f(t[2])).count() as f32 / n;
    let clear = share(&|b| b < 26);
    let rows = share(&|b| (82..=178).contains(&b));
    let loose = share(&|b| b > 219);
    assert!((0.08..0.14).contains(&clear), "clear {clear}");
    assert!((0.34..0.42).contains(&rows), "rows {rows}");
    assert!((0.11..0.17).contains(&loose), "loose {loose}");
    // a (the rows' bend) spans a fair range, so the rows turn.
    let (lo, hi) = w
        .as_chunks::<4>()
        .0
        .iter()
        .fold((255, 0), |(lo, hi), t| (lo.min(t[3]), hi.max(t[3])));
    assert!(hi - lo >= 100, "bend {lo}..{hi}");
}

#[test]
fn the_cirrus_plane_tiles_spans_its_range_and_repeats() {
    let n = 128;
    let a = cirrus_plane(n);
    assert_eq!(a.len(), n * n * 4);
    assert_eq!(a, cirrus_plane(n));
    let at = |x: usize, y: usize, c: usize| i32::from(a[(y * n + x) * 4 + c]);
    for c in 0..3 {
        let (mut inside, mut across) = (0, 0);
        for y in 0..n {
            inside = inside.max((at(n / 2, y, c) - at(n / 2 - 1, y, c)).abs());
            across = across.max((at(0, y, c) - at(n - 1, y, c)).abs());
        }
        assert!(
            across <= inside + 8,
            "channel {c}: across {across}, inside {inside}"
        );
        // Stretched to its percentiles: each channel spans most of the bytes.
        let (lo, hi) = (0..n * n).fold((255, 0), |(lo, hi), i| {
            let v = i32::from(a[i * 4 + c]);
            (lo.min(v), hi.max(v))
        });
        assert!(lo <= 12 && hi >= 243, "channel {c}: {lo}..{hi}");
    }
    // The fibres are streaks along x: neighbours along a row differ less than along a column.
    let (mut along, mut down) = (0i64, 0i64);
    for y in 0..n - 1 {
        for x in 0..n - 1 {
            along += i64::from((at(x + 1, y, 0) - at(x, y, 0)).abs());
            down += i64::from((at(x, y + 1, 0) - at(x, y, 0)).abs());
        }
    }
    assert!(along * 2 < down, "along {along}, down {down}");
}

#[test]
fn the_shadow_window_follows_the_sun_onto_the_base_and_snaps_to_texels() {
    let (c, span) = shadow_window([10.0, 20.0, 5.0], [0.0, 0.0, 1.0], 500.0);
    assert_eq!(span, 1200.0);
    let texel = span / SHADOW_TEXELS as f32;
    assert!((c[0] - 10.0).abs() <= texel && (c[1] - 20.0).abs() <= texel);
    assert_eq!(c[0] % texel, 0.0);
    let (c, _) = shadow_window([0.0, 0.0, 0.0], [1.0, 0.0, 1.0], 500.0);
    assert!((c[0] - SLAB.base).abs() <= texel, "{c:?}");
    assert_eq!(
        drift_at([2.0, -1.0], 3.0),
        [6.0, WEATHER_PERIOD * 64.0 - 3.0]
    );
}

#[test]
fn an_overcast_is_neutral_and_fair_weather_keeps_its_colour() {
    assert_eq!(overcast_share(DEFAULT_COVERAGE), 0.0);
    assert_eq!(overcast_share(OVERCAST.to), 1.0);
    let lavender = [0.6, 0.55, 0.8];
    let grey = keep_chroma(lavender, 0.0);
    let l = 0.2126 * lavender[0] + 0.7152 * lavender[1] + 0.0722 * lavender[2];
    assert!(grey.iter().all(|c| (c - l).abs() < 1e-6));
    let kept = keep_chroma(lavender, 1.0);
    assert!(kept.iter().zip(lavender).all(|(a, b)| (a - b).abs() < 1e-6));
}

#[test]
fn the_default_look_is_the_engines_and_a_look_can_cast_shadows_at_any_quality() {
    let d = CloudLook::DEFAULT;
    assert_eq!(d.layers, FAIR_WEATHER);
    assert_eq!(d.light, CLOUD_LIGHT);
    assert_eq!((d.sun, d.fine_step, d.coverage), (None, 0.0, None));
    for quality in [Quality::Medium, Quality::High, Quality::Ultra] {
        let q = CloudQuality::of(quality).unwrap();
        assert_eq!(d.quality(q), q);
        let sunlit = CloudLook::SUNLIT_CUMULUS.quality(q);
        assert!(sunlit.ground_shadows);
        assert_eq!(
            CloudQuality {
                ground_shadows: q.ground_shadows,
                ..sunlit
            },
            q
        );
    }
    // The sunlit cumulus keeps the layers' order and kinds: a quality's mask draws the same layers.
    let s = CloudLook::SUNLIT_CUMULUS;
    for (a, b) in s.layers.iter().zip(&FAIR_WEATHER) {
        assert_eq!((a.name, a.form, a.profile), (b.name, b.form, b.profile));
        assert!(a.top > a.base || a.form == LayerForm::Sheet);
    }
    assert!(s.fine_step > 0.0 && s.fine_budget >= 1.0);
}

#[test]
fn a_sanitized_look_holds_only_values_the_renderer_can_take() {
    assert_eq!(CloudLook::DEFAULT.sanitized(), CloudLook::DEFAULT);
    assert_eq!(
        CloudLook::SUNLIT_CUMULUS.sanitized(),
        CloudLook::SUNLIT_CUMULUS
    );
    let mut bad = CloudLook::SUNLIT_CUMULUS;
    for (k, l) in bad.layers.iter_mut().enumerate() {
        l.base = [f32::NAN, 5000.0, 3000.0][k];
        l.top = [f32::INFINITY, 4000.0, 3000.0][k];
        l.extinction = f32::NEG_INFINITY;
        l.shape_period = 0.0;
        l.detail_period = -1.0;
        l.weather_period = f32::NAN;
        l.fade = (f32::NAN, -5.0);
        l.sun = f32::NAN;
        l.steps = f32::INFINITY;
    }
    bad.light.forward_g = f32::NAN;
    bad.light.light_step = 0.0;
    bad.light.octaves = u32::MAX;
    bad.coverage = Some(f32::NAN);
    bad.ground_shadow = Some(7.0);
    bad.sun = Some([f32::NAN, -2.0, f32::INFINITY]);
    bad.fine_step = f32::NAN;
    bad.fine_budget = f32::INFINITY;
    let s = bad.sanitized();
    let floats = |l: &CloudLayerDef| {
        [
            l.base,
            l.top,
            l.coverage,
            l.coverage_add,
            l.extinction,
            l.shape_period,
            l.detail_period,
            l.weather_period,
            l.variation,
            l.detail,
            l.turn,
            l.wind_turn,
            l.wind_speed,
            l.edge_base,
            l.edge_top,
            l.top_density,
            l.aerial,
            l.fade.0,
            l.fade.1,
            l.reach,
            l.sun,
            l.ambient,
            l.steps,
            l.evolve,
            l.blur,
        ]
    };
    for l in &s.layers {
        assert!(floats(l).iter().all(|v| v.is_finite()), "{l:?}");
        assert!(l.shape_period > 0.0 && l.detail_period > 0.0 && l.weather_period > 0.0);
        assert!(l.fade.0 <= l.fade.1 && l.steps <= MAX_STEP_SHARE);
        match l.form {
            LayerForm::Volume => assert!(l.top - l.base >= MIN_THICKNESS, "{l:?}"),
            LayerForm::Sheet => assert_eq!(l.top, l.base),
        }
    }
    // An inverted sheet lies at the lower of its heights, a flat volume layer takes the least thickness.
    assert_eq!((s.layers[1].base, s.layers[1].top), (4000.0, 4000.0));
    assert_eq!(
        (s.layers[2].base, s.layers[2].top),
        (3000.0, 3000.0 + MIN_THICKNESS)
    );
    assert!(
        s.light.forward_g.is_finite() && s.light.light_step > 0.0 && s.light.octaves <= MAX_OCTAVES
    );
    assert_eq!(s.coverage, None);
    assert_eq!(s.ground_shadow, Some(1.0));
    assert_eq!(s.sun, Some([0.0, 0.0, 0.0]));
    assert_eq!((s.fine_step, s.fine_budget), (0.0, 1.0));
    let huge = CloudLook {
        fine_budget: 1e9,
        ..CloudLook::SUNLIT_CUMULUS
    };
    assert_eq!(huge.sanitized().fine_budget, CloudLook::MAX_FINE_BUDGET);
    assert_eq!(
        CloudLook {
            coverage: Some(3.0),
            ..CloudLook::DEFAULT
        }
        .sanitized()
        .coverage,
        Some(MAX_COVERAGE)
    );
}

/// The cell noise as it was first written: every neighbor wrapped into the tile.
fn cell_reference(t: &CellTable, p: [f32; 3]) -> f32 {
    let n = t.n;
    let q = p.map(|v| v * n as f32);
    let i = q.map(|v| v.floor() as i32);
    let mut best = f32::MAX;
    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let c = [i[0] + dx, i[1] + dy, i[2] + dz];
                let w = c.map(|v| v.rem_euclid(n));
                let h = t.pts[((w[2] * n + w[1]) * n + w[0]) as usize];
                let d = [0, 1, 2].map(|k| c[k] as f32 + h[k] - q[k]);
                best = best.min(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]);
            }
        }
    }
    (1.0 - best.sqrt()).clamp(0.0, 1.0)
}

/// The gradient noise as it was first written: every corner wrapped into the tile.
fn gradient_reference(t: &GradientTable, p: [f32; 3]) -> f32 {
    let n = t.n;
    let q = p.map(|v| v * n as f32);
    let i = q.map(|v| v.floor());
    let f = [q[0] - i[0], q[1] - i[1], q[2] - i[2]];
    let i = i.map(|v| v as i32);
    let corner = |o: [i32; 3]| -> f32 {
        let c = [0, 1, 2].map(|k| (i[k] + o[k]).rem_euclid(n));
        let g = t.g[((c[2] * n + c[1]) * n + c[0]) as usize];
        let d = [0, 1, 2].map(|k| f[k] - o[k] as f32);
        g[0] * d[0] + g[1] * d[1] + g[2] * d[2]
    };
    let u = f.map(fade);
    let x00 = lerp(corner([0, 0, 0]), corner([1, 0, 0]), u[0]);
    let x10 = lerp(corner([0, 1, 0]), corner([1, 1, 0]), u[0]);
    let x01 = lerp(corner([0, 0, 1]), corner([1, 0, 1]), u[0]);
    let x11 = lerp(corner([0, 1, 1]), corner([1, 1, 1]), u[0]);
    lerp(lerp(x00, x10, u[1]), lerp(x01, x11, u[1]), u[2]) * 1.15
}

#[test]
fn the_tables_give_the_wrapped_lattices_values_to_the_bit() {
    // Points on the volumes' texel centers, others inside the tile, its edges and outside it (the wrapping path).
    let mut points = Vec::new();
    for k in 0..400u32 {
        let h = |s: u32| (hash(k as i32, 3, 5, s) >> 8) as f32 / 16_777_216.0;
        points.push([h(1), h(2), h(3)]);
        points.push([h(4) * 3.0 - 1.0, h(5) * 2.0 - 0.5, 1.0 + h(6)]);
    }
    for size in [16usize, 64] {
        for z in [0, size / 3, size - 1] {
            for y in [0, size / 2, size - 1] {
                for x in [0, 1, size - 1] {
                    points.push([x, y, z].map(|v| (v as f32 + 0.5) / size as f32));
                }
            }
        }
    }
    points.extend([
        [0.0; 3],
        [1.0, 1.0, 1.0],
        [0.999_999_9, 0.0, 0.5],
        [-0.0, 0.5, 0.25],
    ]);
    for n in [1, 2, 3, 5, 8, 24] {
        let c = CellTable::new(n, SEED ^ n as u32);
        let g = GradientTable::new(n, SEED ^ 0x10 ^ n as u32);
        for &p in &points {
            assert_eq!(
                c.at(p).to_bits(),
                cell_reference(&c, p).to_bits(),
                "cell {n} at {p:?}"
            );
            assert_eq!(
                g.at(p).to_bits(),
                gradient_reference(&g, p).to_bits(),
                "gradient {n} at {p:?}"
            );
        }
    }
}

#[test]
fn a_build_in_pieces_counts_its_steps_and_stops_after_the_last() {
    assert_eq!(
        NoiseBuild::STEPS,
        1 + SHAPE_SIZE + DETAIL_SIZE + 1 + CIRRUS_SIZE / CIRRUS_ROWS_A_STEP + 1
    );
    let b = NoiseBuild::new();
    assert!(!b.done());
}

// Slow unoptimized (a minute): `cargo test --release -p torque-render-scene -- --ignored`.
#[test]
#[ignore]
fn a_build_in_pieces_makes_the_same_bytes_as_one_on_the_threads() {
    let mut b = NoiseBuild::new();
    let mut steps = 0;
    while !b.done() {
        b.step();
        steps += 1;
    }
    assert_eq!(steps, NoiseBuild::STEPS);
    let (a, b) = (b.finish(), generate());
    assert!(a.shape == b.shape && a.detail == b.detail && a.weather == b.weather);
    assert!(a.blue == b.blue && a.cirrus == b.cirrus);
}

/// The noise's pieces at small sizes, pinned to the bytes the code made before its lattices read from padded tables and before
/// it could be made in pieces (each hash taken then; the full sizes are the same code at larger sizes).
#[test]
fn the_noise_keeps_its_bytes() {
    let fnv = |b: &[u8]| {
        b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &x| {
            (h ^ u64::from(x)).wrapping_mul(0x0100_0000_01b3)
        })
    };
    let t = NoiseTables::new();
    let mut weather = volume(32, 1, &|p| t.weather_texel(p));
    equalize(&mut weather, 2);
    let shape = volume(16, 16, &|p| t.shape.texel(p));
    let detail = volume(16, 16, &|p| detail_texel(&t.detail, p));
    let cirrus = cirrus_plane(64);
    let blue: Vec<u8> = blue_noise(16)
        .iter()
        .flat_map(|&r| (r as u32).to_le_bytes())
        .collect();
    assert_eq!(
        [&shape, &detail, &weather, &cirrus, &blue].map(|b| fnv(b)),
        [
            0x8c26_592d_0535_9296,
            0xb5cb_446e_f1ac_9b4e,
            0x4dfd_6cde_06d1_676f,
            0x149d_2250_e55a_b02e,
            0x8106_88e1_ab29_0eb5,
        ]
    );
}
