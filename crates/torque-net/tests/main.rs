//! The integration tests, linked as one test program: one module per file.

mod engine_common;

mod channels_accept;
mod chunk_link;
mod connect_gate;
mod engine_fuzz;
mod engine_golden_v5;
mod engine_link;
mod events_link;
mod golden_request;
mod proto_fuzz;
mod roundtrip;
mod tge_golden;
