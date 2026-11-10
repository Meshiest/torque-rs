//! [`Gizmos`]: the shapes asked for and not yet gone, shared by the scripts' natives, a game's Rust code and the view that draws
//! them.

use std::cell::RefCell;
use std::rc::Rc;

use glam::{Quat, Vec3};
use torque_render_hd::gizmos::GizmoSegment;
use torque_ts_vm::vm::Vm;

use super::GizmoSettings;
use super::shapes::{self, Segment};

/// The longest a label's text is (bytes); a longer text is cut.
pub const MAX_LABEL_BYTES: usize = 256;
/// The longest a shape lasts (seconds); a longer time is cut.
pub const MAX_SECONDS: f32 = 86_400.0;

/// How a shape is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    /// Red, green, blue and alpha, each 0 to 1.
    pub color: [f32; 4],
    /// How long it stays, in seconds of drawn frames; 0 (or less) draws it for one frame.
    pub seconds: f32,
    /// Hidden where the scene is in front of it. Labels are not depth tested.
    pub depth_test: bool,
}

impl Style {
    /// `color` (red, green, blue, alpha), for one frame, depth tested.
    pub const fn new(color: [f32; 4]) -> Style {
        Style {
            color,
            seconds: 0.0,
            depth_test: true,
        }
    }

    /// The same, staying `seconds`.
    pub const fn seconds(self, seconds: f32) -> Style {
        Style { seconds, ..self }
    }

    /// The same, depth tested or not.
    pub const fn depth(self, depth_test: bool) -> Style {
        Style { depth_test, ..self }
    }
}

impl Default for Style {
    /// White, for one frame, depth tested.
    fn default() -> Style {
        Style::new([1.0; 4])
    }
}

/// A text at a point of the world.
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub position: [f32; 3],
    pub text: String,
    /// Red, green, blue and alpha.
    pub color: [u8; 4],
}

#[derive(Default)]
struct State {
    segments: Vec<GizmoSegment>,
    segment_ttl: Vec<f32>,
    labels: Vec<Label>,
    label_ttl: Vec<f32>,
    /// Shapes and labels refused for the caps since the last frame ended.
    dropped: u32,
    /// Whether the caps were hit the frame before: one warning per run of such frames.
    overflowing: bool,
    warnings: Vec<String>,
    scratch: Vec<Segment>,
}

/// The debug shapes of one client: lines, boxes, spheres, arrows, crosses and text labels in world space, each drawn for one
/// frame or for a number of seconds, in a color, depth tested or not. Immediate mode: call the drawing functions as often as the
/// shape should be drawn (every tick for a shape that follows something); the view that draws them
/// ([`WorldView`](crate::view::WorldView)) draws what is there each frame and lets go of what ran out.
///
/// A client made over [`GizmosPlugin`](super::GizmosPlugin) has one, found with [`Gizmos::of`]. Everything takes `&self`.
///
/// A shape that does not fit in the caps ([`GizmoSettings`]) is not kept; the first of a run of frames that dropped any is warned
/// on the client's console. A shape with a coordinate that is not a finite number is not kept either.
///
/// ```
/// use torque_app::gizmos::{GizmoSettings, Gizmos, Style};
///
/// let gizmos = Gizmos::new(GizmoSettings::default());
/// let red = Style::new([1.0, 0.0, 0.0, 1.0]);
/// gizmos.line([0.0, 0.0, 0.0], [0.0, 0.0, 5.0], red);
/// gizmos.sphere([0.0, 0.0, 5.0], 0.5, red.seconds(2.0).depth(false));
/// assert_eq!(gizmos.line_count(), 1 + 96);
/// // A drawn frame of a sixtieth of a second: the line goes, the sphere stays for two seconds.
/// gizmos.end_frame(1.0 / 60.0);
/// assert_eq!(gizmos.line_count(), 96);
/// ```
pub struct Gizmos {
    settings: GizmoSettings,
    state: RefCell<State>,
}

impl Gizmos {
    /// An empty set with `settings`.
    pub fn new(settings: GizmoSettings) -> Gizmos {
        Gizmos {
            settings,
            state: RefCell::new(State::default()),
        }
    }

    /// The set of the client whose script engine is `vm` (None: made without the plugin).
    pub fn of(vm: &Vm) -> Option<Rc<Gizmos>> {
        vm.extension::<Gizmos>()
    }

    /// The limits and the look it was made with.
    pub fn settings(&self) -> &GizmoSettings {
        &self.settings
    }

    /// A line from `a` to `b`.
    pub fn line(&self, a: impl Into<Vec3>, b: impl Into<Vec3>, style: Style) {
        self.add(style, |out| out.push((a.into(), b.into())));
    }

    /// The edges of the box with corners `min` and `max`, along the axes.
    pub fn aabb(&self, min: impl Into<Vec3>, max: impl Into<Vec3>, style: Style) {
        self.add(style, |out| shapes::aabb(min.into(), max.into(), out));
    }

    /// The edges of a box about `center`, `half` the extents of each side, turned by `rotation`.
    pub fn obb(
        &self,
        center: impl Into<Vec3>,
        half: impl Into<Vec3>,
        rotation: Quat,
        style: Style,
    ) {
        self.add(style, |out| {
            shapes::obb(center.into(), half.into().abs(), rotation, out);
        });
    }

    /// A sphere's outline: three circles.
    pub fn sphere(&self, center: impl Into<Vec3>, radius: f32, style: Style) {
        self.add(style, |out| {
            shapes::sphere(center.into(), radius.abs(), out);
        });
    }

    /// An arrow from `from`, its head at `to`.
    pub fn arrow(&self, from: impl Into<Vec3>, to: impl Into<Vec3>, style: Style) {
        self.add(style, |out| shapes::arrow(from.into(), to.into(), out));
    }

    /// A cross of three lines, `size` long on each side of `center`.
    pub fn cross(&self, center: impl Into<Vec3>, size: f32, style: Style) {
        self.add(style, |out| shapes::cross(center.into(), size.abs(), out));
    }

    /// A text above `position` (its color is the style's; it is not depth tested).
    pub fn label(&self, position: impl Into<Vec3>, text: &str, style: Style) {
        let position: Vec3 = position.into();
        if !position.is_finite() {
            return;
        }
        let mut s = self.state.borrow_mut();
        if s.labels.len() >= self.settings.max_labels {
            s.dropped += 1;
            s.warn_overflow(&format!(
                "more than {} labels at once: the rest are not drawn",
                self.settings.max_labels
            ));
            return;
        }
        let mut end = text.len().min(MAX_LABEL_BYTES);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        s.labels.push(Label {
            position: position.to_array(),
            text: text[..end].to_owned(),
            color: pack(style.color),
        });
        s.label_ttl.push(ttl(style.seconds));
    }

    /// Everything gone.
    pub fn clear(&self) {
        let mut s = self.state.borrow_mut();
        s.segments.clear();
        s.segment_ttl.clear();
        s.labels.clear();
        s.label_ttl.clear();
    }

    /// The segments held (a box is twelve, a sphere ninety-six).
    pub fn line_count(&self) -> usize {
        self.state.borrow().segments.len()
    }

    /// The labels held.
    pub fn label_count(&self) -> usize {
        self.state.borrow().labels.len()
    }

    /// Whether nothing is held: a view draws nothing, and does nothing on the GPU, for an empty set.
    pub fn is_empty(&self) -> bool {
        let s = self.state.borrow();
        s.segments.is_empty() && s.labels.is_empty()
    }

    /// Hands what is held to `draw` (the segments, then the labels), for a view to draw.
    pub fn with_drawn<R>(&self, draw: impl FnOnce(&[GizmoSegment], &[Label]) -> R) -> R {
        let s = self.state.borrow();
        draw(&s.segments, &s.labels)
    }

    /// A frame of `seconds` was drawn: what was asked for that long ago is gone (a shape asked for one frame goes after the
    /// first). The view calls it after each frame it draws; call it yourself where nothing draws.
    pub fn end_frame(&self, seconds: f32) {
        let seconds = if seconds.is_finite() {
            seconds.max(0.0)
        } else {
            0.0
        };
        let mut s = self.state.borrow_mut();
        let s = &mut *s;
        age(&mut s.segments, &mut s.segment_ttl, seconds);
        age(&mut s.labels, &mut s.label_ttl, seconds);
        if s.dropped == 0 {
            s.overflowing = false;
        }
        s.dropped = 0;
    }

    /// The warnings not yet told to the console, taken (the plugin's tick and the natives tell them).
    pub fn take_warnings(&self) -> Vec<String> {
        std::mem::take(&mut self.state.borrow_mut().warnings)
    }

    fn add(&self, style: Style, shape: impl FnOnce(&mut Vec<Segment>)) {
        let mut s = self.state.borrow_mut();
        let s = &mut *s;
        s.scratch.clear();
        shape(&mut s.scratch);
        if s.scratch
            .iter()
            .any(|(a, b)| !a.is_finite() || !b.is_finite())
        {
            return;
        }
        if s.segments.len() + s.scratch.len() > self.settings.max_lines {
            s.dropped += 1;
            s.warn_overflow(&format!(
                "more than {} lines at once: the rest are not drawn",
                self.settings.max_lines
            ));
            return;
        }
        let color = pack(style.color);
        let life = ttl(style.seconds);
        for &(a, b) in &s.scratch {
            s.segments.push(GizmoSegment {
                a: a.to_array(),
                b: b.to_array(),
                color,
                depth_test: style.depth_test,
            });
            s.segment_ttl.push(life);
        }
    }
}

impl State {
    /// Queues the warning `text` unless the frames before already overflowed.
    fn warn_overflow(&mut self, text: &str) {
        if !self.overflowing {
            self.overflowing = true;
            self.warnings.push(format!("gizmos: {text}"));
        }
    }
}

/// How long a shape asked for `seconds` lasts.
fn ttl(seconds: f32) -> f32 {
    if seconds.is_finite() {
        seconds.clamp(0.0, MAX_SECONDS)
    } else {
        0.0
    }
}

/// A color as bytes.
fn pack(color: [f32; 4]) -> [u8; 4] {
    color.map(|c| {
        if c.is_finite() {
            (c.clamp(0.0, 1.0) * 255.0).round() as u8
        } else {
            0
        }
    })
}

/// Keeps what is left after `seconds`: each entry whose time less `seconds` is more than 0.
fn age<T>(items: &mut Vec<T>, lives: &mut Vec<f32>, seconds: f32) {
    if lives.iter().all(|&l| l - seconds > 0.0) {
        for l in lives.iter_mut() {
            *l -= seconds;
        }
        return;
    }
    let mut keep = 0;
    for i in 0..items.len() {
        let left = lives[i] - seconds;
        if left > 0.0 {
            items.swap(keep, i);
            lives[keep] = left;
            keep += 1;
        }
    }
    items.truncate(keep);
    lives.truncate(keep);
}
