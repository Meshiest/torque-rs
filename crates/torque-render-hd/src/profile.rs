//! A frame's cost over many frames, taken offscreen: the GPU time between the frame's pass marks ([`HdWorld::gpu_passes`]) as a
//! median over the frames (a loaded machine moves the mean, seldom the median), the CPU time of the encoding's steps as a mean,
//! and what the last frame drew (its passes and draw calls by kind, its clusters and cells). The examples' `*-profile` runs
//! print one [`Profile::report`] per scene and quality.

use std::collections::BTreeMap;

use crate::ext::HdExtension;
use crate::{DRAW_KIND_MARKS, DrawStats, HdWorld, PASS_KIND_MARKS, PASS_KIND_NAMES};

/// The frames' pass times gathered so far ([`Profile::frame`]).
#[derive(Debug, Default, Clone)]
pub struct Profile {
    /// Each GPU mark's milliseconds, a value per frame, in the order the marks were first seen.
    gpu: Vec<(&'static str, Vec<f64>)>,
    /// Each CPU mark's milliseconds summed over the frames, and each upload step's.
    cpu: Vec<(&'static str, f64)>,
    upload: Vec<(&'static str, f64)>,
    frames: usize,
    stats: DrawStats,
}

/// The value under `name` in `rows` (pushed as `empty()` when there is none; the rows keep the order the names were first seen).
fn entry<'a, T>(
    rows: &'a mut Vec<(&'static str, T)>,
    name: &'static str,
    empty: impl FnOnce() -> T,
) -> &'a mut T {
    let at = match rows.iter().position(|(n, _)| *n == name) {
        Some(at) => at,
        None => {
            rows.push((name, empty()));
            rows.len() - 1
        }
    };
    &mut rows[at].1
}

/// `values`' median (the upper one of an even count); 0 of none.
pub fn median(values: &[f64]) -> f64 {
    let mut v: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

impl Profile {
    /// Turns the world's pass marks on; a frame drawn before this has no marks to read.
    pub fn begin<E: HdExtension>(world: &mut HdWorld<E>) -> Profile {
        world.profile_passes = true;
        world.take_cpu_passes();
        world.take_upload_steps();
        Profile::default()
    }

    /// Takes the frame just drawn: its GPU pass times (waits for the GPU), its CPU marks and its draw stats.
    pub fn frame<E: HdExtension>(&mut self, world: &mut HdWorld<E>) {
        if let Some(passes) = world.gpu_passes() {
            for (name, ms) in passes {
                entry(&mut self.gpu, name, Vec::new).push(ms);
            }
        }
        for (name, ms) in world.take_cpu_passes() {
            *entry(&mut self.cpu, name, || 0.0) += ms;
        }
        for (name, ms) in world.take_upload_steps() {
            *entry(&mut self.upload, name, || 0.0) += ms;
        }
        self.stats = world.stats;
        self.frames += 1;
    }

    pub fn frames(&self) -> usize {
        self.frames
    }

    /// Each GPU mark's median milliseconds, and their sum (the frame's GPU time between its first and last mark).
    pub fn gpu_medians(&self) -> (Vec<(&'static str, f64)>, f64) {
        let rows: Vec<(&'static str, f64)> =
            self.gpu.iter().map(|(n, v)| (*n, median(v))).collect();
        let total = rows.iter().map(|r| r.1).sum();
        (rows, total)
    }

    /// Each CPU mark's mean milliseconds (the counts of passes and draws among them, as means too).
    pub fn cpu_means(&self) -> Vec<(&'static str, f64)> {
        let n = self.frames.max(1) as f64;
        self.cpu
            .iter()
            .map(|(name, sum)| (*name, sum / n))
            .collect()
    }

    /// Each upload step's mean milliseconds a frame (the uploads between the frames, summed by step).
    pub fn upload_means(&self) -> Vec<(&'static str, f64)> {
        let n = self.frames.max(1) as f64;
        self.upload
            .iter()
            .map(|(name, sum)| (*name, sum / n))
            .collect()
    }

    /// The report: the GPU passes by median with their share of the frame, the CPU steps (the draw's, then the upload's) that
    /// cost at least `cpu_floor_ms`, the passes and draw calls by kind, and the last frame's stats.
    pub fn report(&self, cpu_floor_ms: f64) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let (gpu, total) = self.gpu_medians();
        let _ = writeln!(
            out,
            "  gpu (median of {} frames): {total:.3} ms",
            self.frames
        );
        for (name, ms) in &gpu {
            if *ms >= 0.005 {
                let share = if total > 0.0 { 100.0 * ms / total } else { 0.0 };
                let _ = writeln!(out, "    {name:<28} {ms:>8.3} ms {share:>5.1} %");
            }
        }
        let cpu = self.cpu_means();
        let counts: BTreeMap<&str, f64> = cpu
            .iter()
            .filter(|(n, _)| {
                *n == "passes" || PASS_KIND_MARKS.contains(n) || DRAW_KIND_MARKS.contains(n)
            })
            .map(|(n, v)| (*n, *v))
            .collect();
        let cpu_total: f64 = cpu
            .iter()
            .filter(|(n, _)| !counts.contains_key(n))
            .map(|(_, ms)| ms)
            .sum();
        let _ = writeln!(out, "  cpu (mean): {cpu_total:.3} ms");
        for (name, ms) in &cpu {
            if !counts.contains_key(name) && *ms >= cpu_floor_ms {
                let _ = writeln!(out, "    {name:<28} {ms:>8.3} ms");
            }
        }
        // (A window's control syncs upload without steps: nothing to print then.)
        let upload = self.upload_means();
        if !upload.is_empty() {
            let upload_total: f64 = upload.iter().map(|(_, ms)| ms).sum();
            let _ = writeln!(out, "  upload (mean): {upload_total:.3} ms");
            for (name, ms) in &upload {
                if *ms >= cpu_floor_ms {
                    let _ = writeln!(out, "    {name:<28} {ms:>8.3} ms");
                }
            }
        }
        let _ = write!(
            out,
            "  passes {:.0}:",
            counts.get("passes").copied().unwrap_or(0.0)
        );
        for (k, name) in PASS_KIND_NAMES.iter().enumerate() {
            let p = counts.get(PASS_KIND_MARKS[k]).copied().unwrap_or(0.0);
            let d = counts.get(DRAW_KIND_MARKS[k]).copied().unwrap_or(0.0);
            if p > 0.0 || d > 0.0 {
                let _ = write!(out, " {name} {p:.0}/{d:.0}");
            }
        }
        let _ = writeln!(out, " (passes/draws)");
        let s = &self.stats;
        let _ = writeln!(
            out,
            "  last frame: {} draw calls, cells {}/{}, clusters {}/{}, lights {}, extras {}/{} (ground {})",
            s.draw_calls,
            s.cells_drawn,
            s.cells,
            s.clusters_drawn,
            s.clusters,
            s.lights,
            s.extras_drawn,
            s.extras,
            s.ground_extras_drawn
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::median;

    #[test]
    fn the_median_is_the_middle_value_and_ignores_what_is_not_finite() {
        assert_eq!(median(&[]), 0.0);
        assert_eq!(median(&[3.0]), 3.0);
        assert_eq!(median(&[5.0, 1.0, 3.0]), 3.0);
        assert_eq!(median(&[4.0, 1.0, 3.0, 2.0]), 3.0);
        assert_eq!(median(&[f64::NAN, 2.0, 1.0, f64::INFINITY]), 2.0);
    }
}
