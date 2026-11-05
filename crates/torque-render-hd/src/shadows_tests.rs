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

/// Nineteen shadowed lights 7 units across, four to six apart under a ceiling (an office's), and a camera that turns in place
/// and then walks under them with eight slots: a light that lights the view in two frames running never has its shadow change
/// by more than a fade step between them (no shadow switched on or off at once), though the slots change hands.
#[test]
fn lights_that_stay_in_view_fade_their_shadows_as_slots_change_hands() {
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
    let keys: Vec<[u32; 4]> = lights.iter().map(light_key).collect();
    let mut slots = vec![Slot::default(); 8];
    let mut seen_last: Vec<[u32; 4]> = Vec::new();
    let mut last: Vec<Option<f32>> = vec![None; lights.len()];
    let step = 1.0 / SLOT_FADE_FRAMES as f32;
    let (mut trades, mut faded) = (0, 0);
    let mut held_last: Vec<Option<[u32; 4]>> = slots.iter().map(|s| s.light).collect();
    for frame in 0..240 {
        let (eye, yaw) = if frame < 120 {
            ([7.0, 9.0, 1.7], frame as f32 * 3.0f32.to_radians())
        } else {
            ([2.0 + (frame - 120) as f32 * 0.15, 9.5, 1.7], 0.6)
        };
        let to = [eye[0] + yaw.cos() * 5.0, eye[1] + yaw.sin() * 5.0, 1.2];
        let camera = HdCamera::new(
            torque_render_scene::hd::kit::look_at(eye, to),
            90.0,
            300.0,
            1280,
            720,
        );
        let ranked: Vec<([u32; 4], f32)> = Shadows::lights_in_view(&lights, &camera)
            .into_iter()
            .map(|(score, i)| (keys[i], score))
            .collect();
        assign_slots(&mut slots, &ranked, &seen_last);
        assert!(slots.iter().filter(|s| s.light.is_some()).count() <= 8);
        let held: Vec<Option<[u32; 4]>> = slots.iter().map(|s| s.light).collect();
        trades += held
            .iter()
            .zip(&held_last)
            .filter(|(a, b)| a.is_some() && a != b)
            .count();
        held_last = held;
        for (i, k) in keys.iter().enumerate() {
            let in_view = ranked.iter().any(|r| r.0 == *k);
            let now = slots
                .iter()
                .find(|s| s.light == Some(*k))
                .map_or(0.0, |s| s.strength);
            if in_view && let Some(before) = last[i] {
                assert!(
                    (now - before).abs() <= step + 1e-4,
                    "frame {frame}: light {i}'s shadow went from {before} to {now}"
                );
                if now > 0.0 && now < 1.0 {
                    faded += 1;
                }
            }
            last[i] = in_view.then_some(now);
        }
        seen_last = ranked.iter().map(|r| r.0).collect();
    }
    assert!(trades > 8, "the slots changed hands {trades} times");
    assert!(faded > 0, "no shadow faded");
}

/// A light that comes into view while a slot is free has its whole shadow at once (it lit nothing seen before), and one that
/// leaves the view gives its slot up at once.
#[test]
fn a_light_new_to_the_view_takes_a_free_slot_whole_and_one_gone_frees_it() {
    let (a, b) = ([1, 0, 0, 0], [2, 0, 0, 0]);
    let mut slots = vec![Slot::default(); 2];
    assign_slots(&mut slots, &[(a, 1.0)], &[]);
    assert_eq!((slots[0].light, slots[0].strength), (Some(a), 1.0));
    // b was in view, unshadowed, with no slot to spare: its shadow fades in.
    assign_slots(&mut slots, &[(a, 1.0), (b, 0.5)], &[a, b]);
    assert_eq!(slots[1].light, Some(b));
    assert!(slots[1].strength < 0.5);
    // a leaves the view: its slot is free at once.
    assign_slots(&mut slots, &[(b, 0.5)], &[a, b]);
    assert!(slots.iter().all(|s| s.light != Some(a)));
}
