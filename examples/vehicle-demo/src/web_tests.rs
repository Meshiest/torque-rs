use super::*;

fn files(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("kit/forest{i:02}.dif")).collect()
}

#[test]
fn a_game_played_at_once_makes_its_interiors_a_few_a_step_then_asks_to_play_once() {
    let mut warm = WarmStart::new(files(WARM_PER_STEP * 2 + 3));
    let mut made = Vec::new();
    let mut asks = 0;
    for _ in 0..10 {
        if warm.step(false, |p| made.push(p.to_string())) {
            asks += 1;
        }
    }
    assert_eq!(asks, 1);
    assert_eq!(made.len(), WARM_PER_STEP * 2 + 3);
    assert!(!warm.waiting());
}

#[test]
fn a_kit_with_no_interiors_asks_to_play_at_the_first_step() {
    let mut warm = WarmStart::new(Vec::new());
    assert!(warm.waiting());
    assert!(warm.step(false, |_| panic!("no file to make")));
    assert!(!warm.waiting());
    assert!(!warm.step(false, |_| {}));
}

#[test]
fn what_the_player_asks_for_first_is_not_played_over() {
    let mut warm = WarmStart::new(files(WARM_PER_STEP * 3));
    let mut made = 0;
    assert!(!warm.step(false, |_| made += 1));
    // The menu's Host a game (or a room's code, or Play alone) before the interiors are all made.
    assert!(!warm.step(true, |_| made += 1));
    assert!(!warm.waiting());
    for _ in 0..5 {
        assert!(!warm.step(false, |_| made += 1));
    }
    assert_eq!(made, WARM_PER_STEP);
}
