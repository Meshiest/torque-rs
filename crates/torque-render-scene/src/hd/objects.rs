//! The enhanced renderer's moving objects: the frame's shape draws ([`crate::moving::ShapeDraws`]) as one [`DynamicObject`] a
//! ghost, its draws converted ([`HdDraw::from_fixed`]), its bounds, its transform now and at the last frame and its vertices'
//! last places; with a host's kept builds ([`crate::shapecache`]) a ghost whose builds did not change takes the last frame's
//! object again.

use std::collections::{BTreeMap, HashMap};

use crate::Draw;
use crate::math::Mat;
use crate::mounts::{pose_matrix, render_pose};
use crate::moving::{ShapeDraws, is_image, is_shadow_body, is_view_model};
use crate::shapes::MeshPlace;
use crate::world::{ShapeKind, ShapeWorld};

use super::{
    DynamicObject, HdDraw, MAX_PREVIOUS_VERTICES, NodePlace, bounding_sphere, material,
    nodes_sphere, post,
};

/// What a control sync keeps of one ghost's object for the next ([`crate::shapecache`]): the stamps of the builds its draws came
/// from, its bounds and its own stamp (the object itself is the next sync's last frame).
#[derive(Debug, Clone, PartialEq)]
pub struct KeptObject {
    builds: Vec<u64>,
    centre: [f32; 3],
    radius: f32,
    stamp: u64,
}

/// A ghost's draws in a [`ShapeDraws`]: whether it has translucency, its opaque draws, its translucent entries.
type GhostDraws<'a> = (bool, Vec<&'a Draw>, Vec<&'a [Draw]>);

/// One draw a fresh object converts: the fixed-function draw, its texture map (none: the frame's `local`), for a kept build in its
/// meshes' own space its vertices' meshes and its call's first place in the object's, and for another ghost's kept build the
/// labels its draw's label names in place of the object's (in `labels`, [`crate::shapecache::Kept::relabel`]).
#[derive(Clone, Copy)]
struct Source<'a> {
    draw: &'a Draw,
    map: Option<usize>,
    nodes: Option<(&'a [u16], u32)>,
    label: Option<usize>,
}

impl<'a> Source<'a> {
    fn of(draw: &'a Draw) -> Source<'a> {
        Source {
            draw,
            map: None,
            nodes: None,
            label: None,
        }
    }
}

/// What [`objects_of`] makes of one ghost: the last frame's draws again (a kept object), or its sources to convert (each with its
/// map in `maps`).
enum Made<'a> {
    Same {
        draws: Vec<HdDraw>,
        centre: [f32; 3],
        radius: f32,
        stamp: u64,
    },
    Fresh {
        sources: Vec<Source<'a>>,
        maps: Vec<Vec<usize>>,
        labels: Vec<(&'a str, String)>,
        player: bool,
        stamp: Option<u64>,
    },
}

/// One ghost's object on its way: its ghost, translucency, last frame's object, kept builds, transform and draws, and its kept
/// builds' meshes' places this frame (those in their meshes' own space, in call order) and whether any of its draws is in the
/// world.
struct Pending<'a> {
    ghost: u32,
    translucent: bool,
    before: Option<usize>,
    builds: Option<Vec<u64>>,
    transform: Mat,
    made: Made<'a>,
    places: Vec<MeshPlace>,
    /// Each place's kept build (its call's stamp).
    builds_of_places: Vec<u64>,
    in_world: bool,
}

impl Pending<'_> {
    /// Its work: the vertices it converts (one it keeps only copies its positions: a sixteenth as much).
    fn work(&self) -> usize {
        match &self.made {
            Made::Same { draws, .. } => draws.iter().map(|d| d.vertices.len()).sum::<usize>() / 16,
            Made::Fresh { sources, .. } => sources.iter().map(|s| s.draw.vertices.len()).sum(),
        }
    }
}

/// The shape ghosts' objects of `shaped`, each moving from where `drawn` (the last frame's objects) had it. With `kept` (a control
/// sync's), a ghost whose builds are the kept ones, and whose last frame's object has the kept stamp, takes that object's draws
/// (the same draws: they move out of `drawn`) instead of converting its own; every object has a stamp then. The conversions, the
/// bounds and the previous positions are made over a few threads ([`crate::par`]), each object as one thread makes it.
pub fn objects_of<W: ShapeWorld + ?Sized>(
    world: &W,
    shaped: &ShapeDraws,
    local: &[usize],
    drawn: &mut [DynamicObject],
    kept: Option<&mut HashMap<u32, KeptObject>>,
) -> Vec<DynamicObject> {
    crate::shapecache::with_kept(|cache| objects_with(world, shaped, local, drawn, kept, cache))
}

fn objects_with<W: ShapeWorld + ?Sized>(
    world: &W,
    shaped: &ShapeDraws,
    local: &[usize],
    drawn: &mut [DynamicObject],
    kept: Option<&mut HashMap<u32, KeptObject>>,
    cache: &crate::shapecache::Kept<'_>,
) -> Vec<DynamicObject> {
    let previous: BTreeMap<u32, usize> = drawn
        .iter()
        .enumerate()
        .map(|(i, o)| (o.ghost, i))
        .collect();
    // Each ghost's draws in `shaped` (its calls' that were not kept): the opaque ones, then the translucent entries.
    let mut by_ghost: BTreeMap<u32, GhostDraws<'_>> = BTreeMap::new();
    for (d, &g) in shaped.opaque.iter().zip(&shaped.opaque_owners) {
        by_ghost.entry(g).or_default().1.push(d);
    }
    for ((_, ds), &g) in shaped.translucent.iter().zip(&shaped.translucent_owners) {
        let e = by_ghost.entry(g).or_default();
        e.0 = true;
        e.2.push(ds);
    }
    for g in shaped.calls.keys() {
        by_ghost.entry(*g).or_default();
    }
    let keeping = kept.is_some();
    let mut pending: Vec<Pending<'_>> = Vec::new();
    for (ghost, (translucent, opaque, translucent_entries)) in by_ghost {
        let before = previous.get(&ghost).copied();
        let calls = shaped.calls.get(&ghost).map_or(&[][..], Vec::as_slice);
        // Every build of the ghost kept (none made apart).
        let builds: Option<Vec<u64>> =
            (keeping && !calls.is_empty() && calls.iter().all(|c| c.stamp != 0))
                .then(|| calls.iter().map(|c| c.stamp).collect());
        let same = kept.as_ref().and_then(|k| {
            let e = k
                .get(&ghost)
                .filter(|e| Some(&e.builds) == builds.as_ref())?;
            let b = before?;
            (drawn[b].stamp == Some(e.stamp)).then(|| (e.clone(), b))
        });
        let translucent = translucent || calls.iter().any(|c| c.translucent);
        if calls.is_empty() && opaque.is_empty() && translucent_entries.is_empty() {
            continue;
        }
        // The kept builds in their meshes' own space: each call's places and its draws' vertices' meshes, in call order.
        fn named<'c, 'k>(
            c: &'c crate::shapecache::Call,
            cache: &crate::shapecache::Kept<'k>,
        ) -> Option<(&'c [MeshPlace], &'k [Vec<u16>])> {
            let p = c.places.as_deref().filter(|_| c.stamp != 0)?;
            let n = cache
                .nodes(c.stamp)
                .filter(|n| cache.draws(c.stamp).is_some_and(|d| d.len() == n.len()))?;
            Some((p, n))
        }
        let mut places: Vec<MeshPlace> = Vec::new();
        let mut builds_of_places: Vec<u64> = Vec::new();
        let mut bases: Vec<Option<u32>> = Vec::with_capacity(calls.len());
        for c in calls {
            bases.push(named(c, cache).map(|(p, _)| {
                let base = places.len() as u32;
                places.extend_from_slice(p);
                builds_of_places.extend(std::iter::repeat_n(c.stamp, p.len()));
                base
            }));
        }
        let in_world = places.is_empty()
            || calls.is_empty()
            || calls
                .iter()
                .zip(&bases)
                .any(|(c, b)| b.is_none() && c.count > 0);
        let made = match same {
            Some((e, b)) => Made::Same {
                draws: std::mem::take(&mut drawn[b].draws),
                centre: e.centre,
                radius: e.radius,
                stamp: e.stamp,
            },
            None => {
                let mut front: Vec<Source<'_>> = Vec::new();
                let mut back: Vec<Source<'_>> = Vec::new();
                let mut maps: Vec<Vec<usize>> = Vec::new();
                let mut labels: Vec<(&str, String)> = Vec::new();
                if calls.is_empty() {
                    front.extend(opaque.iter().map(|d| Source::of(d)));
                    for ds in &translucent_entries {
                        back.extend(ds.iter().map(Source::of));
                    }
                } else {
                    // The calls in order: a kept one's draws from the cache (its textures through this frame's set), the others'
                    // from `shaped` in their order (a group of translucent calls has one entry there).
                    let mut opaque = opaque.iter();
                    let mut entries = translucent_entries.iter();
                    let mut entry: Option<std::slice::Iter<'_, Draw>> = None;
                    let mut group = None;
                    for (c, base) in calls.iter().zip(&bases) {
                        if c.translucent && group != Some(c.group) {
                            entry = entries.next().map(|ds| ds.iter());
                        }
                        group = Some(c.group);
                        let into = if c.translucent { &mut back } else { &mut front };
                        if c.stamp == 0 {
                            if c.translucent {
                                if let Some(e) = entry.as_mut() {
                                    into.extend(e.by_ref().take(c.count).map(Source::of));
                                }
                            } else {
                                into.extend(opaque.by_ref().take(c.count).map(|d| Source::of(d)));
                            }
                            continue;
                        }
                        let Some(kept) = cache.draws(c.stamp) else {
                            continue;
                        };
                        let nodes = base.and_then(|b| named(c, cache).map(|(_, n)| (n, b)));
                        let map = (!c.textures.is_empty()).then(|| {
                            let size = c.textures.iter().map(|t| t.0 + 1).max().unwrap_or(0);
                            let mut map = vec![usize::MAX; size];
                            for (was, now) in &c.textures {
                                map[*was] = local.get(*now).copied().unwrap_or(usize::MAX);
                            }
                            maps.push(map);
                            maps.len() - 1
                        });
                        let label = cache.relabel(c.stamp, ghost).map(|l| {
                            labels.push(l);
                            labels.len() - 1
                        });
                        into.extend(kept.iter().enumerate().map(|(k, d)| Source {
                            draw: d,
                            map,
                            nodes: nodes.and_then(|(n, b)| n.get(k).map(|n| (n.as_slice(), b))),
                            label,
                        }));
                    }
                }
                let mut sources = front;
                sources.append(&mut back);
                // A Player's body (the material's player bit; not its images).
                let player = world
                    .records()
                    .ghosts
                    .get(&ghost)
                    .is_some_and(|g| world.kind_of(g) == ShapeKind::Player);
                Made::Fresh {
                    sources,
                    maps,
                    labels,
                    player,
                    stamp: keeping.then(crate::shapecache::next_stamp),
                }
            }
        };
        // The transform its draws were made with.
        let transform = shaped.transforms.get(&ghost).copied().unwrap_or_else(|| {
            world
                .records()
                .ghosts
                .get(&ghost)
                .and_then(|g| {
                    render_pose(world, ghost)
                        .as_ref()
                        .and_then(|p| pose_matrix(p, g.scale()))
                })
                .map_or(Mat::IDENTITY, |(m, _)| m)
        });
        pending.push(Pending {
            ghost,
            translucent,
            before,
            builds,
            transform,
            made,
            places,
            builds_of_places,
            in_world,
        });
    }
    // Each object made (a thread's share of them each).
    let drawn: &[DynamicObject] = drawn;
    let made = crate::par::map_owned(pending, Pending::work, |p| make_object(p, drawn, local));
    let mut out = Vec::with_capacity(made.len());
    let mut now_kept: HashMap<u32, KeptObject> = HashMap::new();
    for (o, builds) in made {
        if let (Some(b), Some(stamp)) = (builds, o.stamp) {
            now_kept.insert(
                o.ghost,
                KeptObject {
                    builds: b,
                    centre: o.centre,
                    radius: o.radius,
                    stamp,
                },
            );
        }
        out.push(o);
    }
    if let Some(k) = kept {
        *k = now_kept;
    }
    out
}

/// Gives each of `objects` its motion from `before` (the last frame's objects): the last transform and vertex places of the object
/// of the same id there. An object with a stamp brings its own ([`objects_of`] made it from the same last frame, whose kept objects
/// gave their draws up to it).
pub fn carry_motion(objects: &mut [DynamicObject], before: &[DynamicObject]) {
    if objects.iter().all(|o| o.stamp.is_some()) {
        return;
    }
    let before: HashMap<u32, &DynamicObject> = before.iter().map(|b| (b.ghost, b)).collect();
    for o in objects.iter_mut().filter(|o| o.stamp.is_none()) {
        if let Some(before) = before.get(&o.ghost) {
            o.prev_transform = before.transform;
            o.previous = o.previous_from(before);
        }
    }
}

/// [`objects_of`]'s object of `p` (and its kept builds): its draws converted (or the last frame's again), its bounds and its
/// previous positions.
fn make_object(
    p: Pending<'_>,
    drawn: &[DynamicObject],
    local: &[usize],
) -> (DynamicObject, Option<Vec<u64>>) {
    let Pending {
        ghost,
        translucent,
        before,
        builds,
        transform,
        made,
        places,
        builds_of_places,
        in_world,
    } = p;
    let (draws, sphere, stamp) = match made {
        Made::Same {
            draws,
            centre,
            radius,
            stamp,
        } => (draws, Some((centre, radius)), Some(stamp)),
        Made::Fresh {
            sources,
            maps,
            labels,
            player,
            stamp,
        } => {
            let draws: Vec<HdDraw> = sources
                .iter()
                .map(|s| {
                    let textures = s.map.map_or(local, |m| maps[m].as_slice());
                    let mut h = HdDraw::from_fixed(s.draw, material::KIND_SHAPE, textures);
                    // Another ghost's build: its draws named as this ghost's own build names them.
                    if let Some((from, to)) = s.label.map(|i| &labels[i])
                        && let Some(at) = h.label.find(from)
                    {
                        h.label.replace_range(at..at + from.len(), to);
                    }
                    if is_shadow_body(&s.draw.label) {
                        h = h.with_material(material::SHADOW_ONLY);
                    }
                    if is_view_model(&s.draw.label) {
                        h = h.with_material(material::VIEW_ONLY);
                    }
                    if let Some((nodes, base)) = s.nodes {
                        for (v, n) in h.vertices.iter_mut().zip(nodes) {
                            v.material |= material::NODE;
                            v.extra = base + u32::from(*n);
                        }
                    }
                    // The player bit is the body's: its held items shade as any shape (in the first person a camera-facing rim
                    // none of the sun's shadows dim would outshine a held item's own sun and shade).
                    if player && !is_image(&s.draw.label) {
                        h.with_material(material::PLAYER)
                    } else {
                        h
                    }
                })
                .collect();
            (draws, None, stamp)
        }
    };
    let reused = sphere.is_some();
    let prev_transform = before.map_or(transform, |b| drawn[b].transform);
    // The meshes in their own space: each place's last frame (the last object's, the same draws; else the last object's place of
    // the same kept build's same mesh: an object made afresh because one of its builds changed or is not kept keeps its other
    // builds' animation as their motion; else moved as the shape moved).
    let nodes: Vec<NodePlace> = if places.is_empty() {
        Vec::new()
    } else {
        let last = before.map_or(&[][..], |b| drawn[b].nodes.as_slice());
        let same = reused && last.len() == places.len();
        // The last object's places of each build, in order.
        let mut by_build: HashMap<u64, Vec<&NodePlace>> = HashMap::new();
        if !same {
            for q in last.iter().filter(|q| q.build != 0) {
                by_build.entry(q.build).or_default().push(q);
            }
        }
        let mut taken: HashMap<u64, usize> = HashMap::new();
        let rigid = (prev_transform != transform)
            .then(|| Mat::mul(&prev_transform, &post::affine_inverse_scaled(&transform)));
        places
            .iter()
            .zip(&builds_of_places)
            .enumerate()
            .map(|(k, (p, &build))| {
                let was = if same {
                    Some(last[k].now)
                } else {
                    let i = taken.entry(build).or_insert(0);
                    *i += 1;
                    by_build
                        .get(&build)
                        .and_then(|v| v.get(*i - 1))
                        .map(|q| q.now)
                };
                NodePlace {
                    now: p.now,
                    before: match (was, &rigid) {
                        (Some(w), _) => w,
                        (None, Some(r)) => Mat::mul(r, &p.now),
                        (None, None) => p.now,
                    },
                    normal: p.normal,
                    sphere: p.sphere,
                    build,
                }
            })
            .collect()
    };
    let (centre, radius) = if nodes.is_empty() {
        sphere.unwrap_or_else(|| bounding_sphere(&draws))
    } else {
        // (The draws in the world, when some are: a texture still being decoded, a skin.)
        let points: Vec<[f32; 3]> = if in_world {
            draws
                .iter()
                .flat_map(|d| &d.vertices)
                .filter(|v| v.material & material::NODE == 0)
                .map(|v| v.position)
                .collect()
        } else {
            Vec::new()
        };
        nodes_sphere(&nodes, &points)
    };
    let mut o = DynamicObject {
        ghost,
        prev_transform,
        transform,
        translucent,
        centre,
        radius,
        draws,
        previous: None,
        stamp,
        nodes,
    };
    o.previous = if !o.nodes.is_empty() {
        None
    } else if reused {
        // The last frame's draws were these: each vertex was where it is (as `previous_from` finds them).
        let n: usize = o.draws.iter().map(|d| d.vertices.len()).sum();
        (n <= MAX_PREVIOUS_VERTICES && !o.draws.is_empty()).then(|| {
            o.draws
                .iter()
                .flat_map(|d| d.vertices.iter().map(|v| v.position))
                .collect()
        })
    } else {
        before.and_then(|b| o.previous_from(&drawn[b]))
    };
    (o, builds)
}

#[cfg(test)]
#[path = "objects_tests.rs"]
mod tests;
