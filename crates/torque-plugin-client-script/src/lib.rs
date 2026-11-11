//! Server scripts for a game: whether a player allows the Lua a server sends, which abilities, and the messages between that Lua
//! and its server.
//!
//! The sandbox itself is `torque-client-script-host` (the realm) and the controls are `torque-gui`'s `client_script`; this crate is
//! the part a game fills in around them.
//!
//! - **Consent** ([`policy`], [`consent`], [`choices`], [`abilities`]). A server's request becomes a [`policy::Decision`]: no
//!   scripts, a realm, or a question. The global setting (ask, or never for a player who wants no server scripts), the per-ability policies
//!   (ask, always, never) and the player's remembered choice for that server decide. The question is a slot a game fills
//!   ([`consent::ConsentPrompt`]): it gets the abilities with stable keys (`client_script.ability.ui`) and English lines, and
//!   answers with the boxes ticked; [`consent::EnginePrompt`] is the plain one that ships. What is remembered lives in an
//!   engine-owned file ([`choices::Choices`]) that no script can reach.
//! - **Messages** ([`messages`], [`call`]). A realm's `net.send` and the server's values travel as plugin RPCs, a realm's
//!   `net.call` reaches only the server's RPCs marked [`realm`](torque_engine::rpc::RpcBuilder::realm), and every size and rate is
//!   capped where the bytes arrive.
//! - **The realms** ([`Realms`]). The state machine of one connection: offer, wait for the files, ask, start, run, end.
//! - **The server** ([`ClientScriptsPlugin`], [`server`]). The offer, the player's answers and messages, and the script functions.
//!
//! ```
//! use torque_client_script_host::REVIEWED_TIERS;
//! use torque_plugin_client_script::{Choices, RealmState, Realms, Setting};
//!
//! let mut realms = Realms::new(Choices::in_memory(), None);
//! assert_eq!(realms.setting, Setting::Ask, "a server that asks is asked about");
//! assert_eq!(realms.offerable.0 & !REVIEWED_TIERS.0, 0, "nothing unreviewed is offered");
//! realms.setting = Setting::Never;
//! assert_eq!(realms.state(), RealmState::Idle);
//! ```
//!
//! # Maintaining
//!
//! **Map.** `abilities` the tiers a player decides on (keys, English lines, defaults); `policy` the setting, policies and the
//! pure `decide`; `choices` the file; `consent` the prompt slot and the engine's prompt; `messages` the RPC messages, their
//! caps and the byte bucket; `call` what a realm's `net.call` may name and how its table becomes arguments; `inbox` what a
//! client holds for the realm; `effects` the recorder the host's session writes to and `Applying`, the effects of a frame,
//! which check a flush's `ui` records whole and apply its pieces as the session applies them; `realms` the state machine; `server` and
//! `plugin` the server's half and the plugin that registers all of it.
//!
//! **Rules and their tests.**
//! - The GUI's work for a realm is the realm's: applying its ui ops counts in its frame budget, its slow rule and its strikes,
//!   a flush past the budget waits for later frames, and the GUI's layouts and image decodes for it are charged to its next
//!   frames (`budget::applying_past_the_budget_waits_for_later_frames_and_makes_the_realm_slow`,
//!   `budget::applying_that_holds_the_frame_too_long_is_a_strike_and_turns_the_realm_off`,
//!   `budget::many_resizes_of_a_nested_container_are_laid_out_once_a_frame_and_charged_to_the_realm`); a flush is checked whole
//!   before any of it applies, and its `net` ops never leave when its `ui` ops are refused
//!   (`budget::a_flush_whose_last_op_is_forged_applies_nothing_and_sends_nothing`). A set costs the host what it carries,
//!   not the size of the control it changes (`budget::a_storm_of_selections_on_a_full_list_costs_what_the_sets_carry`).
//! - Shift+Escape ends the realm for the connection and remembers nothing; the chip's "Turn off for this server" remembers
//!   Never (`consent::shift_escape_ends_the_realm_for_the_connection_only_and_the_chip_remembers_never`).
//! - Before the player's answer nothing of the realm exists: no instance, no file read, no control, no guest started
//!   (`consent::no_realm_instance_and_no_file_read_before_allow`, `consent::not_now_and_never_start_nothing`).
//! - A refusal for good is remembered across a reconnect, with no prompt and no instance
//!   (`consent::never_is_remembered_across_a_reconnect_with_no_prompt_and_no_instance`).
//! - The grant set is the same for every setting, policy and remembered choice, in a table of every ability against every policy
//!   (`consent::grants_for_every_policy_and_remembered_choice`), and a tier no review passed is never granted
//!   (`consent::a_request_gets_only_the_reviewed_tiers_and_the_default_setting_asks`).
//! - The choices file is read and written by `Choices` alone: no pref, global, native, console function or file function of a
//!   script engine reaches it (`consent::the_choices_file_is_beyond_every_script_file_function`,
//!   `consent::no_script_reads_the_remembered_choices`). The path is the game's to choose and must be a folder only the player can
//!   write; a bad file is read as empty.
//! - Every size and rate of a message is checked where the bytes arrive, as hostile input
//!   (`messages::up_messages_past_every_cap_are_refused`, `messages::down_messages_past_every_cap_are_dropped`,
//!   `messages::the_server_holds_a_client_to_its_rate_and_to_the_byte_allowance`, and the fuzz `messages::message_decoding_never_panics_and_what_it_accepts_is_within_caps`).
//! - A realm calls only RPCs marked for it, never this crate's own, with arguments that fit the request
//!   (`messages::an_rpc_without_the_realm_mark_is_refused`, `messages::calls_are_refused_unless_they_fit_the_rpc`).
//! - The state is of one connection, keyed by the RPC layer's epoch and the server dialed: a different one starts at Idle, with
//!   the question taken down and the realm closed, even if no frame saw the old one end, and what an old connection left in the
//!   inbox is never given to the new (`consent::a_new_connection_always_starts_at_idle_even_if_no_frame_saw_the_old_one_end`).
//! - A connection's byte allowance is never reset by others arriving: past the tracked connections the stalest goes
//!   (`server::tests::past_the_tracked_connections_the_stalest_goes_and_nobody_gets_a_fresh_allowance`), and a TorqueScript handler
//!   gets no control character, a NUL included (`messages::tests::text_for_a_script_never_carries_a_control_character`).
//! - The server sees only declined, unavailable or the abilities granted, never why
//!   (`consent::the_server_learns_declined_unavailable_or_the_granted_abilities_only`).
//!
//! **Order of a frame** (`Realms::frame`): the connection's state, the server's offer, the state machine, then for a live realm
//! the inboxes into the session, the GUI's events and the chip's actions, the realm's frame, and the record applied: logs, the
//! `ui` pieces, then the `net` ops, so a flush whose `ui` is refused sends nothing.
//!
//! **Layout.** The `flex` and `grid` controls of a realm are laid out only if the game's engine has `GuiLayoutPlugin`
//! (`torque-plugin-gui-layout`); without it they are plain groups and a server's scripts keep working, with their children
//! where their own `pos` and `size` put them. The glue itself does nothing for layout.
//!
//! **Extending.** A tier needs its host module and review in `torque-client-script-host` first; its ability line is in
//! `abilities`. A new built-in message is an RPC in `plugin` with a type in `messages` and a cap test beside it.
//!
//! **Checks.** `cargo test -p torque-plugin-client-script` (the `consent` and `messages` test files hold the consent and message rules above),
//! `cargo clippy -p torque-plugin-client-script --all-targets -- -D warnings`, and `cargo test -p torque-mods client_script`.

pub mod abilities;
pub mod call;
pub mod choices;
pub mod consent;
mod effects;
mod inbox;
pub mod messages;
mod plugin;
pub mod policy;
mod realms;
pub mod server;

pub use choices::Choices;
pub use consent::{ConsentAnswer, ConsentPrompt, ConsentRequest, EnginePrompt};
pub use inbox::ScriptInbox;
pub use plugin::ClientScriptsPlugin;
pub use policy::{Decision, Policies, Policy, Remembered, Setting};
pub use realms::{ImageReader, ModsSource, OfferSource, RealmState, Realms, RunnerFactory, Stats};
