//! In a browser: a timer on the page's own thread that reads the pads between frames (a worker has no Gamepad API).

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::Closure;

use crate::sampler::Sampler;

/// How often the timer asks to run (milliseconds). Browsers run a timer no faster than about this, and slower on a hidden page.
pub const WEB_TIMER_MS: i32 = 4;

/// A running `setInterval` that samples the pads. Dropping it clears the interval.
pub struct WebTimer {
    id: i32,
    // Kept alive for as long as the interval runs.
    _tick: Closure<dyn FnMut()>,
}

impl WebTimer {
    /// Starts the timer over `sampler`; None when the page has no window to set it on.
    pub fn start(sampler: Rc<RefCell<Sampler>>) -> Option<WebTimer> {
        let window = web_sys::window()?;
        let tick = Closure::<dyn FnMut()>::new(move || {
            // A poll running at this moment holds the sampler: the timer skips a beat.
            if let Ok(mut sampler) = sampler.try_borrow_mut() {
                sampler.sample();
            }
        });
        let id = window
            .set_interval_with_callback_and_timeout_and_arguments_0(
                tick.as_ref().unchecked_ref(),
                WEB_TIMER_MS,
            )
            .ok()?;
        Some(WebTimer { id, _tick: tick })
    }
}

impl Drop for WebTimer {
    fn drop(&mut self) {
        if let Some(window) = web_sys::window() {
            window.clear_interval_with_handle(self.id);
        }
    }
}
