//! A forced correction (the server moves the control object sideways while the client predicts it in place): with TGE's
//! predictor the drawn object and the camera jump with the simulation; with the smoothed predictor the simulation jumps the
//! same and what is drawn moves only a little each frame, then settles where the simulation is.
//!
//! The steps are measured across the floor (x and y), the way the server moves the object: standing still in this world, both
//! the figure and the buggy are also resent every few ticks with a small height difference (the client's prediction sinks a
//! little between resends), which both predictors carry the same in the simulation.

use torque_client::predict::slot::PredictionEntry;
use torque_plugin_smooth_prediction::SmoothPrediction;
use torque_trace_hash::rig::Control;
use torque_trace_hash::scenarios::{DrawnFrame, forced_corrections};

const KICK: [f32; 3] = [0.8, 0.3, 0.0];
const EVERY: u32 = 40;
const KICKS: u32 = 3;
/// A window drawing at eight times the tick rate (250 frames a second).
const FRAMES: u32 = 8;
/// The most the smoothed object may move in one frame (units): a sixteenth of the kick.
const MAX_FRAME_STEP: f32 = 0.055;

/// The distance across the floor.
fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..2).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>().sqrt()
}

/// The largest step across the floor between consecutive frames of `f(frame)`.
fn max_step(frames: &[DrawnFrame], f: impl Fn(&DrawnFrame) -> Option<[f32; 3]>) -> f32 {
    frames
        .windows(2)
        .filter_map(|w| Some(dist(f(&w[0])?, f(&w[1])?)))
        .fold(0.0, f32::max)
}

fn run(control: Control, prediction: Option<SmoothPrediction>) -> Vec<DrawnFrame> {
    let entry = prediction.map(PredictionEntry::new);
    let frames = forced_corrections(control, entry, KICK, EVERY, KICKS, FRAMES).unwrap();
    assert_eq!(frames.len(), (EVERY * KICKS * FRAMES) as usize);
    assert!(frames.iter().all(|f| f.drawn.is_some() && f.sim.is_some()));
    frames
}

fn kick() -> f32 {
    dist(KICK, [0.0; 3])
}

#[test]
fn tge_snaps_what_is_drawn_with_the_simulation() {
    for control in [Control::Buggy, Control::Figure] {
        let frames = run(control, None);
        assert!(max_step(&frames, |f| f.sim) > 0.9 * kick(), "{control:?}");
        assert!(max_step(&frames, |f| f.drawn) > 0.9 * kick(), "{control:?}");
        assert!(max_step(&frames, |f| f.view) > 0.9 * kick(), "{control:?}");
    }
}

#[test]
fn smoothing_eases_the_jump_out_of_what_is_drawn() {
    for control in [Control::Buggy, Control::Figure] {
        let frames = run(control, Some(SmoothPrediction::default()));
        // The simulation corrects exactly as TGE's does...
        assert!(max_step(&frames, |f| f.sim) > 0.9 * kick(), "{control:?}");
        // ...and neither the body nor the camera jumps.
        let drawn = max_step(&frames, |f| f.drawn);
        let view = max_step(&frames, |f| f.view);
        assert!(
            drawn <= MAX_FRAME_STEP,
            "{control:?}: the body moved {drawn} in a frame"
        );
        assert!(
            view <= MAX_FRAME_STEP,
            "{control:?}: the camera moved {view} in a frame"
        );
    }
}

#[test]
fn what_is_drawn_settles_on_the_simulation_and_on_tges() {
    for control in [Control::Buggy, Control::Figure] {
        let smooth = run(control, Some(SmoothPrediction::default()));
        let tge = run(control, None);
        for k in 1..=KICKS {
            let at = (k * EVERY * FRAMES) as usize - 1;
            let (s, t) = (&smooth[at], &tge[at]);
            assert_eq!(s.sim, t.sim, "{control:?} kick {k}: the same simulation");
            let gap = dist(s.drawn.unwrap(), s.sim.unwrap());
            assert!(
                gap < 1e-3,
                "{control:?} kick {k}: drawn {gap} from the simulation"
            );
            assert!(
                dist(s.drawn.unwrap(), t.drawn.unwrap()) < 1e-3,
                "{control:?} kick {k}"
            );
        }
        eprintln!(
            "{control:?}: the largest step in a frame {:.4} smoothed (camera {:.4}), {:.4} TGE's",
            max_step(&smooth, |f| f.drawn),
            max_step(&smooth, |f| f.view),
            max_step(&tge, |f| f.drawn)
        );
    }
}

#[test]
fn a_jump_past_the_snap_distance_is_drawn_at_once() {
    let frames = run(
        Control::Buggy,
        Some(SmoothPrediction::default().snap_distance(0.5)),
    );
    assert!(max_step(&frames, |f| f.drawn) > 0.9 * kick());
}
