//! The GUI side of client scripts: the controls a server's scripts may make, the typed applier that makes them, and the host
//! layers they are shown in.
//!
//! **The controls** are the `ClientScript*` classes ([`add_controls`] gives an engine them): one class a kind of the op
//! format, and a root class for each mod. They are made only by [`Realm`]'s applier, as objects scripts can neither read nor
//! write, with no name, no group of the game's and no script callback. A control keeps what the realm set in its state, not in
//! console fields, so nothing of the game's reaches it: `command`, `altCommand`, `variable`, `accelerator`, `class`,
//! `superClass` and a console name stay empty by construction ([`Realm::check`] looks), and an event for the realm is a queued
//! value, never a script call.
//!
//! **The layers.** The server layer is a host layer, not a canvas child, so the game's `setContent`, `pushDialog` and
//! `popLayer` cannot move or remove it. It is drawn after the canvas content and before every dialog, and only over the
//! content screens the game names ([`crate::Gui::set_client_script_screens`]). The engine layer ([`crate::Gui::push_engine_layer`])
//! and the chip are drawn after every dialog and get input first. The chip says who runs scripts, shows what the realm holds,
//! and opens a menu of per-tier toggles, "Hide for now" and "Turn off for this server". The server layer is neither drawn nor
//! hit in the chip's column (the chip at its widest with its menu open, and a margin), so nothing the realm draws passes for it.
//!
//! **Input.** The realm gets pointer events on its own controls while the cursor is on and no dialog or prompt is up, and keys
//! only as text and navigation in its focused text field. The focus goes with any press off the field and whenever the cursor
//! goes off; a key's release is kept from the game only when the field took its press. Escape is the host's: with the realm
//! holding the cursor, a focus or the camera, the first press drops them and tells it `closed`, and `ui.interactive(true)` is
//! refused until the player acts on the realm's controls again (a dialog's drop is the same); Shift+Escape turns server scripts
//! off. `ui.focus` is let through for a second after the player's own action on a control of the realm that draws something.
//! An engine prompt is modal: while one is up its key responder gets every key but Escape, and the game none (only the
//! releases of the presses it had).
//!
//! **The consent prompt.** `Gui::show_client_script_prompt` shows the engine's default prompt (`prompt`): a modal box in the engine
//! layer with a row for each ability a server asks for, a "remember" box and three buttons. It ignores the pointer for
//! [`IGNORE_MS`] after it is shown, needs the press and the release on the same place, and takes no key (Enter does not allow;
//! Escape is the host's). It never draws or reads anything of the realm, which does not exist until the answer. A game may draw its
//! own prompt in the engine layer instead; the plugin crate's `ConsentPrompt` is the slot.
//!
//! Layout containers (`GuiFlexCtrl`, `GuiGridCtrl`) are not used: format 1 has no props for them, only positions, sizes and the
//! sizing rules of `GuiControl`.
//!
//! # Maintaining
//!
//! **Map.** `apply` is the realm's applier and teardown; `controls` the `ClientScript*` classes, their input handlers and hit
//! test; `paint` how each kind draws (and the kept text layouts); `layers` the layer state, the draw plan, the per-frame holds,
//! the chip's column and `effectively_visible`; `route` input before the canvas; `chip` the chip; `prompt` the consent prompt; `events` the events for the
//! realm and their encoding; `props`, `rich` small helpers. The rest of the crate calls in at these places:
//! - drawing (`draw.rs`): `draw_canvas` takes `DrawPlan::of`; `Walk::server_layer` draws the layer, `focus_on_top`'s field
//!   again, `draw_tooltip`, then cuts the chip's column (`LayerState::column`) out of every quad the layer pushed;
//!   `Walk::engine_layer` draws the prompts and the chip; each layer is recorded as a `LayerMark`; realm images are drawn by
//!   `DrawCtx::realm_image` from `RealmImage` bytes in `DrawCache`'s own realm textures; `Walk::custom_control` runs no
//!   `onRender` for a realm control (`is_realm`);
//! - input (`input.rs`): `process_input_event` calls `route` before the canvas and `after_canvas` after it; `route` gives a
//!   prompt's key responder `on_key_down` and `on_key_up_at`;
//! - the frame (`Gui::frame`) calls `frame`; `ControlClass::on_remove` calls `forget`; `control.rs` skips the game's callbacks
//!   for realm controls (`is_realm`); `canvas.rs`'s `cursorOn` and `cursorOff` count `GuiState::cursor_calls`.
//!
//! **An op's way.** The host gives `Realm::apply_flush` a flush; it is decoded record by record, the tier checked against the
//! grants, and a `ui` record read by its kind. Everything is checked before anything changes, so a refused op changes nothing.
//! A create or set merges the props, checks the rules between them and the caps, makes or finds the object, writes the layout
//! props to its fields and the rest to its `NodeState`. The player's input travels back: `route` finds the control, the class's
//! handler changes its state and queues a `UiEvent`, and the host takes the batch with `Gui::take_client_script_batch`.
//!
//! **Order.** Drawing: content, server layer, dialogs, engine layer, chip (`LayerMark` records it). Input: every Escape to the
//! Escape filter first; other keys to the top prompt's key responder while a prompt is up, else to the focused field, else the
//! canvas; the pointer to the chip, the prompts, the focused field, the server layer, then the canvas. The server layer gets
//! nothing while a dialog, a prompt or an unmarked screen is up.
//!
//! **Invariants and their guards** (run the commands below after any change here):
//! - No code field, name or script callback is reachable from a realm control: `client_script_a_realm_control_is_not_the_games`,
//!   `client_script_string_props_set_to_code_never_run`, `Realm::check` in `client_script_applier_fuzz_100000_sequences`.
//! - The game never moves the server layer: `client_script_game_screen_changes_never_move_the_server_layer`.
//! - Escape never reaches the realm: `client_script_escape_drops_the_holds_and_tells_the_realm`,
//!   `client_script_input_fuzz_never_gives_the_realm_an_escape_or_a_dialogs_input`.
//! - Focus only on a field that can be seen, and drawn and hit on top: `client_script_a_text_field_that_cannot_be_seen_takes_no_focus`,
//!   `client_script_a_field_that_stops_being_seen_loses_the_focus_that_frame`,
//!   `client_script_a_focused_field_is_drawn_and_hit_above_what_covers_it`.
//! - A text field is a leaf, so nothing of the realm's is drawn over it with it: `client_script_a_text_field_is_a_leaf`.
//! - The focus goes with a press off the field and with the cursor:
//!   `client_script_a_press_off_the_field_or_the_cursor_going_off_drops_the_focus`.
//! - A field's typing is drawn in the host's color: `client_script_a_fields_text_is_drawn_opaque_whatever_its_color`.
//! - Only a control that draws something earns the focus: `client_script_clicking_something_invisible_does_not_earn_the_focus`,
//!   `client_script_a_click_on_a_clear_panel_earns_no_focus`.
//! - The chip cannot be covered, faked or pushed out: `client_script_the_server_layer_is_neither_drawn_nor_hit_in_the_chips_column`,
//!   `client_script_a_field_in_the_chips_column_takes_no_focus`, `client_script_the_typing_marker_stays_inside_the_chip`.
//! - After Escape or a dialog the realm needs the player to take the cursor again:
//!   `client_script_after_escape_the_realm_waits_for_the_player_to_take_the_cursor`; giving it back never undoes the game's
//!   `cursorOn`/`cursorOff`: `client_script_giving_the_cursor_back_never_undoes_the_games_call`.
//! - No game key is left held: `client_script_a_game_keys_release_reaches_the_game_when_the_focus_came_after_its_press`;
//!   a prompt takes the keys, not Escape: `client_script_a_prompt_takes_the_keys_and_escape_is_the_hosts`.
//! - The consent prompt ignores the pointer for its first half second, needs press and release on one spot, takes no key, bounds its
//!   rows and text, and the realm under it hears nothing: `client_script_prompt_ignores_the_pointer_for_its_first_half_second`,
//!   `client_script_prompt_needs_the_press_and_the_release_on_the_same_spot`, `client_script_prompt_no_key_answers_it`,
//!   `client_script_prompt_text_is_plain_and_the_rows_are_bounded`, `client_script_the_server_layer_hears_nothing_while_the_prompt_is_up`.
//! - The content store fails closed, and images are drawn from its bytes alone, never the game's files:
//!   `client_script_images_are_refused_until_the_host_sets_a_content_check`, `client_script_images_come_from_the_servers_store_alone`.
//! - Text is measured once a word and its layout kept: `client_script_wrapped_text_is_measured_once_a_word_and_kept`.
//! - The caps hold, per control and in all: `client_script_the_caps_hold`, `client_script_images_and_text_are_counted_across_the_realm`.
//!
//! **Extending.** A new kind needs the format's kind and props first, then a class in `controls`, its name in `class_name` and
//! `add_controls`, a draw in `paint`, and its cases in the refusal and canary tests. A new drawing hook goes in the list above.
//! The consent prompt is an engine-layer control: put it in with `Gui::push_engine_layer`; it takes the pointer and the keys
//! first and is modal, and the server layer takes none while it is up. Never give a realm control a way to run text: no field
//! writes by name, no `Vm::eval`, no script method calls.
//!
//! **Checking.** `cargo test -p torque-gui client_script`, `cargo clippy -p torque-gui --all-targets -- -D warnings` and
//! `cargo fmt -p torque-gui -- --check`; the applier fuzz is 100,000 sequences and runs in seconds.

mod apply;
mod chip;
mod controls;
mod events;
mod layers;
mod paint;
mod prompt;
mod props;
mod rich;
mod route;
#[cfg(test)]
mod tests;

use torque_engine::EngineBuilder;

use crate::custom::GuiBuilderExt;

pub use apply::{Applied, ContentCheck, Forwarded, MAX_ROOTS, Realm, Violation};
pub use events::{Changed, UiEvent, encode_events};
pub use layers::{
    DEFAULT_FACES, DrawPlan, Hold, Holds, LayerAction, LayerMark, LayerState, MAX_ENGINE_LAYER,
    MAX_QUEUED_EVENTS, RealmView, Stats, TierToggle,
};
pub(crate) use layers::{focus_on_top, forget, frame};
pub(crate) use paint::draw_tooltip;
pub use prompt::{
    IGNORE_MS, MAX_LINE, MAX_ROWS, PromptAnswer, PromptRow, PromptSpec, PromptSpot, RowLock,
    prompt_spots,
};
pub(crate) use route::{after_canvas, route};

/// Adds the `ClientScript*` control classes to an engine being built.
pub fn add_controls(builder: &mut EngineBuilder) {
    builder
        .add_gui_control::<controls::RootCtrl>()
        .add_gui_control::<controls::PanelCtrl>()
        .add_gui_control::<controls::TextCtrl>()
        .add_gui_control::<controls::RichTextCtrl>()
        .add_gui_control::<controls::ImageCtrl>()
        .add_gui_control::<controls::ButtonCtrl>()
        .add_gui_control::<controls::ToggleCtrl>()
        .add_gui_control::<controls::TextFieldCtrl>()
        .add_gui_control::<controls::ListCtrl>()
        .add_gui_control::<controls::ScrollCtrl>()
        .add_gui_control::<controls::SliderCtrl>()
        .add_gui_control::<controls::ProgressCtrl>()
        .add_gui_control::<chip::ChipCtrl>()
        .add_gui_control::<prompt::PromptCtrl>();
}

/// The names of the classes [`add_controls`] adds.
pub fn class_names() -> Vec<&'static str> {
    std::iter::once(controls::class_name(None))
        .chain(
            torque_client_script_format::ui::KINDS
                .iter()
                .map(|k| controls::class_name(Some(k.kind))),
        )
        .chain([chip::CLASS, prompt::CLASS])
        .collect()
}
