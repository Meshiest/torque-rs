use super::*;

#[test]
fn a_cascade_moved_by_whole_texels_scrolls_and_nothing_else_does() {
    let size = 1024;
    let mut old = [0.0f32; 16];
    old[0] = 0.01;
    old[5] = 0.01;
    old[10] = 0.005;
    old[12] = -0.25;
    old[13] = 0.5;
    old[14] = 0.3;
    old[15] = 1.0;
    // 64 texels across (clip 0.125), 32 down.
    let mut new = old;
    new[12] += 0.125;
    new[13] -= 0.0625;
    assert_eq!(scroll_of(&old, &new, size), Some([64, 32]));
    assert_eq!(scroll_of(&old, &old, size), None, "not moved");
    let mut half = new;
    half[12] += 1.0 / 2048.0;
    assert_eq!(scroll_of(&old, &half, size), None, "half a texel");
    let mut deeper = new;
    deeper[14] += 0.01;
    assert_eq!(
        scroll_of(&old, &deeper, size),
        None,
        "its depth range moved"
    );
    let mut far = old;
    far[12] += 2.5;
    assert_eq!(scroll_of(&old, &far, size), None, "past the map");
    assert_eq!(
        uncovered([64, 32], size),
        vec![[0, 0, 64, 1024], [0, 0, 1024, 32]]
    );
    assert_eq!(
        uncovered([-64, -32], size),
        vec![[960, 0, 1024, 1024], [0, 992, 1024, 1024]]
    );
    assert_eq!(uncovered([0, 5], size), vec![[0, 0, 1024, 5]]);
}

/// Nineteen shadowed spot lights 7 units across, four to six apart under a ceiling (an office's), and a camera that turns in
/// place and then walks under them with High's tiers: every light in view has its whole shadow in every frame (at once when it
/// comes into view; through a crossfade when it changes tier), though the lights change tiers as the camera moves.
#[test]
fn every_light_in_view_keeps_its_whole_shadow_as_lights_change_tiers() {
    let mut lights = Vec::new();
    for x in [2.0f32, 8.0, 14.0, 20.0, 26.0] {
        for y in [2.0f32, 7.0, 12.0, 17.0] {
            if lights.len() < 19 {
                lights.push(HdLight {
                    position: [x, y, 3.1],
                    color: [1.6, 1.52, 1.38],
                    radius: 7.0,
                    carrier: None,
                });
            }
        }
    }
    let cones = vec![[0.0, 0.0, -3.0, -0.26]; lights.len()];
    let keys: Vec<[u32; 4]> = lights.iter().map(light_key).collect();
    let q = torque_render_scene::hd::Quality::High.shadows();
    let ends: Vec<usize> = point_tiers(&q)
        .iter()
        .scan(0, |e, &(_, n)| {
            *e += n;
            Some(*e)
        })
        .collect();
    assert!(*ends.last().unwrap() >= 19, "{ends:?}");
    let mut slots = vec![Slot::default(); *ends.last().unwrap()];
    let mut seen_last: Vec<[u32; 4]> = Vec::new();
    let tier = |s: usize| ends.iter().position(|&e| s < e).unwrap();
    let mut last_tier: Vec<Option<usize>> = vec![None; lights.len()];
    let (mut changes, mut crossfading) = (0, 0);
    for frame in 0..240 {
        let (eye, yaw) = if frame < 120 {
            ([7.0, 9.0, 1.7], frame as f32 * 3.0f32.to_radians())
        } else {
            ([2.0 + (frame - 120) as f32 * 0.15, 9.5, 1.7], 0.6)
        };
        let to = [eye[0] + yaw.cos() * 5.0, eye[1] + yaw.sin() * 5.0, 1.2];
        let camera = HdCamera::new(
            torque_render_scene::hd::kit::look_at(eye, to),
            90.0_f32,
            300.0,
            1280,
            720,
        );
        let ranked: Vec<([u32; 4], f32)> = Shadows::lights_in_view(&lights, &cones, &camera)
            .into_iter()
            .map(|(score, i)| (keys[i], score))
            .collect();
        assign_slots(&mut slots, &ends, &ranked, &seen_last);
        for (i, k) in keys.iter().enumerate() {
            if !ranked.iter().any(|r| r.0 == *k) {
                assert!(
                    slots.iter().all(|s| s.light != Some(*k)),
                    "an unseen light holds a cube"
                );
                last_tier[i] = None;
                continue;
            }
            let held: Vec<usize> = (0..slots.len())
                .filter(|&s| slots[s].light == Some(*k))
                .collect();
            let whole: f32 = held.iter().map(|&s| slots[s].strength).sum();
            assert!(
                (whole - 1.0).abs() < 1e-3,
                "frame {frame}: light {i}'s shadow at {whole} in cubes {held:?}"
            );
            if held.len() > 1 {
                crossfading += 1;
            }
            let now = held.iter().find(|&&s| !slots[s].leaving).map(|&s| tier(s));
            if last_tier[i].is_some() && now != last_tier[i] {
                changes += 1;
            }
            last_tier[i] = now;
        }
        seen_last = ranked.iter().map(|r| r.0).collect();
    }
    assert!(changes > 4, "the lights changed tiers {changes} times");
    assert!(crossfading > 0, "no crossfade");
}

/// A light that comes into view with a cube free has its whole shadow at once; one that leaves the view gives its cubes up at
/// once; with more lights in view than cubes, the least important fades out.
#[test]
fn a_light_new_to_the_view_takes_a_whole_cube_and_one_gone_frees_it() {
    let (a, b, c) = ([1, 0, 0, 0], [2, 0, 0, 0], [3, 0, 0, 0]);
    let ends = [1, 2];
    let mut slots = vec![Slot::default(); 2];
    assign_slots(&mut slots, &ends, &[(a, 1.0)], &[]);
    assert_eq!((slots[0].light, slots[0].strength), (Some(a), 1.0));
    assign_slots(&mut slots, &ends, &[(a, 1.0), (b, 0.5)], &[a]);
    assert_eq!((slots[1].light, slots[1].strength), (Some(b), 1.0));
    // c outranks both but there is no cube for three: b (the least) fades out, keeping its cube until it has.
    assign_slots(&mut slots, &ends, &[(a, 1.0), (b, 0.5), (c, 3.0)], &[a, b]);
    assert!(slots[1].leaving && slots[1].strength < 1.0);
    assign_slots(&mut slots, &ends, &[(c, 3.0)], &[a, b, c]);
    assert!(
        slots
            .iter()
            .all(|s| s.light != Some(a) && s.light != Some(b))
    );
}

/// The tiers of every quality hold more cubes than its point lights in no more memory than equal cubes of its face size did.
#[test]
fn the_tiers_hold_more_lights_in_less_memory() {
    use torque_render_scene::hd::Quality;
    for quality in [Quality::Medium, Quality::High, Quality::Ultra] {
        let q = quality.shadows();
        let tiers = point_tiers(&q);
        let count: usize = tiers.iter().map(|t| t.1).sum();
        let texels: u64 = tiers
            .iter()
            .map(|&(s, n)| u64::from(s).pow(2) * n as u64)
            .sum();
        let equal = u64::from(q.point_size).pow(2) * q.point_lights as u64;
        assert!(
            count >= 2 * q.point_lights && count <= MAX_POINT_SLOTS,
            "{quality:?}: {count}"
        );
        assert!(
            texels <= equal,
            "{quality:?}: {texels} texels, equal cubes {equal}"
        );
    }
}
