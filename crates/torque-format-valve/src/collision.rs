//! The collision of a model: its physics file's convex pieces in the model's space, or, when it has none, a fallback.
//!
//! In order: (1) the pieces of the model's `.phy` ([`crate::phy`]); (2) the box of the model's hull ([`crate::mdl::Mdl::hull`]),
//! when the header gives one with a volume; (3) the convex hull of the render mesh's vertices ([`crate::vvd::Vvd`]). The
//! fallbacks serve a model whose `.phy` is missing, refused (over a cap, hostile) or empty of convex pieces (a ball, a tree of
//! triangles, a virtual mesh). [`ModelCollision::source`] says which was used and [`ModelCollision::notes`] why the earlier ones
//! were not.
//!
//! **Space.** The model's: the inches and axes of its vertices and bounds. A solid that is named for a bone (a ragdoll's: the
//! solids of such a model hold their points in the bone's own space) is taken to the model's space by the bone's rest pose
//! (the inverse of its pose-to-bone matrix); any other solid's points are in the model's space already.

use std::rc::Rc;

use torque_engine::assets::Assets;
use torque_ts_vm::host::FileHost;

use crate::hull::{Hull, bounds, convex_hull};
use crate::mdl::{Bone, Mdl};
use crate::phy::{MAX_HULL_WORK, Phy};
use crate::read::{Error, Result};
use crate::vvd::Vvd;

/// Where a model's hulls came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HullSource {
    /// The physics file's convex pieces.
    Phy,
    /// The box of the model's hull.
    MdlBox,
    /// The convex hull of the render mesh.
    RenderHull,
}

/// A model's collision hulls.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelCollision {
    pub source: HullSource,
    /// The convex pieces, in the model's space.
    pub hulls: Vec<Hull>,
    /// The model's bounding box as a hull (what a prop solid by its bounds collides as): the header's box, else the box of the
    /// render mesh.
    pub bounds: Option<Hull>,
    /// Why the physics file was not used, and anything else worth knowing.
    pub notes: Vec<String>,
}

/// The smallest extent (inches) a model's hull box may have on each axis to be used.
const MIN_BOX_EXTENT: f32 = 0.01;

/// The header's box as a hull, when it has a volume.
fn mdl_box(mdl: &Mdl) -> Option<Hull> {
    let [lo, hi] = mdl.hull;
    let ok =
        (0..3).all(|k| lo[k].is_finite() && hi[k].is_finite() && hi[k] - lo[k] > MIN_BOX_EXTENT);
    ok.then(|| Hull::of_box(lo, hi))
}

/// The matrix taking a bone's space to the model's: the inverse of its pose-to-bone (3 x 4, row-major), None when it has none.
fn bone_to_model(bone: &Bone) -> Option<[f32; 12]> {
    let m = &bone.pose_to_bone;
    let r = [[m[0], m[1], m[2]], [m[4], m[5], m[6]], [m[8], m[9], m[10]]];
    let det = r[0][0] * (r[1][1] * r[2][2] - r[1][2] * r[2][1])
        - r[0][1] * (r[1][0] * r[2][2] - r[1][2] * r[2][0])
        + r[0][2] * (r[1][0] * r[2][1] - r[1][1] * r[2][0]);
    if det.abs() <= 1e-9 || !det.is_finite() {
        return None;
    }
    let inv = [
        [
            (r[1][1] * r[2][2] - r[1][2] * r[2][1]) / det,
            (r[0][2] * r[2][1] - r[0][1] * r[2][2]) / det,
            (r[0][1] * r[1][2] - r[0][2] * r[1][1]) / det,
        ],
        [
            (r[1][2] * r[2][0] - r[1][0] * r[2][2]) / det,
            (r[0][0] * r[2][2] - r[0][2] * r[2][0]) / det,
            (r[0][2] * r[1][0] - r[0][0] * r[1][2]) / det,
        ],
        [
            (r[1][0] * r[2][1] - r[1][1] * r[2][0]) / det,
            (r[0][1] * r[2][0] - r[0][0] * r[2][1]) / det,
            (r[0][0] * r[1][1] - r[0][1] * r[1][0]) / det,
        ],
    ];
    let t = [m[3], m[7], m[11]];
    let mut out = [0f32; 12];
    for i in 0..3 {
        for j in 0..3 {
            out[i * 4 + j] = inv[i][j];
        }
        out[i * 4 + 3] = -(inv[i][0] * t[0] + inv[i][1] * t[1] + inv[i][2] * t[2]);
    }
    out.iter().all(|v| v.is_finite()).then_some(out)
}

fn apply(m: &[f32; 12], p: [f32; 3]) -> [f32; 3] {
    [
        m[0] * p[0] + m[1] * p[1] + m[2] * p[2] + m[3],
        m[4] * p[0] + m[5] * p[1] + m[6] * p[2] + m[7],
        m[8] * p[0] + m[9] * p[1] + m[10] * p[2] + m[11],
    ]
}

/// The convex pieces of every solid of `phy` in `mdl`'s space (module docs).
pub fn model_space_hulls(phy: &Phy, mdl: &Mdl) -> Vec<Hull> {
    let mut out = Vec::new();
    for (i, solid) in phy.solids.iter().enumerate() {
        let bone = phy
            .props(i)
            .filter(|p| !p.name.is_empty())
            .and_then(|p| {
                mdl.bones
                    .iter()
                    .find(|b| b.name.eq_ignore_ascii_case(&p.name))
            })
            .and_then(bone_to_model);
        for hull in solid.hulls() {
            out.push(match &bone {
                Some(m) => hull.mapped(&|p| apply(m, p)),
                None => hull.clone(),
            });
        }
    }
    out
}

impl ModelCollision {
    /// The hulls of a model from its parts: `phy` (None when there is none, with `why` saying so), its header, and a way to get
    /// the render mesh's vertices (asked for only when the fallbacks need it).
    pub fn from_parts(
        phy: Option<&Phy>,
        why: Option<String>,
        mdl: &Mdl,
        render: &mut dyn FnMut() -> Option<Vec<[f32; 3]>>,
    ) -> ModelCollision {
        let mut notes: Vec<String> = why.into_iter().collect();
        let mut bounds_hull = mdl_box(mdl);
        if let Some(phy) = phy {
            notes.extend(phy.notes.iter().cloned());
            let hulls = model_space_hulls(phy, mdl);
            if !hulls.is_empty() {
                return ModelCollision {
                    source: HullSource::Phy,
                    hulls,
                    bounds: bounds_hull,
                    notes,
                };
            }
            notes.push("the physics file has no convex pieces".into());
        }
        if let Some(b) = &bounds_hull {
            return ModelCollision {
                source: HullSource::MdlBox,
                hulls: vec![b.clone()],
                bounds: bounds_hull,
                notes,
            };
        }
        notes.push("the model's hull box has no volume".into());
        let points = render().unwrap_or_default();
        let mut work = MAX_HULL_WORK;
        let hull = convex_hull(&points, &mut work).or_else(|| {
            let (lo, hi) = bounds(&points);
            (!points.is_empty() && (0..3).all(|k| hi[k] - lo[k] > MIN_BOX_EXTENT))
                .then(|| Hull::of_box(lo, hi))
        });
        if bounds_hull.is_none() && !points.is_empty() {
            let (lo, hi) = bounds(&points);
            if (0..3).all(|k| hi[k] - lo[k] > MIN_BOX_EXTENT) {
                bounds_hull = Some(Hull::of_box(lo, hi));
            }
        }
        ModelCollision {
            source: HullSource::RenderHull,
            hulls: hull.into_iter().collect(),
            bounds: bounds_hull,
            notes,
        }
    }

    /// The collision of the model at `mdl_path` (`models/x.mdl`): its `.phy` through `assets` (the
    /// [`crate::phy::PhyFormatPlugin`]'s loader), then the fallbacks. Errors only when the model's own file does not load.
    pub fn load(assets: &Assets, files: &dyn FileHost, mdl_path: &str) -> Result<ModelCollision> {
        let mdl: Rc<Mdl> = assets
            .load(files, mdl_path)
            .map_err(|e| Error::Missing(format!("{mdl_path}: {e}")))?;
        let stem = mdl_path.strip_suffix(".mdl").unwrap_or(mdl_path);
        let phy_path = format!("{stem}.phy");
        let (phy, why) = match assets.load::<Phy>(files, &phy_path) {
            Ok(p) => (Some(p), None),
            Err(e) => (None, Some(format!("{phy_path}: {e}"))),
        };
        let vvd_path = format!("{stem}.vvd");
        let mut render = || {
            assets
                .load::<Vvd>(files, &vvd_path)
                .ok()
                .map(|v| v.vertices.iter().map(|x| x.position).collect())
        };
        Ok(ModelCollision::from_parts(
            phy.as_deref(),
            why,
            &mdl,
            &mut render,
        ))
    }
}
