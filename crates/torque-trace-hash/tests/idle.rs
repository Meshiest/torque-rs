//! A control object at rest is predicted where the server holds it: standing still on the floor (a StaticShape, which the
//! client's container holds with no game's static world), the figure and the buggy the client predicts are never resent.

use torque_trace_hash::rig::Control;
use torque_trace_hash::scenarios::{TICKS, idle};

/// Ticks for the buggy to drop onto its springs and come to rest first.
const WARM: u32 = 100;

#[test]
fn a_figure_and_a_buggy_standing_still_are_never_corrected() {
    for control in [Control::Figure, Control::Buggy] {
        let run = idle(control, WARM, TICKS).unwrap();
        eprintln!("{control:?}: {run:?}");
        assert_eq!(run.resends, 0, "{control:?}: {run:?}");
        assert_eq!(run.corrections, 0, "{control:?}: {run:?}");
        // The client reported its state every packet, and the server took it.
        assert!(run.matching > u64::from(TICKS) / 4, "{control:?}: {run:?}");
    }
}
