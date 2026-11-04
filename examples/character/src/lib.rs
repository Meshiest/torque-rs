//! An animated character: the asset kit's box figure on a tiled floor, walking, running and jumping under the keys, seen by a
//! camera that circles it, with a help line drawn by the GUI over the world. Client only: no server and no connection.
//!
//! - [`motion`]: the figure's motion and its animation state machine (idle, walk, run, jump) over one sequence thread, its
//!   changes blended as transitions and its gaits played at the speed their ground frames carry the figure.
//! - [`game`]: the client host (the canvas, the action map, the clock), the figure and the camera, and a frame for the world view.
//! - [`window`]: the example in a window.
//! - [`shots`]: the example without a window, drawn offscreen: pictures of its states for checking by eye.

pub mod game;
pub mod motion;
pub mod shots;
pub mod window;
