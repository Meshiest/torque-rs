//! A game in a browser and its page: the renderer's quality the page starts at ([`web_quality`]), the game's loading stages
//! and its frame times written on the page's root element for the page's displays (`web/index.html`, `web/loading.js`).
//!
//! **The quality.** `?quality=low|medium|high|ultra` in the page's address picks it. Otherwise Firefox starts at
//! [`FIREFOX_QUALITY`]: its WebGPU spends far more on each pass and draw than Chromium's, so the same frame costs it
//! several times as much. Other browsers keep the game's own.
//!
//! **The stages** (`data-torque-stage`): `gpu` while the canvas's device is asked for, `map` while the game says it is loading
//! ([`crate::App::loading`]), then `ready` (the page's loading display goes).
//!
//! **The frame times** ([`FrameTimes`]): every [`FrameTimes::EVERY`] the frames a second (`data-torque-fps`) and the mean and
//! longest time between two frames in ms (`data-torque-frame-ms`, `mean max`).

use torque_render_scene::hd::Quality;
use web_time::{Duration, Instant};

/// The quality Firefox starts at when the address names none.
pub const FIREFOX_QUALITY: Quality = Quality::Low;

/// The quality a page starts at: the address's `quality` (`query`, the page's `location.search`), else
/// [`FIREFOX_QUALITY`] in Firefox (`user_agent`, `navigator.userAgent`); None: the game's own.
pub fn web_quality(user_agent: &str, query: &str) -> Option<Quality> {
    let asked = query
        .trim_start_matches('?')
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == "quality")
        .and_then(|(_, v)| Quality::named(v));
    asked.or_else(|| is_firefox(user_agent).then_some(FIREFOX_QUALITY))
}

/// Whether `user_agent` is Firefox's (Gecko's `Firefox/<version>`; Seamonkey and the like count, Chromium's never say it).
pub fn is_firefox(user_agent: &str) -> bool {
    user_agent.contains("Firefox/")
}

/// The time between frames, summed up every [`FrameTimes::EVERY`].
#[derive(Debug, Clone)]
pub struct FrameTimes {
    last: Option<Instant>,
    since: Option<Instant>,
    frames: u32,
    sum: Duration,
    max: Duration,
}

/// A summary of the frames since the last one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameSummary {
    pub fps: f64,
    /// The mean and the longest time between two frames (ms).
    pub mean_ms: f64,
    pub max_ms: f64,
}

impl Default for FrameTimes {
    fn default() -> FrameTimes {
        FrameTimes::new()
    }
}

impl FrameTimes {
    /// How often a summary is made.
    pub const EVERY: Duration = Duration::from_millis(500);

    pub fn new() -> FrameTimes {
        FrameTimes {
            last: None,
            since: None,
            frames: 0,
            sum: Duration::ZERO,
            max: Duration::ZERO,
        }
    }

    /// A frame was presented at `now`: the summary when [`FrameTimes::EVERY`] has passed since the last one.
    pub fn frame(&mut self, now: Instant) -> Option<FrameSummary> {
        let Some(last) = self.last.replace(now) else {
            self.since = Some(now);
            return None;
        };
        let gap = now.saturating_duration_since(last);
        self.frames += 1;
        self.sum += gap;
        self.max = self.max.max(gap);
        let since = *self.since.get_or_insert(last);
        let span = now.saturating_duration_since(since);
        if span < Self::EVERY {
            return None;
        }
        let summary = FrameSummary {
            fps: f64::from(self.frames) / span.as_secs_f64(),
            mean_ms: self.sum.as_secs_f64() * 1000.0 / f64::from(self.frames),
            max_ms: self.max.as_secs_f64() * 1000.0,
        };
        self.since = Some(now);
        self.frames = 0;
        self.sum = Duration::ZERO;
        self.max = Duration::ZERO;
        Some(summary)
    }
}

/// In a browser: the page's address (`location.search`) and the browser's user agent (empty where the page has none).
#[cfg(target_arch = "wasm32")]
pub fn page_query_and_agent() -> (String, String) {
    let Some(window) = web_sys::window() else {
        return (String::new(), String::new());
    };
    let query = window.location().search().unwrap_or_default();
    let agent = window.navigator().user_agent().unwrap_or_default();
    (query, agent)
}

/// In a browser: `value` on the page's root element as `data-torque-<name>`.
#[cfg(target_arch = "wasm32")]
pub fn mark(name: &str, value: &str) {
    if let Some(root) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    {
        let _ = root.set_attribute(&format!("data-torque-{name}"), value);
    }
}

/// In a browser: the frames' summary on the page.
#[cfg(target_arch = "wasm32")]
pub fn mark_frames(s: &FrameSummary) {
    mark("fps", &format!("{:.1}", s.fps));
    mark("frame-ms", &format!("{:.2} {:.1}", s.mean_ms, s.max_ms));
}

#[cfg(test)]
#[path = "web_page_tests.rs"]
mod tests;
