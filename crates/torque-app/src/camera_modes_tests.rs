use std::cell::Cell;
use std::rc::Rc;

use torque_render_scene::source::Eye;

use super::*;
use crate::camera_blend::SETTLE_SECONDS;
use crate::orbit::Orbit;

fn figure() -> OwnView {
    OwnView {
        ghost: 1,
        position: [10.0, 20.0, 5.0],
        yaw: 0.0,
        pitch: 0.0,
        eye: [10.0, 20.0, 6.6],
        drawn_eye: [10.0, 20.0, 6.6],
        fall: 0.0,
        fall_moved: [0.0; 3],
        cut: false,
    }
}

fn eye_at(position: [f32; 3]) -> Eye {
    Eye {
        position,
        yaw: 0.0,
        pitch: 0.0,
        fov: None,
    }
}

/// A mode that stays at one place, with the seconds its ease takes.
struct Fixed {
    at: [f32; 3],
    blend_in: Option<f32>,
    blend_out: Option<f32>,
    keeps: bool,
    log: Rc<Cell<(u32, u32, u32, u32)>>,
}

impl Fixed {
    fn at(at: [f32; 3]) -> Fixed {
        Fixed {
            at,
            blend_in: None,
            blend_out: None,
            keeps: false,
            log: Rc::default(),
        }
    }
}

impl CameraMode for Fixed {
    fn eye(&self, _: &CameraCx<'_>) -> Option<Eye> {
        Some(eye_at(self.at))
    }

    fn update(&mut self, _: &CameraCx<'_>, active: bool) {
        let (n, a, ..) = self.log.get();
        self.log.set((
            n + 1,
            a + u32::from(active),
            self.log.get().2,
            self.log.get().3,
        ));
    }

    fn enter(&mut self, _: &CameraCx<'_>) {
        let l = self.log.get();
        self.log.set((l.0, l.1, l.2 + 1, l.3));
    }

    fn leave(&mut self) {
        let l = self.log.get();
        self.log.set((l.0, l.1, l.2, l.3 + 1));
    }

    fn blend_in(&self) -> Option<f32> {
        self.blend_in
    }

    fn blend_out(&self) -> Option<f32> {
        self.blend_out
    }

    fn keeps_look(&self, _: &CameraCx<'_>) -> bool {
        self.keeps
    }
}

fn modes() -> CameraModes {
    CameraModes::from_entries(&[], |w| panic!("{w}"))
}

fn frame(modes: &CameraModes, dt: f32, fallback: Eye) -> Eye {
    let f = figure();
    let cx = CameraCx::new(&OpenWorld, dt).with_control(Some(&f));
    modes.begin(&cx);
    modes.update(&cx);
    modes.drawn(&cx, fallback)
}

#[test]
fn a_client_starts_with_the_built_in_modes_in_the_first_person() {
    let m = modes();
    assert_eq!(m.names(), [FIRST_PERSON, THIRD_PERSON, ORBIT]);
    assert_eq!(m.active(), FIRST_PERSON);
    assert!(m.is_active(FIRST_PERSON) && !m.is_active(THIRD_PERSON));
}

#[test]
fn set_picks_a_mode_in_any_case_and_leaves_an_unknown_name_alone() {
    let m = modes();
    assert!(m.set("Third-Person"));
    assert_eq!(m.active(), THIRD_PERSON);
    assert!(m.set("  orbit "));
    assert_eq!(m.active(), ORBIT);
    assert!(!m.set("sideways"));
    assert!(!m.set(""));
    assert_eq!(m.active(), ORBIT);
}

#[test]
fn names_are_lower_case_words_and_a_bad_one_is_left_out_with_a_warning() {
    for ok in ["a", "free-cam", "cam_2", &"x".repeat(MAX_NAME)] {
        assert!(valid_name(ok), "{ok}");
    }
    for bad in [
        "",
        "Upper",
        "two words",
        "tab\t",
        "dot.dot",
        &"x".repeat(MAX_NAME + 1),
    ] {
        assert!(!valid_name(bad), "{bad}");
    }
    let mut engine = EngineBuilder::new();
    engine.add_camera_mode("Bad Name", || Fixed::at([0.0; 3]));
    engine.add_camera_mode("good", || Fixed::at([0.0; 3]));
    let mut warnings = Vec::new();
    let m = CameraModes::from_entries(engine.list::<CameraModeEntry>(), |w| warnings.push(w));
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("Bad Name"));
    assert_eq!(m.names().last().map(String::as_str), Some("good"));
}

#[test]
fn a_later_mode_of_a_name_replaces_the_earlier_and_the_list_has_a_cap() {
    let m = modes();
    assert!(m.register(THIRD_PERSON, Fixed::at([1.0, 2.0, 3.0])));
    assert_eq!(m.names().len(), 3);
    m.set(THIRD_PERSON);
    let fallback = eye_at([9.0; 3]);
    let f = figure();
    let cx = CameraCx::new(&OpenWorld, 0.016).with_control(Some(&f));
    assert_eq!(m.target(&cx, fallback).position, [1.0, 2.0, 3.0]);
    for i in 0..MAX_MODES {
        let added = m.register(&format!("m{i}"), Fixed::at([0.0; 3]));
        // The built-ins hold three places.
        assert_eq!(added, i < MAX_MODES - 3, "{i}");
    }
    assert_eq!(m.names().len(), MAX_MODES);
    assert!(!m.register("one-too-many", Fixed::at([0.0; 3])));
}

#[test]
fn the_first_person_looks_from_the_eyes_and_the_third_behind_the_figure() {
    let m = modes();
    let f = OwnView {
        yaw: 0.5,
        pitch: 0.2,
        ..figure()
    };
    let cx = CameraCx::new(&OpenWorld, 0.016).with_control(Some(&f));
    let fallback = eye_at([0.0; 3]);
    let first = m.target(&cx, fallback);
    assert_eq!(first.position, f.eye);
    assert_eq!((first.yaw, first.pitch), (0.5, 0.2));
    m.set(THIRD_PERSON);
    m.begin(&cx);
    m.update(&cx);
    let third = m.target(&cx, fallback);
    // Behind the figure along its look, looking where it looks.
    assert_eq!((third.yaw, third.pitch), (0.5, 0.2));
    let rig = Rig::default();
    let pivot = rig.pivot(&f);
    let d: f32 = (0..3)
        .map(|i| (third.position[i] - pivot[i]).powi(2))
        .sum::<f32>()
        .sqrt();
    assert!(
        (d - (rig.distance.powi(2) + rig.lift.powi(2)).sqrt()).abs() < 1e-3,
        "{d}"
    );
    // No figure: the game's fallback.
    let none = CameraCx::new(&OpenWorld, 0.016);
    assert_eq!(m.target(&none, fallback), fallback);
}

#[test]
fn a_switch_eases_the_eye_to_the_new_mode_and_arrives_exactly() {
    let m = modes();
    m.register("far", Fixed::at([10.0, 0.0, 0.0]));
    m.register("near", Fixed::at([0.0, 0.0, 0.0]));
    m.set_now("near");
    let fallback = eye_at([0.0; 3]);
    assert_eq!(frame(&m, 0.016, fallback).position, [0.0; 3]);
    assert!(!m.blending());
    m.set("far");
    let first = frame(&m, 0.016, fallback).position[0];
    assert!(m.blending(), "the ease runs");
    assert!(first < 10.0, "{first}");
    let mut last = first;
    for _ in 0..200 {
        let x = frame(&m, 0.016, fallback).position[0];
        assert!(x >= last - 1e-4, "never back: {x} after {last}");
        last = x;
    }
    assert_eq!(last, 10.0);
    assert!(!m.blending());
}

#[test]
fn the_ease_takes_the_incoming_modes_time_else_the_outgoing_modes_else_the_default() {
    // 95% of the way from standing at `settle` seconds, whatever the frame time.
    let reach = |a: Fixed, b: Fixed| -> f32 {
        let m = modes();
        m.register("a", a);
        m.register("b", b);
        m.set_now("a");
        let fallback = eye_at([0.0; 3]);
        frame(&m, 0.0, fallback);
        m.set("b");
        // The switch's own frame is the first sample.
        frame(&m, 0.0, fallback);
        let mut t = 0.0;
        let mut x = 0.0;
        while t < 2.0 {
            x = frame(&m, 0.01, fallback).position[0];
            t += 0.01;
            if x > 9.5 {
                return t;
            }
        }
        let _ = x;
        f32::INFINITY
    };
    let at = |x: f32| Fixed::at([x, 0.0, 0.0]);
    let default = reach(at(0.0), at(10.0));
    assert!((default - SETTLE_SECONDS).abs() < 0.03, "{default}");
    let slow_in = reach(
        at(0.0),
        Fixed {
            blend_in: Some(1.0),
            ..at(10.0)
        },
    );
    assert!((slow_in - 1.0).abs() < 0.03, "{slow_in}");
    let slow_out = reach(
        Fixed {
            blend_out: Some(0.5),
            ..at(0.0)
        },
        at(10.0),
    );
    assert!((slow_out - 0.5).abs() < 0.03, "{slow_out}");
    let both = reach(
        Fixed {
            blend_out: Some(0.5),
            ..at(0.0)
        },
        Fixed {
            blend_in: Some(1.0),
            ..at(10.0)
        },
    );
    assert!((both - 1.0).abs() < 0.03, "{both}");
    // 0 cuts.
    let m = modes();
    m.register("a", at(0.0));
    m.register(
        "b",
        Fixed {
            blend_in: Some(0.0),
            ..at(10.0)
        },
    );
    m.set_now("a");
    let fallback = eye_at([0.0; 3]);
    frame(&m, 0.016, fallback);
    m.set("b");
    assert_eq!(frame(&m, 0.016, fallback).position[0], 10.0);
}

#[test]
fn every_mode_updates_each_frame_and_enter_and_leave_see_a_switch_once() {
    let m = modes();
    let a = Fixed::at([0.0; 3]);
    let b = Fixed::at([5.0; 3]);
    let (la, lb) = (Rc::clone(&a.log), Rc::clone(&b.log));
    m.register("a", a);
    m.register("b", b);
    m.set_now("a");
    let fallback = eye_at([0.0; 3]);
    for _ in 0..3 {
        frame(&m, 0.016, fallback);
    }
    // (updates, updates as the mode in use, enters, leaves)
    assert_eq!(la.get(), (3, 3, 0, 0));
    assert_eq!(lb.get(), (3, 0, 0, 0));
    m.set("b");
    for _ in 0..2 {
        frame(&m, 0.016, fallback);
    }
    assert_eq!(la.get(), (5, 3, 0, 1));
    assert_eq!(lb.get(), (5, 2, 1, 0));
}

#[test]
fn a_mode_that_keeps_its_look_is_drawn_looking_its_own_way_whatever_the_ease_did() {
    let turned = |keeps: bool| {
        let m = modes();
        m.register("a", Fixed::at([0.0; 3]));
        struct Turned(bool);
        impl CameraMode for Turned {
            fn eye(&self, _: &CameraCx<'_>) -> Option<Eye> {
                Some(Eye {
                    position: [10.0, 0.0, 0.0],
                    yaw: 2.0,
                    pitch: 0.3,
                    fov: None,
                })
            }
            fn keeps_look(&self, _: &CameraCx<'_>) -> bool {
                self.0
            }
        }
        m.register("b", Turned(keeps));
        m.set_now("a");
        let fallback = eye_at([0.0; 3]);
        frame(&m, 0.016, fallback);
        m.set("b");
        frame(&m, 0.016, fallback);
        frame(&m, 0.016, fallback)
    };
    let eased = turned(false);
    assert!(eased.yaw < 2.0 && eased.position[0] < 10.0, "{eased:?}");
    let kept = turned(true);
    assert_eq!((kept.yaw, kept.pitch), (2.0, 0.3));
    assert!(kept.position[0] < 10.0, "the camera still eased: {kept:?}");
}

#[test]
fn a_cut_puts_the_camera_at_the_new_place_and_every_mode_forgets() {
    let m = modes();
    m.register("far", Fixed::at([10.0, 0.0, 0.0]));
    m.register("near", Fixed::at([0.0, 0.0, 0.0]));
    m.set_now("near");
    let fallback = eye_at([0.0; 3]);
    frame(&m, 0.016, fallback);
    m.set("far");
    frame(&m, 0.016, fallback);
    assert!(m.blending());
    m.cut();
    assert!(!m.blending());
    assert_eq!(frame(&m, 0.016, fallback).position[0], 10.0);
    // The set_now also cuts, and keeps what the modes followed.
    m.set("near");
    frame(&m, 0.016, fallback);
    assert!(m.blending());
    assert!(m.set_now("far"));
    assert!(!m.blending());
    assert_eq!(frame(&m, 0.016, fallback).position[0], 10.0);
}

#[test]
fn the_orbit_turns_with_the_look_and_zooms_with_the_wheel_only_while_in_use() {
    let m = modes();
    let f = figure();
    let fallback = eye_at([0.0; 3]);
    let still = CameraCx::new(&OpenWorld, 0.016).with_control(Some(&f));
    // Not in use: the input does nothing to it.
    let spin = CameraCx::new(&OpenWorld, 0.016)
        .with_control(Some(&f))
        .with_input(CameraInput {
            look: [1.0, 0.0],
            zoom: 0.0,
        });
    m.update(&spin);
    m.set(ORBIT);
    m.begin(&still);
    m.update(&still);
    let start = m.target(&still, fallback);
    // It starts behind the figure, looking at it from the default distance.
    let target = [f.position[0], f.position[1], f.position[2] + 1.5];
    let d = |e: &Eye| {
        (0..3)
            .map(|i| (e.position[i] - target[i]).powi(2))
            .sum::<f32>()
            .sqrt()
    };
    assert!((d(&start) - Orbit::default().distance).abs() < 1e-3);
    m.update(&spin);
    let turned = m.target(&still, fallback);
    assert!((turned.yaw - start.yaw).abs() > 0.5, "{start:?} {turned:?}");
    let zoom = CameraCx::new(&OpenWorld, 0.016)
        .with_control(Some(&f))
        .with_input(CameraInput {
            look: [0.0; 2],
            zoom: 2.0,
        });
    m.update(&zoom);
    let closer = m.target(&still, fallback);
    assert!(
        d(&closer) < d(&turned) - 0.5,
        "{} {}",
        d(&closer),
        d(&turned)
    );
    // A wheel that is not a number moves nothing.
    let bad = CameraCx::new(&OpenWorld, 0.016)
        .with_control(Some(&f))
        .with_input(CameraInput {
            look: [f32::NAN; 2],
            zoom: f32::NAN,
        });
    m.update(&bad);
    assert!(
        m.target(&still, fallback)
            .position
            .iter()
            .all(|x| x.is_finite())
    );
}

#[test]
fn pushed_input_is_summed_until_taken_and_not_numbers_are_dropped() {
    let m = modes();
    m.push_input([0.1, 0.2], 1.0);
    m.push_input([0.1, f32::NAN], f32::INFINITY);
    m.push_input([0.0, 0.1], 0.5);
    let input = m.take_input();
    assert!((input.look[0] - 0.2).abs() < 1e-6 && (input.look[1] - 0.3).abs() < 1e-6);
    assert_eq!(input.zoom, 1.5);
    assert_eq!(m.take_input(), CameraInput::default());
}

#[test]
fn the_world_a_mode_reads_can_be_the_games_own_type() {
    struct Mine(f32);
    impl CameraWorld for Mine {}
    let mine = Mine(3.0);
    let cx = CameraCx::new(&mine, 0.0);
    assert_eq!(cx.world_as::<Mine>().map(|m| m.0), Some(3.0));
    assert!(cx.world_as::<OpenWorld>().is_none());
}

#[test]
fn the_third_person_stays_in_front_of_what_is_in_the_way() {
    // A wall 2 m behind the figure's pivot (north of it, the figure looks south... the boom runs against the look).
    struct Wall;
    impl CameraWorld for Wall {
        fn clear_share(&self, from: [f32; 3], to: [f32; 3]) -> Option<f32> {
            let d = to[1] - from[1];
            (d.abs() > 1e-6)
                .then(|| (-2.0 - from[1] + 20.0) / d)
                .filter(|s| (0.0..1.0).contains(s))
        }
        fn clear_hit(&self, from: [f32; 3], dir: [f32; 3], reach: f32) -> Option<f32> {
            self.clear_share(from, [0, 1, 2].map(|i| from[i] + dir[i] * reach))
                .map(|s| s * reach)
        }
    }
    let f = figure();
    let m = modes();
    m.set_now(THIRD_PERSON);
    let cx = CameraCx::new(&Wall, 0.016).with_control(Some(&f));
    for _ in 0..200 {
        m.begin(&cx);
        m.update(&cx);
    }
    let eye = m.target(&cx, eye_at([0.0; 3]));
    // Looking along +y: the boom runs back to -y; the wall is at y = 18 (2 m behind the figure at y = 20), and the camera stops
    // in front of it by the clearance.
    assert!(eye.position[1] > 18.0, "{eye:?}");
    assert!(eye.position[1] < 20.0, "{eye:?}");
}
