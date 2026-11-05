//! The window's frame cap while no game world is drawn. The main menu, the menus over its background and the loading screens
//! before the game view follow the display: with vertical sync the presents pace them at its refresh; without it they draw at most
//! the display's refresh rate a second (240 when the monitor does not say). A preference of the host's set to a number caps them
//! there instead (0: no cap). A frame with a world is never held back (in
//! game the vsync pref alone paces it). The window waits for the
//! next frame's time (`ControlFlow::WaitUntil`: the thread sleeps on a high-resolution timer), it never spins.
//!
//! The frames are due on a fixed beat from the first one (each a period after the last one's due time), not a period after the last
//! one ended: a frame's own work and the timer's wake-up delay are inside the period, so the menu holds the cap.

use web_time::{Duration, Instant};

/// The cap when the preference is unset and the display's refresh rate is not known.
pub const UNKNOWN_DISPLAY_FPS: u32 = 240;

/// What paces the menu's frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cap {
    /// No cap.
    None,
    /// The display's refresh: the presents wait for it (vertical sync), or the cap is its rate.
    Display,
    /// At most this many frames a second.
    Fps(u32),
}

/// The cap a preference's text asks for: unset or not a number follows the display, 0 is none, anything else at least 10.
pub fn cap_of(pref: &str) -> Cap {
    let t = pref.trim();
    if t.is_empty() {
        return Cap::Display;
    }
    match t.parse::<f64>() {
        Ok(v) if v.is_finite() && v <= 0.0 => Cap::None,
        Ok(v) if v.is_finite() => Cap::Fps((v.round() as u32).clamp(10, 10_000)),
        _ => Cap::Display,
    }
}

/// The frames a second the window holds the menu to (0: none): the display's cap is no timer when the presents wait for the
/// refresh (vsync), else the refresh rate (`display_mhz`, the monitor's in millihertz), else [`UNKNOWN_DISPLAY_FPS`].
pub fn fps_for(cap: Cap, vsync: bool, display_mhz: Option<u32>) -> u32 {
    match cap {
        Cap::None => 0,
        Cap::Fps(n) => n,
        Cap::Display if vsync => 0,
        Cap::Display => display_mhz
            .filter(|m| *m >= 1_000)
            .map_or(UNKNOWN_DISPLAY_FPS, |m| m.div_ceil(1000).max(30)),
    }
}

/// When frames may be drawn.
#[derive(Debug, Default)]
pub struct FrameCap {
    /// When the next frame is due (None: at once).
    next: Option<Instant>,
}

impl FrameCap {
    pub fn new() -> FrameCap {
        FrameCap::default()
    }

    /// The time the next frame may be drawn at: None when it may be drawn now (no cap, a world drawn, or its time has come).
    pub fn wait_until(&self, now: Instant, fps: u32, world: bool) -> Option<Instant> {
        if fps == 0 || world {
            return None;
        }
        let next = self.next?;
        (now < next).then_some(next)
    }

    /// The window's call after each presented frame, begun at `began` under a cap of `fps`, `world` when it drew a game world: the
    /// cap holds the menus only, so a frame with a world leaves no beat behind (the first menu frame after it is drawn at once).
    pub fn frame_drawn(&mut self, began: Instant, fps: u32, world: bool) {
        self.drawn(began, if world { 0 } else { fps });
    }

    /// A frame was begun at `began` under a cap of `fps`: the next is due a period after this one's due time, or a period after
    /// `began` when this one came more than a period late (a stall is not caught up with a burst) or before its due time (a frame
    /// the cap did not hold: a redraw the system asked for). The next frame is never due more than a period after `began`: else
    /// every frame of a game (drawn as fast as it goes, the cap not holding a world) would push the beat a period on, and after
    /// minutes at a few hundred frames a second the main menu after leaving the game would not be drawn again until the beat came
    /// (the window would look frozen).
    pub fn drawn(&mut self, began: Instant, fps: u32) {
        if fps == 0 {
            self.next = None;
            return;
        }
        let period = Duration::from_secs_f64(1.0 / f64::from(fps));
        self.next = Some(match self.next {
            Some(due) if began >= due && began - due < period => due + period,
            _ => began + period,
        });
    }
}

#[cfg(test)]
#[path = "frame_cap_tests.rs"]
mod tests;
