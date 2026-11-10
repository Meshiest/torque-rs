use super::*;
use std::time::Duration;

fn kinds(events: &[InputEvent]) -> Vec<(u32, u16, u16, u8, f32)> {
    events
        .iter()
        .map(|e| (e.device_inst, e.obj_type, e.obj_inst, e.action, e.value))
        .collect()
}

#[test]
fn a_button_makes_and_breaks_once_with_the_devices_number() {
    let mut sources = InputSources::default();
    let said = [
        JoyEvent::Button {
            device: 2,
            button: 3,
            down: true,
        },
        JoyEvent::Button {
            device: 2,
            button: 3,
            down: true,
        },
        JoyEvent::Button {
            device: 2,
            button: 3,
            down: false,
        },
        JoyEvent::Button {
            device: 2,
            button: 3,
            down: false,
        },
        JoyEvent::Button {
            device: 0,
            button: 32,
            down: true,
        },
    ];
    let events = sources.translate(&said, 0);
    assert_eq!(
        kinds(&events),
        [
            (2, SI_BUTTON, 0x103, SI_MAKE, 1.0),
            (2, SI_BUTTON, 0x103, SI_BREAK, 0.0),
        ]
    );
    assert!(events.iter().all(|e| e.device == JOYSTICK_DEVICE));
}

#[test]
fn an_axis_moves_only_when_it_changes_and_is_clamped() {
    let mut sources = InputSources::default();
    let axis = |axis, value| JoyEvent::Axis {
        device: 0,
        axis,
        value,
    };
    let events = sources.translate(
        &[
            axis(JoyAxis::X, 0.0),
            axis(JoyAxis::X, 0.25),
            axis(JoyAxis::X, 0.25),
            axis(JoyAxis::Y, 7.0),
            axis(JoyAxis::Rz, f32::NAN),
            axis(JoyAxis::Slider, -0.5),
        ],
        0,
    );
    assert_eq!(
        kinds(&events),
        [
            (0, 0x20b, 0, SI_MOVE, 0.25),
            (0, 0x20c, 0, SI_MOVE, 1.0),
            (0, 0x211, 0, SI_MOVE, -0.5),
        ]
    );
}

#[test]
fn a_hat_makes_its_directions_and_moves_its_axes() {
    let mut sources = InputSources::default();
    let pov = |hat, x, y| JoyEvent::Pov {
        device: 1,
        hat,
        x,
        y,
    };
    // Up and right: both directions make, and both axes move.
    let events = sources.translate(&[pov(0, 1, -1)], 0);
    assert_eq!(
        kinds(&events),
        [
            (1, 0x204, 0, SI_MOVE, 1.0),
            (1, 0x205, 0, SI_MOVE, -1.0),
            (1, SI_POV, 0x206, SI_MAKE, 1.0),
            (1, SI_POV, 0x209, SI_MAKE, 1.0),
        ]
    );
    // Up only: right breaks.
    let events = sources.translate(&[pov(0, 0, -1), pov(0, 0, -1)], 0);
    assert_eq!(
        kinds(&events),
        [
            (1, SI_POV, 0x209, SI_BREAK, 0.0),
            (1, 0x204, 0, SI_MOVE, 0.0)
        ]
    );
    // The second hat has its own names.
    let events = sources.translate(&[pov(1, -1, 0)], 0);
    assert_eq!(
        kinds(&events),
        [
            (1, 0x212, 0, SI_MOVE, -1.0),
            (1, SI_POV, 0x216, SI_MAKE, 1.0)
        ]
    );
}

#[test]
fn a_device_that_goes_lets_go_of_everything_it_held() {
    let mut sources = InputSources::default();
    sources.translate(
        &[
            JoyEvent::Button {
                device: 1,
                button: 0,
                down: true,
            },
            JoyEvent::Button {
                device: 1,
                button: 5,
                down: true,
            },
            JoyEvent::Axis {
                device: 1,
                axis: JoyAxis::X,
                value: 0.7,
            },
            JoyEvent::Axis {
                device: 1,
                axis: JoyAxis::Y,
                value: 0.0,
            },
            JoyEvent::Pov {
                device: 1,
                hat: 0,
                x: 0,
                y: 1,
            },
            JoyEvent::Button {
                device: 0,
                button: 1,
                down: true,
            },
        ],
        0,
    );
    let events = sources.translate(&[JoyEvent::Disconnected { device: 1 }], 0);
    assert_eq!(
        kinds(&events),
        [
            (1, SI_BUTTON, 0x100, SI_BREAK, 0.0),
            (1, SI_BUTTON, 0x105, SI_BREAK, 0.0),
            (1, SI_POV, 0x207, SI_BREAK, 0.0),
            (1, 0x205, 0, SI_MOVE, 0.0),
            (1, 0x20b, 0, SI_MOVE, 0.0),
        ]
    );
    // It is gone: a second goodbye says nothing, and the other device still holds its button.
    assert!(
        sources
            .translate(&[JoyEvent::Disconnected { device: 1 }], 0)
            .is_empty()
    );
    // A pad plugged in again starts from nothing: the same button makes again.
    let events = sources.translate(
        &[JoyEvent::Button {
            device: 1,
            button: 0,
            down: true,
        }],
        0,
    );
    assert_eq!(events.len(), 1);
}

#[test]
fn the_events_carry_the_modifier_bits() {
    let mut sources = InputSources::default();
    let events = sources.translate(
        &[JoyEvent::Button {
            device: 0,
            button: 0,
            down: true,
        }],
        gui::keys::SI_LSHIFT,
    );
    assert_eq!(events[0].modifier, gui::keys::SI_LSHIFT);
}

#[test]
fn a_source_cannot_say_more_than_the_cap_in_one_poll() {
    struct Chatty;
    impl InputSource for Chatty {
        fn poll(&mut self, cx: &mut InputCx<'_>) {
            for n in 0..(MAX_POLL_EVENTS as u32 * 2) {
                cx.axis(n, JoyAxis::X, 1.0);
            }
        }
    }
    let mut sources = InputSources::default();
    sources.add(Box::new(Chatty));
    let vm = Vm::new();
    assert_eq!(sources.poll(&vm, 0).len(), MAX_POLL_EVENTS);
}

#[test]
fn sources_poll_in_the_order_they_were_added() {
    let (a, b) = (FakeInput::default(), FakeInput::default());
    let mut sources = InputSources::default();
    sources.add(Box::new(a.clone()));
    sources.add(Box::new(b.clone()));
    b.button(1, 0, true);
    a.button(0, 0, true);
    let vm = Vm::new();
    let devices: Vec<u32> = sources.poll(&vm, 0).iter().map(|e| e.device_inst).collect();
    assert_eq!(devices, [0, 1]);
}

/// A registry over a fake source and a fake clock, and a script engine to poll with.
fn timed() -> (InputSources, FakeInput, FakeClock, Vm) {
    let (fake, clock) = (FakeInput::default(), FakeClock::default());
    let mut sources = InputSources::default();
    sources.add(Box::new(fake.clone()));
    sources.set_clock(clock.reader());
    (sources, fake, clock, Vm::new())
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

#[test]
fn a_stamped_axis_reads_as_its_mean_over_the_span() {
    let (mut sources, fake, clock, vm) = timed();
    // The first poll starts the stick's history at 0.
    fake.axis_at(clock.at(0), 0, JoyAxis::X, 0.0);
    assert!(sources.poll(&vm, 0).is_empty());
    // 5 ms at 0, then 5 ms at 1, in a 10 ms frame.
    fake.axis_at(clock.at(5), 0, JoyAxis::X, 1.0);
    clock.advance(ms(10));
    assert_eq!(kinds(&sources.poll(&vm, 0)), [(0, 0x20b, 0, SI_MOVE, 0.5)]);
    // Said nothing in the next frame: the stick sat at 1 all of it.
    clock.advance(ms(10));
    assert_eq!(kinds(&sources.poll(&vm, 0)), [(0, 0x20b, 0, SI_MOVE, 1.0)]);
    // And nothing new once the mean is the value held.
    clock.advance(ms(10));
    assert!(sources.poll(&vm, 0).is_empty());
}

#[test]
fn a_stamped_axis_is_weighted_by_time_not_by_events() {
    let (mut sources, fake, clock, vm) = timed();
    fake.axis_at(clock.at(0), 0, JoyAxis::Y, 0.0);
    sources.poll(&vm, 0);
    // Four quick moves, then a long rest: 0.4 for 1 ms, -1 for 1 ms, then 0.2 for the last 8 of 10.
    fake.axis_at(clock.at(0), 0, JoyAxis::Y, 0.4);
    fake.axis_at(clock.at(1), 0, JoyAxis::Y, -1.0);
    fake.axis_at(clock.at(2), 0, JoyAxis::Y, 0.2);
    clock.advance(ms(10));
    let events = sources.poll(&vm, 0);
    let expected = (0.4 * 1.0 - 1.0 * 1.0 + 0.2 * 8.0) / 10.0;
    assert_eq!(events.len(), 1);
    assert!((events[0].value - expected).abs() < 1e-6, "{events:?}");
}

#[test]
fn stamped_buttons_keep_their_order_across_a_frame_boundary() {
    let (mut sources, fake, clock, vm) = timed();
    fake.button_at(clock.at(1), 0, 0, true);
    fake.button_at(clock.at(2), 0, 1, true);
    fake.axis_at(clock.at(3), 0, JoyAxis::X, 1.0);
    fake.button_at(clock.at(4), 0, 0, false);
    clock.advance(ms(5));
    let first = kinds(&sources.poll(&vm, 0));
    fake.button_at(clock.at(6), 0, 1, false);
    fake.button_at(clock.at(7), 0, 2, true);
    clock.advance(ms(5));
    let second = kinds(&sources.poll(&vm, 0));
    let order: Vec<_> = first
        .iter()
        .chain(&second)
        .map(|e| (e.1, e.2, e.3))
        .collect();
    assert_eq!(
        order,
        [
            (SI_BUTTON, 0x100, SI_MAKE),
            (SI_BUTTON, 0x101, SI_MAKE),
            (SI_BUTTON, 0x100, SI_BREAK),
            // The axis (1 since it began) follows the frame's buttons, and is not said again at the same value.
            (0x20b, 0, SI_MOVE),
            (SI_BUTTON, 0x101, SI_BREAK),
            (SI_BUTTON, 0x102, SI_MAKE),
        ]
    );
}

#[test]
fn a_slow_frame_loses_no_events() {
    let (mut sources, fake, clock, vm) = timed();
    fake.axis_at(clock.at(0), 0, JoyAxis::X, 0.0);
    sources.poll(&vm, 0);
    // 100 events in a 200 ms frame: a button toggling every 2 ms, and a stick stepping 0.01 every 2 ms.
    for n in 0..100u64 {
        fake.button_at(clock.at(2 * n + 1), 0, 4, n % 2 == 0);
        fake.axis_at(clock.at(2 * n + 1), 0, JoyAxis::X, (n + 1) as f32 * 0.01);
    }
    clock.advance(ms(200));
    let events = sources.poll(&vm, 0);
    let buttons: Vec<_> = events
        .iter()
        .filter(|e| e.obj_type == SI_BUTTON)
        .map(|e| e.action)
        .collect();
    assert_eq!(buttons.len(), 100);
    assert!(buttons.chunks(2).all(|p| p == [SI_MAKE, SI_BREAK]));
    // The stick: 0 until 1 ms, then 0.01 for 2 ms, 0.02 for 2 ms ... 1.0 for the last 1 ms, over 200 ms.
    let area: f64 = (0..100u64)
        .map(|n| {
            let held = if n == 99 { 1.0 } else { 2.0 };
            held * (n + 1) as f64 * 0.01
        })
        .sum();
    let moves: Vec<_> = events.iter().filter(|e| e.obj_type == 0x20b).collect();
    assert_eq!(moves.len(), 1);
    assert!(
        (f64::from(moves[0].value) - area / 200.0).abs() < 1e-4,
        "{moves:?} {}",
        area / 200.0
    );
}

#[test]
fn a_device_that_goes_forgets_its_stamped_axes() {
    let (mut sources, fake, clock, vm) = timed();
    fake.axis_at(clock.at(0), 3, JoyAxis::X, 1.0);
    clock.advance(ms(4));
    assert_eq!(sources.poll(&vm, 0).len(), 1);
    fake.disconnect(3);
    clock.advance(ms(4));
    assert_eq!(kinds(&sources.poll(&vm, 0)), [(3, 0x20b, 0, SI_MOVE, 0.0)]);
    // It stays quiet after: no mean of a value the device no longer has.
    clock.advance(ms(4));
    assert!(sources.poll(&vm, 0).is_empty());
}

#[test]
fn unstamped_events_ignore_the_clock() {
    let (mut sources, fake, clock, vm) = timed();
    fake.axis(0, JoyAxis::X, 0.25);
    fake.axis(0, JoyAxis::X, 0.75);
    clock.advance(ms(500));
    assert_eq!(
        kinds(&sources.poll(&vm, 0)),
        [(0, 0x20b, 0, SI_MOVE, 0.25), (0, 0x20b, 0, SI_MOVE, 0.75),]
    );
}

#[test]
fn a_thread_fills_a_queue_a_source_drains() {
    struct Queued(InputQueue);
    impl InputSource for Queued {
        fn poll(&mut self, cx: &mut InputCx<'_>) {
            cx.drain(&self.0);
        }
    }
    let queue = InputQueue::default();
    let mut sources = InputSources::default();
    sources.add(Box::new(Queued(queue.clone())));
    let feeder = queue.clone();
    std::thread::spawn(move || {
        for n in 0..50 {
            feeder.button_at(Instant::now(), 0, 0, n % 2 == 0);
        }
    })
    .join()
    .expect("the thread ends");
    assert_eq!(queue.len(), 50);
    let vm = Vm::new();
    assert_eq!(sources.poll(&vm, 0).len(), 50);
    assert!(queue.is_empty());
}
