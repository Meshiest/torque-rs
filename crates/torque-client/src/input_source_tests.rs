use super::*;

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
