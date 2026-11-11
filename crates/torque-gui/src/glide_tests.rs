use torque_ts_vm::vm::Vm;

use super::*;
use crate::draw::{DrawCache, draw_canvas};
use crate::test_rig::{id, point_at, press, rig, scroll_body, wheel, y_of};

/// A rig with the pointer over the scroll control.
fn scrolling() -> (Vm, Gui) {
    let (mut vm, gui) = rig(&scroll_body());
    point_at(&mut vm, &gui, (50, 50));
    (vm, gui)
}

fn smooth(vm: &mut Vm) {
    vm.eval(b"setSmoothScroll(1);").unwrap();
}

fn scroll_to(vm: &mut Vm, gui: &Gui, y: i32) {
    let scroll = id(vm, "Scroll");
    crate::scroll::scroll_to(vm, gui, scroll, 0, y);
}

/// The y of the marked row's fill in a frame (it is the only quad of its color).
fn marked_row_y(vm: &Vm, gui: &Gui) -> f32 {
    let frame = draw_canvas(vm, gui, &mut DrawCache::default());
    let quad = frame
        .draws
        .batches
        .iter()
        .flat_map(|b| &b.quads)
        .find(|q| q.color == [255, 0, 255, 255])
        .expect("the marked row is drawn");
    quad.rect[1]
}

#[test]
fn smooth_scrolling_is_off_until_a_pref_or_the_call_turns_it_on() {
    let (mut vm, gui) = scrolling();
    assert!(!enabled(&vm));
    wheel(&mut vm, &gui, false);
    // TGE's step: a row, and the picture is the place.
    assert_eq!(y_of(&vm, "Page"), -30);
    assert_eq!(shift(&gui, id(&vm, "Scroll"), vm.sim_time()), (0, 0));
    vm.eval(b"$Pref::Gui::SmoothScroll = 1;").unwrap();
    assert!(enabled(&vm));
    vm.eval(b"setSmoothScroll(0);").unwrap();
    assert_eq!(vm.global(PREF).as_deref(), Some(&b"0"[..]));
    smooth(&mut vm);
    assert!(enabled(&vm));
    assert_eq!(vm.eval(b"return getSmoothScroll();").unwrap(), b"1");
}

#[test]
fn the_content_is_at_its_final_place_at_once_and_only_the_picture_glides() {
    let (mut vm, gui) = scrolling();
    smooth(&mut vm);
    let scroll = id(&vm, "Scroll");
    let before = marked_row_y(&vm, &gui);
    wheel(&mut vm, &gui, false);
    // What a script sees: the final place, now.
    assert_eq!(y_of(&vm, "Page"), -30);
    assert_eq!(crate::scroll::state(&gui, scroll).child_rel, (0, 30));
    // What is drawn: where it was, a row from where it is.
    assert_eq!(shift(&gui, scroll, vm.sim_time()), (0, 30));
    assert_eq!(marked_row_y(&vm, &gui), before);
    // Halfway the cubic has an eighth of the way left.
    vm.advance_time(75);
    assert_eq!(shift(&gui, scroll, vm.sim_time()), (0, 4));
    assert_eq!(marked_row_y(&vm, &gui), before - 30.0 + 4.0);
    assert!(any_gliding(&gui, vm.sim_time()));
    vm.advance_time(75);
    assert_eq!(shift(&gui, scroll, vm.sim_time()), (0, 0));
    assert!(!any_gliding(&gui, vm.sim_time()));
    assert_eq!(marked_row_y(&vm, &gui), before - 30.0);
    assert_eq!(y_of(&vm, "Page"), -30);
}

#[test]
fn a_glide_takes_the_same_clock_time_however_the_frames_come() {
    let at_75 = |frames: u32| {
        let (mut vm, gui) = scrolling();
        smooth(&mut vm);
        wheel(&mut vm, &gui, false);
        for _ in 0..frames {
            vm.advance_time(75 / frames);
            // A frame drawn in between changes nothing.
            draw_canvas(&vm, &gui, &mut DrawCache::default());
        }
        shift(&gui, id(&vm, "Scroll"), vm.sim_time())
    };
    assert_eq!(at_75(1), (0, 4));
    assert_eq!(at_75(5), at_75(1));
    assert_eq!(at_75(25), at_75(1));
}

#[test]
fn a_notch_that_comes_while_gliding_goes_on_from_the_place_drawn() {
    let (mut vm, gui) = scrolling();
    smooth(&mut vm);
    let scroll = id(&vm, "Scroll");
    wheel(&mut vm, &gui, false);
    vm.advance_time(75);
    assert_eq!(shift(&gui, scroll, vm.sim_time()), (0, 4));
    wheel(&mut vm, &gui, false);
    // The place is two rows down; the picture is 4 pixels from the first row's place, plus the new row.
    assert_eq!(y_of(&vm, "Page"), -60);
    assert_eq!(shift(&gui, scroll, vm.sim_time()), (0, 34));
}

#[test]
fn a_press_ends_every_glide_and_leaves_the_content_where_it_is() {
    let (mut vm, gui) = scrolling();
    smooth(&mut vm);
    let scroll = id(&vm, "Scroll");
    wheel(&mut vm, &gui, false);
    assert_eq!(shift(&gui, scroll, vm.sim_time()), (0, 30));
    press(&mut vm, &gui);
    assert_eq!(shift(&gui, scroll, vm.sim_time()), (0, 0));
    assert_eq!(y_of(&vm, "Page"), -30);
}

#[test]
fn keyboard_and_script_scrolls_move_at_once_with_the_glide_left_as_it_was() {
    let (mut vm, gui) = scrolling();
    smooth(&mut vm);
    let scroll = id(&vm, "Scroll");
    vm.eval(b"Scroll.scrollToBottom();").unwrap();
    assert_eq!(y_of(&vm, "Page"), -300);
    assert_eq!(shift(&gui, scroll, vm.sim_time()), (0, 0));
    vm.eval(b"Scroll.scrollToTop();").unwrap();
    assert_eq!(y_of(&vm, "Page"), 0);
    assert_eq!(shift(&gui, scroll, vm.sim_time()), (0, 0));
}

#[test]
fn the_picture_never_leaves_the_scroll_range() {
    let (mut vm, gui) = scrolling();
    smooth(&mut vm);
    let scroll = id(&vm, "Scroll");
    wheel(&mut vm, &gui, false);
    // A script then puts the content back at the top: the place drawn (a row below it) would be above the range.
    vm.eval(b"Scroll.scrollToTop();").unwrap();
    let (s, d) = crate::scroll::shown(&vm, &gui, scroll, vm.sim_time());
    assert_eq!(d, (0, 0));
    assert_eq!(s.child_rel, (0, 0));
}

#[test]
fn smooth_scrolling_turned_off_ends_the_glides() {
    let (mut vm, gui) = scrolling();
    smooth(&mut vm);
    wheel(&mut vm, &gui, false);
    vm.eval(b"setSmoothScroll(0);").unwrap();
    assert_eq!(shift(&gui, id(&vm, "Scroll"), vm.sim_time()), (0, 0));
    // And a notch no longer glides.
    wheel(&mut vm, &gui, false);
    assert_eq!(shift(&gui, id(&vm, "Scroll"), vm.sim_time()), (0, 0));
}

#[test]
fn parts_of_a_notch_move_the_content_by_their_share_and_whole_notches_land_as_they_do_without() {
    let (mut vm, gui) = scrolling();
    smooth(&mut vm);
    scroll_to(&mut vm, &gui, 100);
    assert_eq!(y_of(&vm, "Page"), -100);
    // A swipe up of a third, a half and nine tenths of a notch (positive: away from the user, the content moves down).
    for (part, y) in [(0.3, -91), (0.5, -85), (0.9, -73)] {
        wheel_part(&mut vm, &gui, part).unwrap();
        assert_eq!(y_of(&vm, "Page"), y, "part {part}");
    }
    // 1.2 notches in all: the window posts the whole one and the part falls back to 0.2.
    wheel(&mut vm, &gui, true);
    assert_eq!(y_of(&vm, "Page"), -43);
    wheel_part(&mut vm, &gui, 0.2).unwrap();
    // The 1.2 notches moved the content 36 pixels, what 1.2 rows are.
    assert_eq!(y_of(&vm, "Page"), -100 + 36);
}

#[test]
fn parts_of_a_notch_do_nothing_while_smooth_scrolling_is_off() {
    let (mut vm, gui) = scrolling();
    scroll_to(&mut vm, &gui, 100);
    wheel_part(&mut vm, &gui, 0.5).unwrap();
    assert_eq!(y_of(&vm, "Page"), -100);
}

#[test]
fn a_part_the_content_could_not_move_by_is_not_owed_back() {
    let (mut vm, gui) = scrolling();
    smooth(&mut vm);
    // At the top, a swipe up moves nothing; the swipe back down then moves from where the content is.
    wheel_part(&mut vm, &gui, 0.6).unwrap();
    assert_eq!(y_of(&vm, "Page"), 0);
    wheel_part(&mut vm, &gui, -0.5).unwrap();
    assert_eq!(y_of(&vm, "Page"), -15);
}

#[test]
fn a_glide_is_forgotten_with_its_control() {
    let (mut vm, gui) = scrolling();
    smooth(&mut vm);
    wheel(&mut vm, &gui, false);
    vm.eval(b"Scroll.delete();").unwrap();
    assert!(!any_gliding(&gui, vm.sim_time()));
}
