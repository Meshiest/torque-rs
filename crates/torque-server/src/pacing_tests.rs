use super::*;

#[test]
fn passes_land_on_the_grid_and_carry_the_fraction() {
    let mut clock = PassClock::new();
    clock.wait_until_ms(5);
    let started = Instant::now();
    let mut total = 0u64;
    let mut most = 0u32;
    for _ in 0..32 {
        let ms = clock.wait_pass();
        assert!(ms >= 1);
        most = most.max(ms);
        total += u64::from(ms);
    }
    let wall = started.elapsed();
    // 32 periods of 15.625 ms are 500 ms; the milliseconds handed out add up to the wall (the fraction is carried, none is
    // lost).
    assert!(
        wall >= Duration::from_millis(480),
        "32 passes took {wall:?}"
    );
    let wall_ms = wall.as_millis() as u64;
    assert!(
        total.abs_diff(wall_ms) <= 20,
        "{total} ms handed out over {wall_ms} ms"
    );
    // On an idle machine no pass is late by a whole period; a loaded test machine is given some room.
    assert!(most <= 60, "a pass covered {most} ms");
    assert_eq!(clock.passes(), 33);
}
