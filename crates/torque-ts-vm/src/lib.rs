//! The TorqueScript runtime: the console's variables, namespaces, objects, natives and events, with a pluggable
//! executor for a dialect's opcode loop.
//!
//! Follows Torque3D's MIT-licensed `console/` and `console/torquescript/` (Copyright (c) 2012 GarageGames, LLC, and
//! later holders where a file names them); the code here is this crate's own.
//!
//! # Maintaining
//!
//! Rules a change must keep, each guarded by a test:
//! - A native's and a global's effects ([`effects`]) only grow, and a name's are every namespace's
//!   (`a_names_effects_only_grow_and_cover_every_namespace`, `a_globals_effects_come_from_every_pattern_that_names_it`).
//! - `export` never writes a partial statement: a value too long for a line goes in whole statements of at most a line each,
//!   and reads back as it was (`export_never_writes_an_unterminated_statement`,
//!   `two_long_prefs_never_run_as_script_when_read_back`).
//! - TorqueScript's own extensions are never another language's, and an extension is one language's
//!   (`torque_script_s_extensions_are_never_taken_and_others_only_once`).
//! - A variable's name ends at its first NUL byte, for its key and the spelling it keeps alike, and a global pattern
//!   reads a name the same way (`a_name_is_the_same_variable_up_to_its_nul`,
//!   `a_globals_effects_come_from_every_pattern_that_names_it`).
//! - [`vm::Vm::invoke`] prints at most [`vm::MAX_UNKNOWN_COMMAND_LINES`] "Unknown command" lines a second of the simulation's
//!   time for each source ([`vm::Vm::invoke_from`]: a connection on a server, the server on a client), and the clock prints
//!   how many each left out once its second is over
//!   (`a_flood_of_unknown_commands_prints_a_few_lines_a_second_and_how_many_it_left_out`,
//!   `each_source_has_its_own_unknown_command_lines_and_a_timer_reports_what_it_left_out`).
//! - [`vm::Vm::command_source`] is the innermost remote party's command in progress, kept through the host's own calls inside
//!   it and restored when it ends (`a_remote_partys_command_is_the_running_source_through_the_hosts_own_calls_inside_it`);
//!   torque-client's `commandToServer` reads it on a script engine a server shares.
//! - The step budget is a time budget: an operation that copies or scans a value it leaves on the stack (a save, a
//!   variable or field array name, an object lookup, a `foreach$`'s text, an `_NP` jump, the classic executor's every
//!   read of the current string) is charged its bytes each time (`chained_assignments_are_charged_by_their_bytes` in both
//!   executors' tests, `a_save_loop_on_a_long_string_meets_the_step_limit`,
//!   `an_np_jump_loop_on_a_long_string_meets_the_step_limit`, `an_object_lookup_loop_on_a_long_string_meets_the_step_limit`,
//!   `a_word_loop_charges_the_text_it_copies`); a
//!   function's declared parameter count is at most the register cap (4096 a frame) and charged
//!   (`a_huge_declared_parameter_count_is_refused`).
//! - A native's work is linear in its arguments or charged as it goes: a set a script passes is tested through
//!   [`vm::words::ByteSet`] (`strip_chars_and_next_token_are_linear_in_their_arguments`), `StripMLControlChars` checks
//!   the budget after each scan (`strip_ml_scans_stop_at_the_step_limit`), and `echo`, `warn` and `error` build at
//!   most one log line (`echo_joins_at_most_one_log_line`).
//! - What scripts keep counts in the `Vm`'s [`memory::MemoryAccount`]: variables, dynamic fields, objects with their
//!   names, internal names and static texts, `foreach$` texts, loaded code (an `eval`'s functions keep theirs alive), and
//!   a class's own state through a [`memory::MemoryCharge`] (a MessageVector's lines). A store past the limit
//!   ([`policy::Limits::max_script_bytes`], at most [`memory::MAX_SCRIPT_BYTES`], which the engine-effect pref
//!   [`memory::MEMORY_LIMIT_PREF`] can lower and never raise) is not made and ends the run with
//!   [`vm::VmError::MemoryLimit`], so what is held stays under the limit; a buffer grows to exactly what it was admitted
//!   for, and a dictionary's spare entries are not counted (`global_and_field_growth_stops_at_the_memory_limit`,
//!   `global_and_local_growth_stops_at_the_memory_limit`, `loop_texts_names_and_code_count_in_the_memory_limit`).
//! - Scripts write at most [`policy::FileRules::write_quota`] to files in each hour of simulation time
//!   ([`policy::WRITE_WINDOW_MS`]): bytes per file and in all, and files opened, through `writeLine`, `export` (the
//!   engine's prefs too) and `fileCopy` alike (`writes_stop_at_the_write_quota`).
//! - Every `.dso` `exec` loads, with or without its source, is refused over [`vm::MAX_DSO_BYTES`] before it is parsed
//!   (`an_oversized_dso_is_refused`), and a removal of a freed tag never changes its counts, whatever [`tags::TagRules`]
//!   say (`removing_a_dead_tag_twice_keeps_counts`).
//! - A retiring datablock ([`sim::Objects::retire_datablock`]: a host's reference-counted removal) is found by no new lookup
//!   that gives it a user: a datablock-typed field write resolves neither its name nor its id (`VmEnv::find_object`),
//!   `datablock Class(name)` of its name makes a new datablock (`find_declared_datablock`), and the id leaves the set with the
//!   object (`Objects::unregister`), so the set is bounded by the datablock id range
//!   (`a_retiring_datablock_is_found_by_no_field_write_and_its_name_declares_a_new_one`). Datablock ids are never reused
//!   inside a mission: `next_datablock_id` only grows and only `deleteDataBlocks` resets it.
//! - No input panics the runtime, and every case's time stays within 5 microseconds a step of its runs' steps: random
//!   programs, their bytecode mutated, strings up to the size cap through what copies or scans them, and every native
//!   called from script with hostile arguments, in both executors (`tests/fuzz.rs`:
//!   `random_programs_run_through_both_executors`, `mutated_code_never_panics_the_executor`,
//!   `long_strings_stay_in_proportion`, `every_native_survives_hostile_arguments`; a server's natives in torque-app's
//!   `the_servers_natives_survive_hostile_arguments`). `TORQUE_FUZZ_VM=<rounds>` runs a long sweep (every long-string
//!   case at the cap, 1 MiB arguments, every server native), `TORQUE_FUZZ_SEED` another seed.
//! - [`host::FileHost::read_prefix`] keeps `read`'s refusals; a host over real files reads only the prefix
//!   (`a_prefix_read_gives_the_first_bytes_of_a_file_or_an_entry_with_the_reads_refusals`, in `torque-resource`).
//! - `exec` remembers the files it ran with their change times and the file whose code declared each datablock
//!   ([`vm::Vm::script_file`], [`vm::Vm::script_files_declaring_datablocks`], [`vm::Vm::datablocks_declared_by`]): at most
//!   [`vm::MAX_TRACKED_SCRIPT_FILES`] files and one entry per datablock id, both cleared by `deleteDataBlocks`.
//!   [`vm::Vm::exec_file_checked`] runs a file for a host that reloads it: the source, compiled once, and nothing of a source
//!   with a syntax error; [`vm::Vm::trace_new_objects`] lists what a run registered, up to [`vm::MAX_TRACED_OBJECTS`]
//!   (torque-server's `tests/datablock_watch.rs`).
//!
//! **Test helpers.** [`testing`] (feature `testing`, a dev-dependency only: no normal build enables it) holds the script test
//! helpers every crate's tests share (`eval`, `try_eval`, `global`, `console`, `log_lines`); each reads bytes lossily. A new
//! helper goes there only when two crates' tests would write it.
//!
//! **Speed** (`just bench-script` prints the workloads below, for both executors; a change that alters what a script
//! prints changes a digest it prints):
//! - A call from the host into script allocates nothing but what the script's own work needs: `Executor::save`
//!   returns a plain [`RunMark`](exec::RunMark), `Vm::invoke_owned` and `call_method_owned` take the argument list
//!   without copying it and skip the result copy when it is dropped (a fired event). Benches `host-calls` and
//!   `schedule`.
//! - A number becomes text on the stack (`text::with_format_int`, `with_format_g`, `with_format_g9`), on the string
//!   stack and in a variable read, and prints what `format_int`, `format_g` and `format_g9` print
//!   (`the_stack_forms_print_what_the_allocating_ones_do`). Bench `numstr`.
//! - A dynamic field that exists is found by a name folded on the stack, written into its own buffer and read into the
//!   executor's, with an arrayed field's name built on the stack. Benches `fields` and `objects`.
//! - An object with one pending event holds no set, and the queue's maps use the id hasher
//!   (`an_objects_events_are_tracked_from_one_to_many_and_back`). Bench `schedule`.
//! - `exec` runs the compiler once for a source whose check pass the run pass would repeat
//!   (`exec_compiles_a_clean_source_once_and_a_broken_one_twice`). Benches `exec-files` and `eval`.

pub mod classes;
pub mod code;
pub mod convert;
pub mod effects;
pub mod events;
pub mod exec;
pub mod fields;
/// The matrix and quaternion maths of the vector functions (TGE's `QuatF`, `AngAxisF` and `MatrixF`), with their
/// rounding, for a host that keeps transforms: the same functions `MatrixCreate` and the `rotation` field use, so a host's
/// numbers are theirs.
pub mod geometry {
    /// A 4x4 matrix, row by row (as TGE's `MatrixF`: translation in the last column).
    pub use crate::vm::vectors::{Mat4, V3};
    pub use crate::vm::vectors::{
        axis_angle_from_quat, matrix_from_quat, quat_from_axis_angle, quat_from_matrix,
    };
}
pub mod host;
pub mod idhash;
pub mod listing;
pub mod memory;
pub mod namehash;
pub mod ns;
pub mod policy;
pub mod sim;
pub mod tags;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
/// The C-library behavior the runtime depends on (the `torque-ts-text` crate, shared with the compiler).
pub use torque_ts_text as text;
pub mod vars;
pub mod vm;
