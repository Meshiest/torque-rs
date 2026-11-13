//! The integration tests, linked as one test program: one module per file.

#[path = "net/rig.rs"]
mod rig;

mod datablock_prediction;
mod datablock_refresh;
mod datablock_reload;
mod draw_scale;
mod frozen;
mod frozen_messages;
mod frozen_predict;
mod fuzz;
mod hierarchy;
mod hierarchy_net;
mod joints;
mod joints_net;
mod net;
mod predict;
mod scope;
mod shape_collision;
mod tge_client;
