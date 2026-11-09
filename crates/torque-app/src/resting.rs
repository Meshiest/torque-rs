//! Plugin ghosts at rest as still geometry ([`torque_render_scene::moving::resting`]): a sleeping prop is drawn as one copy of
//! its shape's static batch ([`WorldView::set_static_batch`]: kept on the GPU, culled there, casting into the cached shadows)
//! instead of being posed, built and uploaded every frame.
//!
//! Each frame [`Resting::update`] asks every plugin ghost whether it rests (live: its draw reads its record and the datablocks
//! it names, either of which may change without the other) and where, and gives back the ghosts it drew still: exactly those
//! the frame's moving shapes leave out ([`torque_render_scene::moving::Hooks::still`]). A shape's batch is laid out again only
//! when its resting copies change (one wakes, one falls asleep, one is put somewhere else). A ghost it cannot keep still is
//! drawn with the moving shapes instead, never left undrawn: a shape past [`MAX_RESTING_SHAPES`], a shape that did not load,
//! or a batch the view's triangle budget would cut short.
//!
//! A batch is one shape (by its file): a copy's uniform scale is in its transform, so a server's many scales of one shape cost
//! one mesh.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use torque_ghost_world::tge::TgeClientSet;
use torque_ghost_world::world::ClientWorld;
use torque_render_scene::Assets;
use torque_render_scene::images::scale_columns;
use torque_render_scene::math::Mat;
use torque_render_scene::mounts::pose_matrix;
use torque_render_scene::moving::{draw_rests, drawn_where_sent};
use torque_render_scene::world::ShapeWorld as _;

use crate::render::StaticBatch;
use crate::view::{MAX_RESTING_SHAPES, RESTING_BATCHES, SceneShape, WorldView};

type World = ClientWorld<TgeClientSet>;

/// One shape's batch: its id, its mesh (none for a shape that did not load), its copies as laid out last, and whether the view
/// holds all of them.
struct Kept {
    id: u32,
    mesh: Option<StaticBatch>,
    copies: BTreeMap<u32, Mat>,
    laid_out: bool,
}

/// The resting ghosts kept as static batches ([module docs](self)).
#[derive(Default)]
pub(crate) struct Resting {
    kept: BTreeMap<String, Kept>,
    /// The ids of batches no shape holds now, and the ones to take out of the view at the next update.
    free: Vec<u32>,
    retired: Vec<u32>,
    next: u32,
    /// The shapes read, by file.
    shapes: HashMap<String, Option<SceneShape>>,
    warned: bool,
}

impl Resting {
    /// The resting ghosts of `world` put into `view`'s static batches (module docs): the ghosts drawn still this frame.
    pub(crate) fn update(
        &mut self,
        view: &mut WorldView,
        assets: &dyn Assets,
        world: &World,
    ) -> BTreeSet<u32> {
        for id in self.retired.drain(..) {
            view.remove_static_batch(id);
            self.free.push(id);
        }
        let mut want = wanted(world);
        let gone: Vec<String> = self
            .kept
            .keys()
            .filter(|k| !want.contains_key(*k))
            .cloned()
            .collect();
        for key in gone {
            if let Some(k) = self.kept.remove(&key) {
                view.remove_static_batch(k.id);
                self.free.push(k.id);
            }
        }
        let mut still = BTreeSet::new();
        for (shape, copies) in std::mem::take(&mut want) {
            if !self.kept.contains_key(&shape) {
                let Some(id) = self.free.pop().or_else(|| {
                    (self.next < MAX_RESTING_SHAPES).then(|| {
                        self.next += 1;
                        RESTING_BATCHES + self.next - 1
                    })
                }) else {
                    if !self.warned {
                        self.warned = true;
                        view.warn(format!(
                            "resting shapes: more than {MAX_RESTING_SHAPES} shapes rest at once; the others are drawn every frame"
                        ));
                    }
                    continue;
                };
                let mesh = self
                    .shapes
                    .entry(shape.clone())
                    .or_insert_with(|| SceneShape::load(assets, &shape))
                    .as_ref()
                    .and_then(|s| s.batch(assets));
                self.kept.insert(
                    shape.clone(),
                    Kept {
                        id,
                        mesh,
                        copies: BTreeMap::new(),
                        laid_out: false,
                    },
                );
            }
            let Some(k) = self.kept.get_mut(&shape) else {
                continue;
            };
            if k.copies != copies {
                k.copies = copies;
                k.laid_out = match &mut k.mesh {
                    Some(mesh) => {
                        mesh.transforms = k.copies.values().copied().collect();
                        // A batch the budget cuts short is not drawn at all: its ghosts are drawn every frame instead.
                        let short = view.set_static_batch(k.id, mesh).is_some();
                        if short {
                            view.remove_static_batch(k.id);
                        }
                        !short
                    }
                    None => false,
                };
            }
            if k.laid_out {
                still.extend(k.copies.keys().copied());
            }
        }
        still
    }

    /// The ghosts drawn still now.
    pub(crate) fn count(&self) -> usize {
        self.kept
            .values()
            .filter(|k| k.laid_out)
            .map(|k| k.copies.len())
            .sum()
    }

    /// Forgets the shapes read and the batches made (a new connection may name other files): the batches are taken out of the
    /// view at the next update.
    pub(crate) fn forget(&mut self) {
        self.shapes.clear();
        self.retired
            .extend(std::mem::take(&mut self.kept).into_values().map(|k| k.id));
    }
}

/// The resting plugin ghosts of `world` by shape file (its path as the files name it), each with its place (its pose's turn
/// and move, scaled by its uniform scale), read live ([module docs](self)).
pub(crate) fn wanted(world: &World) -> BTreeMap<String, BTreeMap<u32, Mat>> {
    let mut want: BTreeMap<String, BTreeMap<u32, Mat>> = BTreeMap::new();
    for (&index, g) in &world.ghosts {
        if !g.class.is_some_and(|c| world.drawn_class(c)) {
            continue;
        }
        let Some(draw) = world.drawn_shape(g).filter(draw_rests) else {
            continue;
        };
        if !drawn_where_sent(world, index) {
            continue;
        }
        let Some((m, _)) = g.pose().and_then(|p| pose_matrix(&p, None)) else {
            continue;
        };
        let s = draw.scale[0];
        let mut shape = draw.shape;
        if shape.contains('\\') {
            shape = shape.replace('\\', "/");
        }
        want.entry(shape)
            .or_default()
            .insert(index, scale_columns(&m, [s; 3]));
    }
    want
}

#[cfg(test)]
mod tests {
    use super::*;
    use torque_engine::net_classes::{DatablockRef, ShapeDraw, Xf};
    use torque_engine::{GhostRecord, Replicate};
    use torque_ghost_world::tge::TgeDatablock;
    use torque_ghost_world::world::Ghost;
    use torque_ghosts::plugin::{DrawnShapes, PluginClass, ShapeFn};
    use torque_ghosts::{TgeClass, TgeRecord, Value};

    /// A prop as a client knows it: where it is and which datablock it is.
    #[derive(Debug, Clone, PartialEq, Default, GhostRecord)]
    struct Prop {
        #[net(name = "transform")]
        pose: Xf,
        data_block: DatablockRef,
    }

    /// A world of one resting prop (ghost 1) of datablock 3, drawn as its datablock's `model` says; the datablock not there
    /// yet.
    fn world() -> World {
        let class = PluginClass::ghost::<Prop>("Prop", Replicate::Interpolated);
        let mut drawn = DrawnShapes::default();
        drawn.insert(
            class,
            ShapeFn::new::<Prop>(|prop, blocks| {
                let model = blocks.text(prop.data_block.0?, "model")?;
                Some(ShapeDraw::new(model).resting(true))
            }),
        );
        let mut w = World {
            plugin_shapes: drawn,
            ..Default::default()
        };
        let prop = Prop {
            data_block: DatablockRef(Some(3)),
            ..Prop::default()
        };
        w.ghosts.insert(
            1,
            Ghost {
                class: Some(TgeClass::Plugin(class)),
                serial: 1,
                always: false,
                record: Some(TgeRecord::Plugin(class, Box::new(prop))),
                part: (),
                position: None,
                updates: 0,
            },
        );
        w
    }

    fn datablock(model: &str) -> TgeDatablock {
        TgeDatablock {
            class: "PropData".to_string(),
            fields: vec![("model".to_string(), 0, Value::str(model))],
        }
    }

    #[test]
    fn a_ghosts_shape_is_read_live_so_a_datablock_arriving_or_changing_after_it_counts() {
        let mut w = world();
        // No datablock yet: nothing to draw, so nothing rests (the moving shapes draw nothing of it either).
        assert!(wanted(&w).is_empty());
        // The datablock comes with no update of the ghost: it rests as that shape at once.
        w.datablocks.insert(3, datablock("art/crate.dts"));
        let want = wanted(&w);
        assert_eq!(want.keys().collect::<Vec<_>>(), ["art/crate.dts"]);
        assert!(want["art/crate.dts"].contains_key(&1));
        // The datablock names another file (a backslashed one), again with no update of the ghost.
        w.datablocks.insert(3, datablock(r"art\barrel.dts"));
        assert_eq!(wanted(&w).keys().collect::<Vec<_>>(), ["art/barrel.dts"]);
    }
}
