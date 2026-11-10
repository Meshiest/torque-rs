//! The default consent prompt: what it ignores, what answers it, and that nothing under it hears the player.

use super::*;
use crate::client_script::prompt::Spot;
use crate::client_script::prompt_spots;
use crate::client_script::{MAX_LINE, MAX_ROWS, PromptAnswer, PromptRow, PromptSpec, RowLock};

fn row(key: &str, on: bool, lock: RowLock) -> PromptRow {
    PromptRow {
        key: key.to_string(),
        label: format!("Label of {key}"),
        on,
        lock,
    }
}

fn spec() -> PromptSpec {
    PromptSpec {
        heading: "A Server (10.0.0.1:28000)".into(),
        detail: "10.0.0.1:28000".into(),
        rows: vec![
            row("client_script.ability.ui", true, RowLock::Free),
            row("client_script.ability.net", true, RowLock::Free),
            row("client_script.ability.draw", false, RowLock::Locked),
        ],
        allow: "Allow".into(),
        not_now: "Not now".into(),
        never: "Never for this server".into(),
        remember: "Remember".into(),
        remember_on: true,
    }
}

/// The center of `spot` in the layout of a prompt with `rows` rows on the rig's canvas.
fn center(spot: Spot, rows: usize) -> (i32, i32) {
    let rect = prompt_spots((640, 480), rows)
        .into_iter()
        .find(|(s, _)| *s == spot)
        .map(|(_, r)| r)
        .expect("the spot");
    (rect[0] + rect[2] / 2, rect[1] + rect[3] / 2)
}

fn shown(rig: &mut Rig) -> ObjId {
    let id = rig
        .gui
        .show_client_script_prompt(&mut rig.vm, spec())
        .expect("the prompt shows");
    rig.frame();
    id
}

fn click(rig: &mut Rig, at: (i32, i32)) {
    rig.click_at(at.0, at.1);
}

#[test]
fn client_script_prompt_ignores_the_pointer_for_its_first_half_second() {
    let mut rig = Rig::new(&[]);
    let id = shown(&mut rig);
    click(&mut rig, center(Spot::Allow, 3));
    assert_eq!(rig.gui.take_client_script_prompt(&mut rig.vm, id), None);
    rig.vm.advance_time(499);
    click(&mut rig, center(Spot::Allow, 3));
    assert_eq!(rig.gui.take_client_script_prompt(&mut rig.vm, id), None);
    rig.vm.advance_time(1);
    click(&mut rig, center(Spot::Allow, 3));
    assert!(matches!(
        rig.gui.take_client_script_prompt(&mut rig.vm, id),
        Some(PromptAnswer::Allow { .. })
    ));
    assert!(!rig.gui.client_script_prompt_up(&rig.vm, id), "taken down");
}

#[test]
fn client_script_prompt_answers_with_the_rows_as_the_player_left_them() {
    let mut rig = Rig::new(&[]);
    let id = shown(&mut rig);
    rig.vm.advance_time(600);
    // Clear the first row, try the locked one, clear "remember".
    click(&mut rig, center(Spot::Row(0), 3));
    click(&mut rig, center(Spot::Row(2), 3));
    click(&mut rig, center(Spot::Remember, 3));
    click(&mut rig, center(Spot::Allow, 3));
    let Some(PromptAnswer::Allow { rows, remember }) =
        rig.gui.take_client_script_prompt(&mut rig.vm, id)
    else {
        panic!("allowed");
    };
    assert!(!remember);
    assert_eq!(
        rows,
        [
            ("client_script.ability.ui".to_string(), false),
            ("client_script.ability.net".to_string(), true),
            ("client_script.ability.draw".to_string(), false),
        ],
        "a locked row stays as it was given"
    );
}

#[test]
fn client_script_prompt_needs_the_press_and_the_release_on_the_same_spot() {
    let mut rig = Rig::new(&[]);
    let id = shown(&mut rig);
    rig.vm.advance_time(600);
    let allow = center(Spot::Allow, 3);
    rig.move_to(allow.0, allow.1);
    assert!(rig.button(SI_MAKE));
    let never = center(Spot::Never, 3);
    rig.move_to(never.0, never.1);
    rig.button(SI_BREAK);
    assert_eq!(
        rig.gui.take_client_script_prompt(&mut rig.vm, id),
        None,
        "pressed on Allow, released on Never"
    );
    // A press off the box and a release on a button is nothing either.
    rig.move_to(2, 2);
    assert!(rig.button(SI_MAKE), "the layer takes a press anywhere");
    rig.move_to(allow.0, allow.1);
    rig.button(SI_BREAK);
    assert_eq!(rig.gui.take_client_script_prompt(&mut rig.vm, id), None);
}

#[test]
fn client_script_prompt_no_key_answers_it() {
    let mut rig = Rig::new(&[]);
    let id = shown(&mut rig);
    rig.vm.advance_time(600);
    for (key, ascii) in [(KEY_RETURN, 13), (KEY_ESCAPE, 27), (89, u16::from(b'y'))] {
        rig.key(key, ascii, SI_MAKE, 0);
        rig.key(key, ascii, SI_BREAK, 0);
    }
    assert_eq!(rig.gui.take_client_script_prompt(&mut rig.vm, id), None);
    assert!(rig.gui.client_script_prompt_up(&rig.vm, id));
}

#[test]
fn client_script_prompt_gives_not_now_and_never() {
    let mut rig = Rig::new(&[]);
    for (spot, expected) in [
        (Spot::NotNow, PromptAnswer::NotNow),
        (Spot::Never, PromptAnswer::Never),
    ] {
        let id = shown(&mut rig);
        rig.vm.advance_time(600);
        click(&mut rig, center(spot, 3));
        assert_eq!(
            rig.gui.take_client_script_prompt(&mut rig.vm, id),
            Some(expected)
        );
    }
}

#[test]
fn client_script_prompt_text_is_plain_and_the_rows_are_bounded() {
    let mut rig = Rig::new(&[]);
    let mut loud = spec();
    loud.heading = format!("Evil\u{202e}\nServer {}", "x".repeat(1000));
    loud.rows = (0..40)
        .map(|n| row(&format!("k{}", n % 20), true, RowLock::Free))
        .collect();
    let id = rig
        .gui
        .show_client_script_prompt(&mut rig.vm, loud)
        .expect("shows");
    let (rows, heading) = rig
        .gui
        .with_custom_state::<crate::client_script::prompt::PromptState, _>(id, |st| {
            (st.row_count(), st.heading())
        })
        .expect("state");
    assert!(rows <= MAX_ROWS, "no more rows than the cap");
    assert_eq!(rows, MAX_ROWS.min(20), "a key counts once");
    assert!(!heading.contains('\n') && !heading.contains('\u{202e}'));
    assert!(heading.chars().count() <= MAX_LINE);
}

#[test]
fn client_script_the_server_layer_hears_nothing_while_the_prompt_is_up() {
    let mut rig = Rig::new(&[Tier::Ui]);
    rig.apply_ok(&button_ops());
    rig.click_at(20, 20);
    assert!(
        !rig.events().is_empty(),
        "the button works without a prompt"
    );
    let id = shown(&mut rig);
    rig.vm.advance_time(600);
    rig.click_at(20, 20);
    assert!(rig.events().is_empty(), "the prompt is over the realm");
    assert!(rig.gui.client_script_prompt_up(&rig.vm, id));
    rig.gui.close_client_script_prompt(&mut rig.vm, id);
    assert!(!rig.gui.client_script_prompt_up(&rig.vm, id));
    rig.click_at(20, 20);
    assert!(
        !rig.events().is_empty(),
        "and the realm hears again after it"
    );
}

fn cursor_on(rig: &Rig) -> bool {
    rig.gui.0.borrow().cursor_on
}

#[test]
fn client_script_a_mouse_look_client_can_answer_the_prompt_and_gets_its_cursor_back() {
    let mut rig = Rig::new(&[]);
    rig.game("Canvas.cursorOff();");
    assert!(!cursor_on(&rig), "a world played with the mouse");
    let id = shown(&mut rig);
    assert!(cursor_on(&rig), "the prompt turns the cursor on");
    // The world goes on turning the cursor off as it starts (a call of the game's own): the prompt is still answerable.
    rig.game("Canvas.cursorOff();");
    assert!(cursor_on(&rig), "and keeps it on");
    // The half second of ignoring the pointer still holds with the cursor on.
    click(&mut rig, center(Spot::Allow, 3));
    assert_eq!(rig.gui.take_client_script_prompt(&mut rig.vm, id), None);
    rig.vm.advance_time(600);
    click(&mut rig, center(Spot::Allow, 3));
    assert!(matches!(
        rig.gui.take_client_script_prompt(&mut rig.vm, id),
        Some(PromptAnswer::Allow { .. })
    ));
    assert!(!cursor_on(&rig), "the cursor is back as the game had it");
}

#[test]
fn client_script_a_prompt_taken_down_unanswered_puts_the_cursor_back_and_the_games_own_call_stands()
{
    let mut rig = Rig::new(&[]);
    rig.game("Canvas.cursorOff();");
    let id = shown(&mut rig);
    assert!(cursor_on(&rig));
    rig.gui.close_client_script_prompt(&mut rig.vm, id);
    assert!(!cursor_on(&rig), "taken down without an answer");
    // A game that turns the cursor on itself while the prompt is up keeps it on.
    let id = shown(&mut rig);
    rig.game("Canvas.cursorOn();");
    rig.gui.close_client_script_prompt(&mut rig.vm, id);
    assert!(cursor_on(&rig), "the game's own call is the game's");
}

#[test]
fn client_script_a_prompt_deleted_without_closing_it_gives_the_cursor_back() {
    let mut rig = Rig::new(&[]);
    rig.game("Canvas.cursorOff();");
    let id = shown(&mut rig);
    assert!(cursor_on(&rig));
    let _ = rig.vm.delete_object_quiet(id);
    rig.frame();
    assert!(!cursor_on(&rig), "the prompt took its hold with it");
    // A frame later it is still the game's: nothing holds the cursor on any more.
    rig.frame();
    assert!(!cursor_on(&rig));
}
