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
//! - [`host::FileHost::read_prefix`] keeps `read`'s refusals; a host over real files reads only the prefix
//!   (`a_prefix_read_gives_the_first_bytes_of_a_file_or_an_entry_with_the_reads_refusals`, in `torque-resource`).
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
pub mod namehash;
pub mod ns;
pub mod policy;
pub mod sim;
pub mod tags;
/// The C-library behavior the runtime depends on (the `torque-ts-text` crate, shared with the compiler).
pub use torque_ts_text as text;
pub mod vars;
pub mod vm;
