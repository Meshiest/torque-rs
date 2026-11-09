//! Which moving shapes a frame of the enhanced renderer can show ([`ViewCull`], a [`crate::moving::Cull`]): a shape is left out only
//! when nothing the frame draws can show it or anything it casts. It is kept when its sphere meets an eye's view, when the shadow it
//! casts from the sun (its sphere swept away from the sun) meets the view within the shadows' range, when it or its shadow is seen in
//! the water's planar reflection, when it is within the reach of a point or spot light that the view sees, and, while the light
//! capture is drawn (a cube of every direction from the eye), whenever it is within the visible distance.
//!
//! The eyes are the frame's guesses at the eye it is drawn from (the eye the host is about to draw from, the world's camera, the
//! last frame's drawn eye), each view widened by [`MARGIN`]: a host whose eye turns between the build and the draw keeps what is
//! just past the edge. A frame whose eye jumped (a cut) is not culled at all (the app's `WorldView::ghost_cull`).

use glam::Vec3;

use crate::hd::HdScene;
use crate::math::Mat;
use crate::moving::Cull;
use crate::source::Eye;

/// The views' widening: each side of the view turned out by this share of its half width, and the spheres grown by
/// [`MARGIN_UNITS`].
pub const MARGIN: f32 = 0.25;
pub const MARGIN_UNITS: f32 = 1.0;

/// The least move of the eye from one frame to the next that is a cut (units), and the least turn (radians): a teleport, a
/// respawn, a camera mode taken with no ease or a large jump of an orbit. A frame after a cut is not culled.
pub const CUT_DISTANCE: f32 = 4.0;
pub const CUT_TURN: f32 = 0.5;

/// The way `eye` looks (unit).
fn forward(eye: &Eye) -> Vec3 {
    Vec3::from(Mat::mul(&Mat::euler_z(eye.yaw), &Mat::euler_x(eye.pitch)).column(1))
}

/// Whether the eye going from `last` to `now` in a frame is a cut ([`CUT_DISTANCE`], [`CUT_TURN`]); a jump that is not a number
/// is one.
pub fn is_cut(last: &Eye, now: &Eye) -> bool {
    let moved = Vec3::from(now.position).distance(Vec3::from(last.position));
    let turned = forward(last).dot(forward(now)).clamp(-1.0, 1.0).acos();
    !(moved < CUT_DISTANCE && turned < CUT_TURN)
}

/// One eye's view: four side planes through the eye (unit normals pointing in) and the eye's place and forward direction.
#[derive(Debug, Clone, Copy, PartialEq)]
struct View {
    eye: Vec3,
    forward: Vec3,
    sides: [Vec3; 4],
}

impl View {
    /// The view of `eye` with the horizontal field of view `fov_deg` on a `width` x `height` frame, widened by [`MARGIN`].
    fn of(eye: &Eye, fov_deg: f32, (width, height): (u32, u32)) -> View {
        let m = Mat::mul(&Mat::euler_z(eye.yaw), &Mat::euler_x(eye.pitch));
        let (right, forward, up) = (
            Vec3::from(m.column(0)),
            Vec3::from(m.column(1)),
            Vec3::from(m.column(2)),
        );
        let half = (fov_deg.clamp(1.0, 179.0).to_radians() / 2.0).tan();
        let aspect = height.max(1) as f32 / width.max(1) as f32;
        let (h, v) = (half * (1.0 + MARGIN), half * aspect * (1.0 + MARGIN));
        // A side plane's inward normal: the forward direction tilted against the side it bounds.
        let side = |across: Vec3, t: f32| (forward * t - across).normalize_or(forward);
        View {
            eye: Vec3::from(eye.position),
            forward,
            sides: [side(right, h), side(-right, h), side(up, v), side(-up, v)],
        }
    }

    /// Whether the capsule from `a` to `b` of `radius` may meet the view up to `far` from the eye (no near plane: what is about
    /// the eye can hide a corona).
    fn meets(&self, a: Vec3, b: Vec3, radius: f32, far: f32) -> bool {
        let r = radius + MARGIN_UNITS;
        let (a, b) = (a - self.eye, b - self.eye);
        let outside = |n: Vec3, d: f32| n.dot(a) - d < -r && n.dot(b) - d < -r;
        if self.sides.iter().any(|&n| outside(n, 0.0)) {
            return false;
        }
        !outside(-self.forward, -far)
    }
}

/// What a frame of the enhanced renderer can show (module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct ViewCull {
    views: Vec<View>,
    far: f32,
    /// The way the sun's light goes (unit) and the farthest the shadows reach from the eye; none: no sun shadows.
    sun: Option<(Vec3, f32)>,
    /// The water's level, when its planar reflection may be drawn.
    water: Option<f32>,
    /// The light capture is drawn: every shape within the visible distance is kept.
    capture: bool,
    /// The point and spot lights the views see: where each is and its reach.
    lights: Vec<(Vec3, f32)>,
}

impl ViewCull {
    /// The cull of a frame of `scene` drawn from any of `eyes` (each with its horizontal field of view, degrees), with the light
    /// capture drawn or not. `shadow_end` is where the last frame's sun shadows ended (its shadow range's end, which a range
    /// fitted to far geometry pushes past the scene's `shadow_distance`); none: the shadows may reach the visible distance. With
    /// no eye nothing is left out.
    pub fn new<G>(
        scene: &HdScene<G>,
        eyes: &[(Eye, f32)],
        capture: bool,
        shadow_end: Option<f32>,
    ) -> Option<ViewCull> {
        if eyes.is_empty() {
            return None;
        }
        let views: Vec<View> = eyes
            .iter()
            .map(|(e, fov)| View::of(e, *fov, scene.size))
            .collect();
        let far = scene.visible_distance.max(1.0);
        let towards = Vec3::from(scene.sun.towards);
        let reach = shadow_end
            .map_or(far, |end| end.max(scene.shadow_distance))
            .min(far);
        let sun = (scene.shadow_distance > 0.0 && towards.length_squared() > 1e-12)
            .then(|| (-towards.normalize(), reach));
        let water = crate::hd::reflect::water_level(scene);
        let mut out = ViewCull {
            views,
            far,
            sun,
            water,
            capture,
            lights: Vec::new(),
        };
        let lights = scene
            .lights
            .iter()
            .chain(scene.spots.iter().map(|s| &s.light))
            .map(|l| (Vec3::from(l.position), l.radius.max(0.0)))
            .filter(|&(p, r)| out.seen(p, r))
            .collect();
        out.lights = lights;
        Some(out)
    }

    /// Whether a sphere at `centre` of `radius` meets a view, straight or in the water.
    fn seen(&self, centre: Vec3, radius: f32) -> bool {
        self.capsule_seen(centre, centre, radius, self.far)
    }

    fn capsule_seen(&self, a: Vec3, b: Vec3, radius: f32, far: f32) -> bool {
        let mirror = |p: Vec3, l: f32| Vec3::new(p.x, p.y, 2.0 * l - p.z);
        self.views.iter().any(|v| {
            v.meets(a, b, radius, far)
                || self
                    .water
                    .is_some_and(|l| v.meets(mirror(a, l), mirror(b, l), radius, far))
        })
    }
}

impl Cull for ViewCull {
    fn keeps(&self, centre: [f32; 3], radius: f32) -> bool {
        let centre = Vec3::from(centre);
        if self.seen(centre, radius) {
            return true;
        }
        let near = |p: Vec3, reach: f32| centre.distance(p) <= reach + radius + MARGIN_UNITS;
        if self.capture && self.views.iter().any(|v| near(v.eye, self.far)) {
            return true;
        }
        if let Some((way, reach)) = self.sun {
            // The shadow falls along the light's way; past the eye's distance plus the shadows' reach it is out of every cascade.
            let length = self
                .views
                .iter()
                .map(|v| centre.distance(v.eye))
                .fold(0.0f32, f32::max)
                + reach
                + radius;
            if self.capsule_seen(centre, centre + way * length, radius, reach) {
                return true;
            }
        }
        self.lights.iter().any(|&(p, reach)| near(p, reach))
    }
}

#[cfg(test)]
#[path = "view_cull_tests.rs"]
mod tests;
