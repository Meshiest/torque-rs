//! A client controls a turret hung on a tank another client drives: its prediction runs the turret in the tank's frame, so it
//! is corrected no more often than when the same turret makes the same moves on a base that never moves, the driver is
//! corrected as often with the gunner as without, and the gunner draws its turret on the tank as it draws the tank
//! (torque-trace-hash's `turret::run`, over links that hold each datagram from the server a few ticks).

use torque_trace_hash::turret::{Base, run};

/// Steps each datagram from the server waits: the clients' moves pile up unacknowledged and a correction replays them.
const LATENCIES: [u32; 2] = [0, 3];

/// The farthest the gunner may draw its turret from its drawn base times its local pose (units).
const DRAWN_GAP: f32 = 1e-3;

#[test]
fn a_turret_on_a_driven_tank_corrects_its_gunner_no_more_than_on_a_still_base() {
    for latency in LATENCIES {
        let tank = run(Base::Tank, true, latency).expect("the turret on the tank");
        let still = run(Base::Still, true, latency).expect("the turret on a still base");
        let alone = run(Base::Tank, false, latency).expect("the tank without the gunner");
        let (on_tank, on_still) = (
            tank.gunner.expect("a gunner"),
            still.gunner.expect("a gunner"),
        );
        println!(
            "latency {latency}: the gunner corrected {on_tank} times on the tank, {on_still} on a still base; the driver {} \
             times with the gunner, {} without; the turret drawn within {} units of the tank",
            tank.driver, alone.driver, tank.drawn_gap
        );
        assert!(
            tank.aboard && still.aboard && alone.aboard,
            "the turret left its base at latency {latency}"
        );
        assert!(
            on_tank <= on_still,
            "latency {latency}: {on_tank} corrections on the tank, {on_still} on a still base"
        );
        assert_eq!(
            tank.driver, alone.driver,
            "latency {latency}: the gunner changed the driver's corrections"
        );
        for (base, gap) in [("tank", tank.drawn_gap), ("still base", still.drawn_gap)] {
            assert!(
                gap <= DRAWN_GAP,
                "latency {latency}: the turret drawn {gap} units off its {base}"
            );
        }
    }
}
