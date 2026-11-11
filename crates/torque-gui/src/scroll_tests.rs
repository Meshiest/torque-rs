use torque_ts_vm::vm::Vm;

use super::*;
use crate::keys::KEY_TAB;
use crate::test_rig::{id, key, point_at, rig, scroll_body, y_of};

/// The page of ten rows in a scroll control that shows 100 pixels of its 400, and a control beside it.
fn paged() -> (Vm, Gui) {
    let (mut vm, gui) = unrevealing();
    vm.eval(b"setRevealFocus(1);").unwrap();
    (vm, gui)
}

/// The same with `$Pref::Gui::RevealFocus` at its default.
fn unrevealing() -> (Vm, Gui) {
    let body = format!(
        "{} new GuiControl(Beside) {{ profile = RowProfile; position = \"130 10\"; extent = \"50 30\"; visible = 1; }};",
        scroll_body()
    );
    rig(&body)
}

fn focus(vm: &mut Vm, name: &str) {
    vm.eval(format!("{name}.makeFirstResponder(1);").as_bytes())
        .unwrap();
}

#[test]
fn a_control_that_takes_the_focus_below_the_view_is_scrolled_into_it() {
    let (mut vm, _gui) = paged();
    assert_eq!(y_of(&vm, "Page"), 0);
    // Row8 is 320 to 350 of the page: the view's bottom comes to its bottom edge.
    focus(&mut vm, "Row8");
    assert_eq!(y_of(&vm, "Page"), -250);
    // And back up to the first row, which is above the view.
    focus(&mut vm, "Row0");
    assert_eq!(y_of(&vm, "Page"), 0);
}

#[test]
fn a_control_that_takes_the_focus_scrolls_nothing_until_the_pref_is_on() {
    let (mut vm, gui) = unrevealing();
    assert!(!crate::glide::reveal_enabled(&vm));
    focus(&mut vm, "Row8");
    assert_eq!(y_of(&vm, "Page"), 0);
    point_at(&mut vm, &gui, (150, 150));
    for _ in 0..4 {
        key(&mut vm, &gui, KEY_TAB);
    }
    assert_eq!(y_of(&vm, "Page"), 0);
    vm.eval(b"$Pref::Gui::RevealFocus = 1;").unwrap();
    focus(&mut vm, "Row8");
    assert_eq!(y_of(&vm, "Page"), -250);
    assert_eq!(vm.eval(b"return getRevealFocus();").unwrap(), b"1");
}

#[test]
fn a_control_already_in_view_leaves_the_content_where_it_is() {
    let (mut vm, _gui) = paged();
    focus(&mut vm, "Row1");
    assert_eq!(y_of(&vm, "Page"), 0);
    // And one outside every scroll control moves none.
    focus(&mut vm, "Row5");
    let at = y_of(&vm, "Page");
    focus(&mut vm, "Beside");
    assert_eq!(y_of(&vm, "Page"), at);
}

#[test]
fn tab_takes_the_focus_to_a_control_below_the_view_and_the_view_follows() {
    let (mut vm, gui) = paged();
    point_at(&mut vm, &gui, (150, 150));
    // Only the button is active, so Tab goes between it and the first row, where it wraps.
    let (row0, btn) = (id(&vm, "Row0"), id(&vm, "Btn"));
    let mut stops = Vec::new();
    for _ in 0..4 {
        key(&mut vm, &gui, KEY_TAB);
        stops.push((gui.first_responder(), y_of(&vm, "Page")));
    }
    // The button is 345 to 375 of the page: the view's bottom comes to it. The first row is in view at the top.
    let at = |f| stops.iter().find(|s| s.0 == Some(f)).map(|s| s.1);
    assert_eq!(at(btn), Some(-275), "{stops:?}");
    assert_eq!(at(row0), Some(0), "{stops:?}");
}

#[test]
fn a_click_on_a_partly_hidden_control_brings_all_of_it_into_view() {
    let (mut vm, gui) = paged();
    // The view shows 250 to 350 of the page; the button is 345 to 375, five pixels of it in view.
    let scroll = id(&vm, "Scroll");
    crate::scroll::scroll_to(&mut vm, &gui, scroll, 0, 250);
    point_at(&mut vm, &gui, (50, 108));
    crate::test_rig::press(&mut vm, &gui);
    assert_eq!(gui.first_responder(), Some(id(&vm, "Btn")));
    assert_eq!(y_of(&vm, "Page"), -275);
}

#[test]
fn with_smooth_scrolling_the_view_glides_to_the_control_and_the_place_is_final_at_once() {
    let (mut vm, gui) = paged();
    vm.eval(b"setSmoothScroll(1);").unwrap();
    focus(&mut vm, "Row8");
    assert_eq!(y_of(&vm, "Page"), -250);
    let scroll = id(&vm, "Scroll");
    assert_eq!(crate::glide::shift(&gui, scroll, vm.sim_time()), (0, 250));
    vm.advance_time(crate::glide::GLIDE_MS);
    assert_eq!(crate::glide::shift(&gui, scroll, vm.sim_time()), (0, 0));
}

#[test]
fn without_smooth_scrolling_the_view_moves_at_once() {
    let (mut vm, gui) = paged();
    focus(&mut vm, "Row8");
    assert_eq!(
        crate::glide::shift(&gui, id(&vm, "Scroll"), vm.sim_time()),
        (0, 0)
    );
}
