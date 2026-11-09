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
//! **Layout.** The format's `flex` and `grid` kinds are containers. Their classes are a plugin's (`GuiFlexCtrl` and
//! `GuiGridCtrl` of the layout plugin, which this crate cannot depend on): the plugin registers a [`RealmLayout`] while the
//! engine is built, the GUI plugin hands it to each client's GUI, and the applier makes the container as an object of its class
//! with the checked props written to its fields as the words of CSS. A container, and the item props of any control, are the
//! layout's to place; the realm sees neither a state nor a look of a container (it is a transparent group to input and
//! drawing). Without a layout a container is a `ClientScriptGroup` that places nothing and the item props are not written.
//!
//! # Maintaining
//!
//! **Map.** `apply` is the realm's applier and teardown; `controls` the `ClientScript*` classes, their input handlers and hit
//! test; `paint` how each kind draws (and the kept text layouts); `layers` the layer state, the draw plan, the per-frame holds,
//! the chip's column and `effectively_visible`; `route` input before the canvas; `chip` the chip; `prompt` the consent prompt; `events` the events for the
//! realm and their encoding; `layout` the hook a layout plugin fills (`RealmLayout`); `props`, `rich` small helpers. The rest of the crate calls in at these places:
//! - drawing (`draw.rs`): `draw_canvas` takes `DrawPlan::of`; `Walk::server_layer` draws the layer, `focus_on_top`'s field
//!   again, `draw_tooltip`, then cuts the chip's column (`LayerState::column`) out of every quad the layer pushed, and keeps what
//!   it pushed (`LayerMemo`, in `DrawCache`) to push again in a frame whose `LayerKey` is the same (see **Kept draws**);
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
//! props to its fields and the rest to its `NodeState`. A container (`flex`, `grid`) is made as an object of the layout's class
//! with its props, and every control's item props, written as the words of CSS to the fields the layout names
//! (`apply::layout_fields`); it keeps its props in the applier's node, not in a `NodeState`. The player's input travels back: `route` finds the control, the class's
//! handler changes its state and queues a `UiEvent`, and the host takes the batch with `Gui::take_client_script_batch`.
//!
//! **Kept draws.** A realm that is not changing costs the canvas next to nothing: the server layer's batches are kept from the
//! frame that drew them and pushed again while the layer's key (`LayerKey`: `Gui::custom_stamp`, the extension's `layer_stamp`,
//! the canvas size, the chip's column, hover and focus, the caret's blink phase, the pointer while a tooltip shows, the fonts'
//! revision and the saved glyph color) is the same. The stamp is the one thing this module must keep: it changes with every
//! change to what a realm control draws from. Today those are `Realm::apply_flush`, a pointer event other than a move, a key, a
//! wheel notch, a plugin control's resize, a control made or removed (`custom.rs`), and `set_client_script_faces`. A new way to
//! change a control's state, layout, props or children (an input handler that writes a `NodeState`, a host call) must call
//! `Gui::touch_custom`, or a frame is drawn from what the control used to be. The control context does it for a behavior: a
//! bounds that `ControlCx::set_bounds_of` changes, a `resize_control`, `parent_resized_control`, `base_resize` or `call`, and
//! `Gui::control_data` all touch the stamp, so a container that lays its children out in `update` or `wake` invalidates the kept
//! draw by moving them, and only when it moves them (a layout pass that finds nothing to change leaves it). A behavior that
//! changes a field a draw reads through `ControlCx::vm` says so with `ControlCx::changed`. A layer is not kept when it drew what a later
//! frame may draw differently (a texture that did not load, a class not modelled, a view or world, text a frame's font quota
//! cut), or when the extension gives no `layer_stamp`. The layer's controls are otherwise drawn from the tree each frame, with
//! what a walk of a thousand controls costs: the extent a parent read is handed to the child, a control with no children is
//! not read for them, `control::point` and `control::flag` read the stored numbers, a control's props are a short list, and a
//! font is looked up once a frame for each face and size.
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
//! - A container never has more children than `MAX_CONTAINER_CHILDREN` or more containers around it than `MAX_CONTAINER_DEPTH`, on
//!   create and on `parent`, and a refused op changes nothing (the nodes count their containers, which `Realm::check` re-derives):
//!   `client_script_a_container_has_at_most_so_many_children`, `client_script_containers_nest_at_most_so_deep`,
//!   `client_script_applier_fuzz_of_layout_containers_100000_sequences` (and the same without a layout). A container takes no look prop,
//!   no value out of range and no track past the cap: `client_script_a_container_refuses_the_looks_it_lacks_and_hostile_values`.
//! - Without a layout a container is a transparent group that places nothing and writes no field, and with one it is invisible to
//!   drawing and input (it has no `NodeState`, so no hit and no quad):
//!   `client_script_without_a_layout_a_container_is_a_plain_group`, `client_script_a_container_is_transparent_to_input_and_adds_no_look`.
//!   The layout plugin's own tests drive the real classes (`torque-plugin-gui-layout`, `tests/realm.rs`).
//! - The caps hold, per control and in all: `client_script_the_caps_hold`, `client_script_images_and_text_are_counted_across_the_realm`.
//! - The realm's controls look as they did, and a frame drawn from kept draws is the list a cache that drew nothing draws, after
//!   every kind of change: `the_realm_controls_look_as_pinned` and `the_realm_containers_look_as_pinned` (a pinned hash of a software
//!   raster, one of each kind of control and a scene of laid-out containers; a change to how a control looks changes it on
//!   purpose, and its constant is then pinned again),
//!   `kept_draws_never_differ_from_drawing_from_nothing`, `kept_draws_of_text_with_no_font_say_so_again`,
//!   `kept_draws_of_laid_out_containers_never_differ_from_drawing_from_nothing`,
//!   `a_layout_that_moves_children_leaves_no_stale_kept_draws` (`tests/laid.rs`: a test layout that places children as a plugin's
//!   does), the fonts' and the extension's parts of the key (`picture::keys`), and the fuzzes
//!   `client_script_kept_draws_fuzz_never_differs_from_drawing_from_nothing` and its laid-out containers' twin (`tests/fuzz.rs`).
//!   A layer is let go when it is not drawn (`a_layer_that_is_not_drawn_keeps_nothing`).
//!
//! **Extending.** A new kind needs the format's kind and props first, then a class in `controls`, its name in `class_name` and
//! `add_controls`, a draw in `paint`, and its cases in the refusal and canary tests. A layout prop is a prop of the format first
//! (`ITEM` or a container's own), then a row in the layout plugin's `RealmLayout` table; the applier needs no change. The layout's
//! classes are not canvas children, so the GUI's frame does not reach them: `layers::frame` runs their `update` (`LayerState::laid`), and only
//! for two frames after something a layout reads changed (`layout_frames`, set by `mark_layout` for every op on a container or on a
//! control in one, and by a resize of the layer), so an idle realm does no layout work at all, which `Stats::layout_updates` and
//! `layout_work` count: `client_script_layouts_are_checked_only_after_a_change_inside_one`. The applier is the only writer of what a
//! layout reads; a new op that writes a container's fields or moves a control must call `mark_layout`. A new drawing hook goes in the list above.
//! The consent prompt is an engine-layer control: put it in with `Gui::push_engine_layer`; it takes the pointer and the keys
//! first and is modal, and the server layer takes none while it is up. Never give a realm control a way to run text: no field
//! writes by name, no `Vm::eval`, no script method calls.
//!
//! **Checking.** `cargo test -p torque-gui client_script`, `cargo clippy -p torque-gui --all-targets -- -D warnings` and
//! `cargo fmt -p torque-gui -- --check`; the applier fuzz is 100,000 sequences and runs in seconds. The cost of the frame, the
//! canvas's drawing of 1,024 controls included, is the justfile's `client-script-bench` (`TORQUE_PICTURE_DIR` makes the picture test write
//! its raster as a PNG there).

mod apply;
mod chip;
mod controls;
mod events;
mod layers;
mod layout;
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
pub use layout::{RealmLayout, RealmLayoutEntry};
pub(crate) use paint::{caret_on, draw_tooltip, tooltip_text};
pub use prompt::{
    IGNORE_MS, MAX_LINE, MAX_ROWS, PromptAnswer, PromptRow, PromptSpec, PromptSpot, RowLock,
    prompt_spots,
};
pub(crate) use route::{after_canvas, route};

/// Adds the `ClientScript*` control classes to an engine being built.
pub fn add_controls(builder: &mut EngineBuilder) {
    builder
        .add_gui_control::<controls::RootCtrl>()
        .add_gui_control::<controls::GroupCtrl>()
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
    let all = std::iter::once(controls::class_name(None))
        .chain(
            torque_client_script_format::ui::KINDS
                .iter()
                .map(|k| controls::class_name(Some(k.kind))),
        )
        .chain([chip::CLASS, prompt::CLASS]);
    // The two container kinds share one class (the group made when no layout is installed): each name once, in order.
    let mut names: Vec<&'static str> = Vec::new();
    for name in all {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}
