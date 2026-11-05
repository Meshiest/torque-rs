//! What a window's frame breakdown is made of, whatever it counts: the frames' rolling mean interval ([`Rolling`]), the
//! percentile of a sorted list, names cleaned for the console ([`clean`]), lists folded on two lines ([`push_list`]) and the
//! process's memory read by a query on a thread of its own ([`MemoryQuery`]: PowerShell with no window on Windows).

use std::collections::VecDeque;
use std::sync::mpsc;
use web_time::{Duration, Instant};

/// The frames' intervals over the last `window` (when each frame was presented, its interval in ms) and their sum.
#[derive(Debug, Clone)]
pub struct Rolling {
    window: Duration,
    frames: VecDeque<(Instant, f64)>,
    sum: f64,
    last_present: Option<Instant>,
}

impl Rolling {
    /// No frames yet, a mean over `window`.
    pub fn new(window: Duration) -> Rolling {
        Rolling {
            window,
            frames: VecDeque::new(),
            sum: 0.0,
            last_present: None,
        }
    }

    /// A frame was presented at `now`: its interval since the last (ms; None for the first).
    pub fn present(&mut self, now: Instant) -> Option<f64> {
        let last = self.last_present.replace(now)?;
        let ms = now.duration_since(last).as_secs_f64() * 1000.0;
        self.frames.push_back((now, ms));
        self.sum += ms;
        while self
            .frames
            .front()
            .is_some_and(|(t, _)| now.duration_since(*t) > self.window)
        {
            if let Some((_, old)) = self.frames.pop_front() {
                self.sum -= old;
            }
        }
        Some(ms)
    }

    /// The mean of the frames' intervals (ms; 0 before half a window of frames).
    pub fn mean(&self) -> f64 {
        let span = match (self.frames.front(), self.frames.back()) {
            (Some(a), Some(b)) => b.0.duration_since(a.0),
            _ => Duration::ZERO,
        };
        if span < self.window / 2 || self.frames.is_empty() {
            return 0.0;
        }
        self.sum / self.frames.len() as f64
    }
}

/// The process's memory, as the query read it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Memory {
    pub working_set: u64,
    pub private: u64,
}

/// The memory query running on a thread of its own ([`query_memory`]).
#[derive(Debug)]
pub struct MemoryQuery {
    rx: mpsc::Receiver<Option<Memory>>,
}

impl MemoryQuery {
    /// Starts a query (None: no thread).
    pub fn start() -> Option<MemoryQuery> {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("perf-memory".into())
            .spawn(move || {
                let _ = tx.send(query_memory());
            })
            .ok()
            .map(|_| MemoryQuery { rx })
    }

    /// The answer once it has one.
    pub fn poll(&self) -> Option<Option<Memory>> {
        self.rx.try_recv().ok()
    }
}

/// `items` after `head`, on at most two lines.
pub fn push_list(out: &mut Vec<String>, head: &str, items: Vec<String>) {
    if items.is_empty() {
        out.push(format!("{head}: none"));
        return;
    }
    let half = items.len().div_ceil(2).max(5).min(items.len());
    out.push(format!("{head}: {}", items[..half].join(", ")));
    if half < items.len() {
        out.push(format!("{head} (more): {}", items[half..].join(", ")));
    }
}

/// Bytes as GiB.
pub fn gib(b: u64) -> f64 {
    b as f64 / (1u64 << 30) as f64
}

/// The `p` percentile of the sorted `sorted` (0 when empty).
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// A name for the console: control characters as `?`, at most `max` characters (`...` after a cut).
pub fn clean(bytes: &[u8], max: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out: String = text
        .chars()
        .take(max)
        .map(|c| if c.is_control() { '?' } else { c })
        .collect();
    if text.chars().nth(max).is_some() {
        out.push_str("...");
    }
    out
}

/// The process's working set and private bytes, read by PowerShell with no window (None without it).
pub fn query_memory() -> Option<Memory> {
    let me = std::process::id();
    let mut cmd = std::process::Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        &format!("$p = Get-Process -Id {me}; \"$($p.WorkingSet64) $($p.PrivateMemorySize64)\""),
    ])
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Win32's CREATE_NO_WINDOW.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd.output().ok()?;
    parse_memory(&String::from_utf8_lossy(&out.stdout))
}

/// The query's answer: the working set and the private bytes.
pub fn parse_memory(text: &str) -> Option<Memory> {
    let mut words = text.split_whitespace();
    let working_set = words.next()?.parse().ok()?;
    let private = words.next()?.parse().ok()?;
    Some(Memory {
        working_set,
        private,
    })
}

#[cfg(test)]
#[path = "perf_tests.rs"]
mod tests;
