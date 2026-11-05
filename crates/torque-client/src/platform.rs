//! The engine's window glue (feature `window`): what a window client hands the platform layer.
//!
//! | module | part |
//! |---|---|
//! | [`keymap`] | the window's keys, buttons and wheel as the platform messages the input layer reads ([`crate::input`]) |
//! | [`cursor_lock`] | the window's hold on the system cursor while the game has it |
//! | [`frame_cap`] | the frame cap while no world is drawn |
//! | [`weblink`] | the URL opener `gotoWebPage` hands an address to |
//! | [`web_input`] | in a browser: the keys held let go when the page loses the focus or the pointer lock, and a lost lock as Esc |
//! | [`window`] | the window's event loop over a client host ([`window::run`]): input, cursor hold, screen modes, clock, frame cap |
//!
//! What the window draws, and with which renderers, is the game's ([`window::WindowGame`]).

pub mod cursor_lock;
pub mod frame_cap;
pub mod keymap;
pub mod web_input;
pub mod weblink;
pub mod window;
