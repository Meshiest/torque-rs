//! A determinism harness: scripted 600-tick runs of the predicted paths (the figure on foot, the buggy, the client's
//! prediction and its replay of the server's corrections) hashed into one value per scenario. The value must be the same on
//! every target: native, and wasm32 (`wasm32-wasip1`), which CI builds and compares.
//!
//! - [`hash::TraceHasher`]: the hash; floats go in by their bits.
//! - [`rig::Rig`] and [`rig::Mode`]: a server (`MinimalPlugins` and a plugin under test) and a client over the in-memory link, stepped by hand.
//! - [`rapier`]: the Rapier world's scenario (bodies dropped onto a terrain and an interior; no server or client), printed
//!   among [`scenarios::physics`] and folded into the `physics` line, not `all`.
//! - [`props`]: the props plugin's scenario (crates dropped and walked through, a server and a predicting client), printed among
//!   [`scenarios::plugins`] and folded into the `plugins` line; [`joints`], its joints' (a grappled figure swinging, with
//!   tethers, a winch, a weld and a hinge around it), printed after it; [`props_predict`], a client that predicts props (a
//!   held crate carried, and shots of the figure's own at it), printed after those.
//! - [`hierarchy`]: the scene hierarchy's scenario (a moving root, three levels of children and a Player child on it, and the
//!   client's figure riding a deck hung on a moving lift while the client predicts it, in engine mode, where the hierarchy
//!   is on), printed among [`scenarios::scene`] and folded into the `scene` line; [`hierarchy::ride`] runs the ride alone
//!   for torque-plugin-hierarchy's `tests/ride.rs`.
//! - [`hierarchy_props`]: props in the scene hierarchy (driven children on a moving lift, compound bodies loaded with crates, and
//!   the changes between the two), a server and a predicting client in engine mode, printed after [`hierarchy`]'s.
//! - [`turret`]: a second client controls a turret hung on the tank the first drives, predicting it in the tank's frame
//!   (engine mode), printed after [`hierarchy_props`]'s and folded into the `scene` line; [`turret::run`] runs it on the tank or a
//!   still base, with or without the gunner, for torque-plugin-hierarchy's `tests/turret.rs`. [`rig::Rig::add_client`]
//!   joins the second client.
//! - [`scenarios`]: the scenarios ([`scenarios::all`]) and [`scenarios::trace_hash`], the per-plugin entry point
//!   (`trace_hash::<MyPlugin>()`).
//!
//! The `trace-hash` binary prints `name mode hash` for every scenario (the mode is the network protocol, `tge`); `just trace-hash` runs it natively and as wasm32-wasip1
//! and fails when the two differ.
//!
//! What a predicted path must not do (the engine's rules for plugins that run on it): iterate a `HashMap` or `HashSet` in an
//! order that reaches the state, read a clock, or use a random number that is not seeded from the state. The harness cannot see
//! those directly; a plugin that does one shows as a hash that changes between runs or between targets.

pub mod hash;
pub mod hierarchy;
pub mod hierarchy_props;
pub mod joints;
pub mod props;
pub mod props_predict;
pub mod rapier;
pub mod rig;
pub mod scenarios;
pub mod turret;
