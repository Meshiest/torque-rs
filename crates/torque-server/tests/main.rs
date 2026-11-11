//! The integration tests, linked as one test program: one module per file.

mod figure;
mod net_client;

mod channels;
mod control_index;
mod datablock_live;
mod datablock_reload;
mod datablock_remove;
mod datablock_watch;
mod ghost_pass;
mod ghosting;
mod hitboxes;
mod interior;
mod lag_compensation;
mod lights;
mod plain;
mod plugin_classes;
mod projectile;
#[cfg(feature = "webrtc")]
mod rtc;
mod server;
mod set_datablock;
mod shape_reload;
mod tge_transform_masks;
#[cfg(feature = "web")]
mod web;
