//! A map's static props as collision: each prop's model hulls ([`crate::collision::ModelCollision`]) placed where the prop is,
//! as one [`torque_mapcore::collision::ConvexSet`] a map's collision reads (feature `collide`, off by default).
//!
//! **Which props.** A prop's solid type ([`crate::bsp::StaticProp::solid`]) picks its shape: 6 the model's hulls
//! (physics pieces, or the fallbacks), 2, 3 and 4 the model's bounding box, anything else (0 none, 1 the world's own
//! brushes, 5 custom) nothing.
//!
//! **Space.** The map's own: inches, Source axes, the same as [`crate::bsp::Bsp::build`]'s surfaces; a host places the whole
//! set the way it places the map's drawn surfaces (the quarter turn about z and the inch scale). A prop's angles (pitch, yaw, roll
//! in degrees) turn its hulls as they turn its drawn model.
//!
//! **Bounds.** A set is held to [`MAX_HULLS`] hulls, [`MAX_POINTS`] points and [`MAX_PLANES`] planes together, and the work of
//! placing is paid from a budget of [`MAX_WORK`] (each prop instance costs [`INSTANCE_COST`], each hull it places three units a
//! point and one a plane). A hull that would pass a limit is not placed and is counted in [`PropHulls::left_out`].
//!
//! **Planes.** The collision library refuses a hull over [`HULL_PLANES`] planes (it adds six of its own), so a hull with more
//! keeps the [`HULL_PLANES`] planes covering the most surface; the library closes it with the box of its points, which holds the
//! true hull. Those hulls are counted in [`PropHulls::simplified`].

use std::collections::HashMap;
use std::rc::Rc;

use torque_mapcore::collision::{Contents, ConvexSet, HullRange, Plane, PlaneRef};

use crate::bsp::StaticProp;
use crate::collision::ModelCollision;
use crate::hull::Hull;

/// The most hulls one set holds.
pub const MAX_HULLS: usize = 1 << 18;
/// The most points one set holds, all hulls together.
pub const MAX_POINTS: usize = 1 << 22;
/// The most planes one set holds, all hulls together.
pub const MAX_PLANES: usize = 1 << 22;
/// The work of placing one set may spend (module docs).
pub const MAX_WORK: u64 = 1 << 24;
/// What each prop instance costs, whatever it places.
pub const INSTANCE_COST: u64 = 32;
/// The most planes a placed hull keeps: the collision library's own limit of 96 less the six it adds.
pub const HULL_PLANES: usize = 90;

/// The static props' collision.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PropHulls {
    pub set: ConvexSet,
    /// Props that placed at least one hull.
    pub placed: usize,
    /// Props that are not solid (or collide as the world's brushes).
    pub not_solid: usize,
    /// Props whose model has no collision.
    pub no_model: usize,
    /// Hulls past a limit that were not placed (once the work budget is spent, each remaining solid prop counts as one).
    pub left_out: usize,
    /// Placed hulls that kept only their [`HULL_PLANES`] largest planes.
    pub simplified: usize,
}

/// The rotation of Source angles (pitch, yaw, roll in degrees), row-major.
pub fn rotation(angles: [f32; 3]) -> [[f32; 3]; 3] {
    let [p, y, r] = angles.map(f32::to_radians);
    let (sp, cp) = p.sin_cos();
    let (sy, cy) = y.sin_cos();
    let (sr, cr) = r.sin_cos();
    [
        [cp * cy, sr * sp * cy - cr * sy, cr * sp * cy + sr * sy],
        [cp * sy, sr * sp * sy + cr * cy, cr * sp * sy - sr * cy],
        [-sp, sr * cp, cr * cp],
    ]
}

/// Places `hull` (keeping the planes `kept` names, or all) at `origin` turned by `rot`.
fn place(
    set: &mut ConvexSet,
    hull: &Hull,
    kept: Option<&[u32]>,
    origin: [f32; 3],
    rot: &[[f32; 3]; 3],
) {
    let placed = hull.mapped(&|p| {
        let mut q = origin;
        for (i, row) in rot.iter().enumerate() {
            q[i] += row[0] * p[0] + row[1] * p[1] + row[2] * p[2];
        }
        q
    });
    let point_start = set.point_refs.len() as u32;
    for p in &placed.points {
        set.point_refs.push(set.points.len() as u32);
        set.points.push(*p);
    }
    let plane_start = set.plane_refs.len() as u32;
    let planes = placed
        .planes
        .iter()
        .enumerate()
        .filter(|(i, _)| kept.is_none_or(|k| k.binary_search(&(*i as u32)).is_ok()));
    for (_, pl) in planes {
        set.plane_refs.push(PlaneRef {
            index: set.planes.len() as u32,
            flipped: false,
        });
        set.planes.push(Plane {
            normal: [pl[0], pl[1], pl[2]],
            d: pl[3],
        });
    }
    set.hulls.push(HullRange {
        point_start,
        point_count: placed.points.len() as u32,
        plane_start,
        plane_end: set.plane_refs.len() as u32,
        contents: Contents::SOLID,
    });
}

/// A model's hulls with the planes each keeps (None: all of them).
struct Entry {
    model: Rc<ModelCollision>,
    kept: Vec<Option<Vec<u32>>>,
}

impl Entry {
    fn new(model: Rc<ModelCollision>) -> Entry {
        let kept = model
            .hulls
            .iter()
            .map(|h| (h.planes.len() > HULL_PLANES).then(|| h.largest_planes(HULL_PLANES)))
            .collect();
        Entry { model, kept }
    }
}

/// The collision of `props`: `collision` gives a model's hulls by its path (None when it has none; asked once for each model).
pub fn static_prop_convexes(
    props: &[StaticProp],
    collision: &mut dyn FnMut(&str) -> Option<Rc<ModelCollision>>,
) -> PropHulls {
    let mut out = PropHulls::default();
    let mut cache: HashMap<&str, Option<Rc<Entry>>> = HashMap::new();
    let mut work = 0u64;
    for (i, prop) in props.iter().enumerate() {
        let by_box = matches!(prop.solid, 2..=4);
        // A placement the map gives as no finite number, or one far outside any world, collides as nothing.
        let sane = |v: [f32; 3]| v.iter().all(|c| c.is_finite() && c.abs() < 1.0e7);
        if (prop.solid != 6 && !by_box) || !sane(prop.origin) || !sane(prop.angles) {
            out.not_solid += 1;
            continue;
        }
        if work >= MAX_WORK {
            // The budget is spent: the rest of the solid props are left out whole, without visiting their hulls.
            out.left_out += props[i..]
                .iter()
                .filter(|p| p.solid == 6 || matches!(p.solid, 2..=4))
                .count();
            break;
        }
        work = work.saturating_add(INSTANCE_COST);
        let entry = cache
            .entry(prop.model.as_str())
            .or_insert_with(|| collision(&prop.model).map(|m| Rc::new(Entry::new(m))))
            .clone();
        let Some(entry) = entry else {
            out.no_model += 1;
            continue;
        };
        let bounds_only;
        let (hulls, kept): (&[Hull], &[Option<Vec<u32>>]) = if by_box {
            match &entry.model.bounds {
                Some(b) => {
                    bounds_only = [b.clone()];
                    (&bounds_only, &[None])
                }
                None => (&[], &[]),
            }
        } else {
            (&entry.model.hulls, &entry.kept)
        };
        if hulls.is_empty() {
            out.no_model += 1;
            continue;
        }
        let rot = rotation(prop.angles);
        let mut placed_any = false;
        for (k, h) in hulls.iter().enumerate() {
            let keep = kept.get(k).and_then(|k| k.as_deref());
            let planes = keep.map_or(h.planes.len(), <[u32]>::len);
            let cost = (3 * h.points.len() + planes) as u64;
            let fits = out.set.hulls.len() < MAX_HULLS
                && out.set.points.len() + h.points.len() <= MAX_POINTS
                && out.set.planes.len() + planes <= MAX_PLANES
                && work.saturating_add(cost) <= MAX_WORK;
            if !fits {
                out.left_out += 1;
                continue;
            }
            work += cost;
            out.simplified += usize::from(keep.is_some());
            place(&mut out.set, h, keep, prop.origin, &rot);
            placed_any = true;
        }
        out.placed += usize::from(placed_any);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::HullSource;
    use crate::hull::convex_hull;

    fn prop(model: &str) -> StaticProp {
        StaticProp {
            model: model.into(),
            origin: [0.0; 3],
            angles: [0.0; 3],
            skin: 0,
            solid: 6,
            leaves: Vec::new(),
        }
    }

    /// A model of one round hull of `n` points (hundreds of planes).
    fn round_model(n: usize) -> Rc<ModelCollision> {
        let pts: Vec<[f32; 3]> = (0..n)
            .map(|i| {
                let t = i as f32 * 2.399_963;
                let z = 1.0 - 2.0 * (i as f32 + 0.5) / n as f32;
                let r = (1.0 - z * z).sqrt();
                [r * t.cos() * 10.0, r * t.sin() * 10.0, z * 10.0]
            })
            .collect();
        let mut work = 1 << 26;
        let hull = convex_hull(&pts, &mut work).unwrap();
        Rc::new(ModelCollision {
            source: HullSource::Phy,
            hulls: vec![hull],
            bounds: None,
            notes: Vec::new(),
        })
    }

    #[test]
    fn a_hull_over_the_planes_the_library_takes_keeps_the_largest() {
        let model = round_model(600);
        assert!(model.hulls[0].planes.len() > HULL_PLANES);
        let out = static_prop_convexes(&[prop("a")], &mut |_| Some(model.clone()));
        assert_eq!((out.placed, out.simplified, out.left_out), (1, 1, 0));
        let r = out.set.hulls[0];
        assert_eq!((r.plane_end - r.plane_start) as usize, HULL_PLANES);
        // The kept planes still hold every point of the hull.
        for p in &model.hulls[0].points {
            for pl in &out.set.planes {
                let d = pl.normal[0] * p[0] + pl.normal[1] * p[1] + pl.normal[2] * p[2] + pl.d;
                assert!(d <= 1e-3);
            }
        }
        let geom = torque_collide::map::interior::InteriorGeom::new(&out.set);
        assert_eq!(geom.hulls.len(), 1);
    }

    #[test]
    fn many_props_of_one_huge_model_stop_at_the_limits() {
        let model = round_model(3000);
        let props: Vec<StaticProp> = (0..20_000).map(|_| prop("a")).collect();
        let started = std::time::Instant::now();
        let out = static_prop_convexes(&props, &mut |_| Some(model.clone()));
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        assert!(out.left_out > 0);
        assert!(out.set.points.len() <= MAX_POINTS);
        assert!(out.set.planes.len() <= MAX_PLANES);
        assert!(out.set.hulls.len() <= MAX_HULLS);
        let cost = (3 * model.hulls[0].points.len() + HULL_PLANES) as u64;
        assert!(out.set.hulls.len() as u64 * cost <= MAX_WORK);
        assert_eq!(out.set.hulls.len() + out.left_out, props.len());
    }

    #[test]
    fn many_instances_of_a_tiny_model_pay_the_instance_cost() {
        let model = Rc::new(ModelCollision {
            source: HullSource::MdlBox,
            hulls: vec![Hull::of_box([0.0; 3], [1.0; 3])],
            bounds: None,
            notes: Vec::new(),
        });
        let props: Vec<StaticProp> = (0..(MAX_WORK / INSTANCE_COST) as usize + 1000)
            .map(|_| prop("a"))
            .collect();
        let out = static_prop_convexes(&props, &mut |_| Some(model.clone()));
        assert!(out.left_out >= 1000);
        assert_eq!(out.set.hulls.len() + out.left_out, props.len());
    }

    #[test]
    fn a_placement_with_no_finite_number_collides_as_nothing() {
        let model = round_model(16);
        let mut bad = vec![prop("a"), prop("a"), prop("a"), prop("a")];
        bad[0].origin = [f32::NAN, 0.0, 0.0];
        bad[1].angles = [0.0, f32::INFINITY, 0.0];
        bad[2].origin = [1.0e9, 0.0, 0.0];
        let out = static_prop_convexes(&bad, &mut |_| Some(model.clone()));
        assert_eq!((out.not_solid, out.placed), (3, 1));
        assert!(out.set.points.iter().flatten().all(|c| c.is_finite()));
    }
}
