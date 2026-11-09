//! The host of client scripts: Lua that a server sends, run on a player's client in a **realm**, a Luau state inside a
//! WebAssembly guest (`torque_lua_client.wasm`), one per server connection. The guest's ops and the host's events follow
//! `torque-client-script-format`, whose docs are the specification; this crate keeps that format's host contracts.
//!
//! A [`Session`] is one realm: made when the player allows the server's scripts, closed on disconnect. It hands the
//! guest the server's verified script files ([`ServerContent`]), then one entry at a time: a frame's events in, a flush
//! of ops out. Every op is checked against the format and the realm's tiers ([`GrantSet`]) before the engine's
//! [`Effects`] apply it; one that a checked guest never sends stops the realm. A [`Runner`] calls the guest: natively a
//! [`NativeRealm`] under Wasmtime (the `wasmtime` feature), on the web a Worker (`web/client-script.js`, through the
//! `web` feature's `WebRunner`).
//!
//! **Stops.** A realm stops (its instance thrown away, everything it did removed, the player told, nothing of it run
//! again) when an entry runs past its deadline (250 ms natively by Wasmtime's epoch, 1 s on the web by the Worker's
//! watchdog), when its linear memory would grow past its cap, on any other trap, on an op a checked guest never sends,
//! and on 3 strikes in 10 s (the strike rule): a callback stopped at its step or memory limit, or an entry that ran
//! past 25 ms. A frame's callbacks get a frame's budget of Luau (20,000 steps, about 2 ms), so a realm at its caps
//! every frame is struck off rather than holding the frame rate down; a file run gets 100,000 (about 11 ms), and a
//! server with a bigger setup spreads it over frames.
//!
//! **Budgets.** A realm's work in a frame (natively its entries and the applying of their ops; on the web only the
//! applying) is held to `min(2 ms, 12%)` of the frame (1 ms of applying on the web); past it the rest waits for the next
//! frame and repeated input events keep only the newest. Over budget in 30 of the last 60 frames, it runs every other
//! frame and [`Session::is_slow`] says so.
//!
//! **Tiers.** A tier is granted only when its server asked, the player allowed it, its host module is in this build
//! (a cargo feature each: `ui`, `net`) and a review passed it ([`REVIEWED_TIERS`]). Revoking one removes its effects in
//! the same frame; the guest is told in its next frame, and an op of that tier from then on stops the realm.
//!
//! The crate is in no default plugin set.
//!
//! The older runner here, [`WasmtimeLuaModule`], runs the Lua extension's own WebAssembly module as the engine's
//! `web_lua::LuaModule` (the justfile's `realm-gate` measures it); it shares this crate's WASI imports.
//!
//! # Maintaining
//!
//! **Map.** `session` the realm's state machine (`Core`, and `Session`, which owns the engine's [`Effects`]); `flush`
//! the check of a flush; `events` the host-to-guest queue; `pace` the strike and slow rules; `grants`, `content`,
//! `effects`, `settings`; `runner` the runner contract; `native` the Wasmtime runner; `wasi` the WASI imports (shared
//! with `guest`, the Lua module's runner); `web` the Worker runner over `web/client-script*.js`; `cases` the shared cases.
//!
//! **Data flow.** `Session::open` queues the startup (new, each script, each mod's entry run); each `frame` collects the
//! running entry's outcome (a long one is a strike), checks its flush whole (`flush::check`, the GUI's `check_ui`
//! included), counts its limit stops, applies its `ui` pieces then the rest within the budget, then starts the next
//! entry, a frame entry carrying `events::take_batch` last. Natively an entry ends inside `start`; on the web on a poll.
//!
//! **Invariants** (and the tests that hold them):
//! - Only WASI imports, so nothing re-enters the guest: `a_guest_importing_anything_but_wasi_does_not_link`. A trap, a
//!   violation or the strike rule throws the instance away: `after_a_trap_the_instance_is_thrown_away_and_runs_nothing`,
//!   `a_violation_stops_the_realm_and_throws_its_instance_away`; a late web reply cannot undo a trap: `web/client-script.test.mjs`.
//! - The deadline stops builtins the steps barely see within 2x: `the_deadline_stops_*`. Memory traps stop one realm:
//!   `a_guest_past_its_memory_traps_and_only_it_stops`. The clock is the instance's: `os_clock_is_the_instances_own`;
//!   NaN prints alike: `nan_prints_the_same_on_every_runner`.
//! - A frame's callbacks get 20,000 steps, a file run 100,000 (`a_file_run_gets_its_own_steps_and_a_frames_callbacks_the_frames`
//!   in the guest's suite); a stop or an entry past 25 ms is a strike: `a_guest_at_its_step_caps_every_frame_is_struck_off`,
//!   `a_guest_holding_its_entries_past_the_long_entry_time_is_struck_off`, `three_limit_stops_in_ten_seconds_turn_the_realm_off`.
//! - A flush is checked whole, `ui` included, before anything applies; its `net` waits for its `ui`:
//!   `malformed_records_unknown_tiers_and_kinds_are_violations`, `a_flush_whose_ui_is_refused_sends_nothing`,
//!   `a_flush_whose_ui_fails_to_apply_sends_nothing`, `calls_keep_their_order_names_and_caps`.
//! - Grants and revocation: `a_request_gets_only_reviewed_tiers_the_player_allowed`, `an_op_of_an_ungranted_tier_stops_the_realm`,
//!   `revoking_removes_the_tier_in_the_same_frame`, `an_op_of_a_revoked_tier_stops_the_realm_once_it_was_told`.
//! - Closing or dropping leaves nothing: `closing_leaves_no_instance_and_nothing_applied`, `dropping_a_session_tears_it_down`.
//!   Budgets: `a_realm_over_budget_too_often_runs_every_other_frame`, `applying_past_the_web_budget_waits_for_the_next_frame`.
//! - Scripts come only from the server's list; batches always decode: `scripts_come_only_from_the_servers_list_within_the_caps`,
//!   `events_keep_the_newest_state_and_drop_input_before_messages`. `unreviewed-tiers` needs debug assertions (`compile_error!`).
//! - The Lua module's runner (`guest`) checks a text a request names against the C ABI's cap for it before it copies it
//!   out of the module; one past it is `TORQUE_ERR_RANGE` and reaches no host:
//!   `texts_past_the_abi_s_caps_are_refused_before_they_are_copied` (`web/lua.js` keeps the same caps: `web/lua.test.mjs`).
//! - The guest is checked against the build's list of accepted hashes ([`ACCEPTED_CLIENT_GUESTS`]) before it is compiled
//!   (`RealmGuest::new`, `WebRunner::open` for a guest it is given, `web/client-script.js` for the page's own):
//!   `a_guest_runs_only_if_its_hash_is_listed`, `a_guest_off_the_list_is_refused_before_it_is_compiled`, and
//!   `a guest runs only if its hash is on the list the build gives` in `web/client-script.test.mjs`. A release lists the hash of
//!   the guest `just lua-client-web` built for it (the justfile's `client-script-guest-hash` prints the line); the list is empty until a
//!   release does, so no build runs a server's scripts before that. `RealmGuest::unlisted` is for the tests' own guests.
//!
//! **Extending.** A tier: the format's ops first, then a cargo feature and its bit in [`HOST_TIERS`], its arm in
//! `flush::check` and an `Action`, an [`Effects`] method and its `revoke` handling, its cases; it joins [`REVIEWED_TIERS`]
//! only after its review. A runner: implement [`Runner`] and the `cases` crate feature's `Runners`; pass every case.
//!
//! **Checks.** The justfile's `client-script-host-test` (unit tests, the cases under Wasmtime, clippy, formatting) and
//! `client-script-web-check` (the runner's Node tests, then the cases under Node and headless Chrome).

// Tiers granted before their review exist only in builds for tests and for working on a tier.
#[cfg(all(feature = "unreviewed-tiers", not(debug_assertions)))]
compile_error!(
    "`unreviewed-tiers` grants tiers no review passed: it is for builds with debug assertions only"
);

mod content;
mod effects;
mod events;
mod flush;
mod grants;
mod guests;
mod pace;
mod runner;
mod session;
mod settings;

#[cfg(all(feature = "wasmtime", not(target_arch = "wasm32")))]
mod guest;
#[cfg(all(feature = "wasmtime", not(target_arch = "wasm32")))]
mod native;
#[cfg(all(feature = "wasmtime", not(target_arch = "wasm32")))]
mod wasi;
#[cfg(all(feature = "web", target_arch = "wasm32"))]
mod web;

#[cfg(feature = "cases")]
pub mod cases;
#[cfg(all(test, feature = "unreviewed-tiers", feature = "ui", feature = "net"))]
mod tests;

pub use content::{ClientMod, Refused, ServerContent};
pub use effects::{CallRoute, Effects, LogLevel, Refusal, Stop, Violation};
pub use grants::{GrantSet, HOST_TIERS, REVIEWED_TIERS, unknown_tiers};
pub use guests::{
    ACCEPTED_CLIENT_GUESTS, GuestHash, GuestRejected, accepted_hex, check_against, check_guest,
    guest_hash, guest_hex,
};
pub use runner::{Entry, InstanceLimits, Outcome, Reply, Runner, Trap, Unstarted};
pub use session::{FrameReport, Session};
pub use settings::{
    FRAME_STEPS, FrameBudget, GUEST_OVERHEAD, RUN_STEPS, SLOW_WINDOW, STEP_NANOS, Settings,
    StrikeRule,
};

#[cfg(all(feature = "wasmtime", not(target_arch = "wasm32")))]
pub use guest::{Counters, GuestEngine, GuestLimits, LuaGuest, WasmtimeLuaModule, is_deadline};
#[cfg(all(feature = "wasmtime", not(target_arch = "wasm32")))]
pub use native::{NativeRealm, RealmGuest};
#[cfg(all(feature = "web", target_arch = "wasm32"))]
pub use web::{WebRunner, web_live};
