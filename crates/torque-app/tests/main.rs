//! The integration tests, linked as one test program: one module per file.

#[path = "../../torque-ts-vm/tests/sweep/mod.rs"]
mod sweep;

mod camera_modes;
mod dissolve_wire;
mod drawn_shapes;
mod gizmos_pictures;
mod gui_controls;
mod input_sources;
mod native_sweep;
mod natives;
mod quic_link;
mod render_ext;
mod render_passes;
mod server_commands;
mod shape_sounds;
mod view_rebuild;
