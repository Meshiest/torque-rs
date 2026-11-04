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
