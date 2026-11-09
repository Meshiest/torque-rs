//! Plugin ghosts drawn as shapes: a plugin says, for a ghost class, how a record becomes a shape the client's renderer draws
//! ([`ShapeFn`], [`ShapeDraw`]), and a client's world keeps those for the classes that have one ([`DrawnShapes`]).
//!
//! The renderer draws the shape as it draws a TGE ghost's: lit, shadowed, culled, with the transform the record's first
//! [`Xf`](crate::codec::Xf) gives it (shown between the server's updates as the class's [`Replicate`] mark says) and the scale and skin
//! here. What the shape is comes from the record, and from the datablock it names when the draw function reads one
//! ([`DatablockFields`]). A draw that says its ghost rests ([`ShapeDraw::rests`]: a sleeping prop) lets a client draw it with
//! its still geometry until a record says otherwise.

use std::fmt;
use std::sync::Arc;

use super::{NetRecord, PluginClass, Record, Replicate};
use crate::Value;

/// The most any one axis of a drawn shape's scale may be (a hostile server's record cannot ask for more).
pub const MAX_SCALE: f32 = 1024.0;

/// What a plugin ghost is drawn as: a shape file of the game's files, its scale and the texture its first material shows in place of
/// its own.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeDraw {
    /// The shape file's path, as a TGE datablock's `shapeFile` gives it (a `.dts`, or a format the engine imports).
    pub shape: String,
    /// The scale along each axis, each finite and at most [`MAX_SCALE`] across (a shape with any other is not drawn).
    pub scale: [f32; 3],
    /// A texture file shown in place of the first material's (a skin); none keeps the shape's own.
    pub skin: Option<String>,
    /// The ghost rests (a sleeping prop): it stays where its record puts it until a record says otherwise, so a client may draw
    /// it with its still geometry (made once, kept on the GPU) instead of every frame, and draws it every frame again once it
    /// stops resting.
    pub rests: bool,
}

impl ShapeDraw {
    /// The shape `shape`, unscaled and unskinned.
    pub fn new(shape: impl Into<String>) -> ShapeDraw {
        ShapeDraw {
            shape: shape.into(),
            scale: [1.0; 3],
            skin: None,
            rests: false,
        }
    }

    /// The shape resting (`rests`) or not.
    pub fn resting(mut self, rests: bool) -> ShapeDraw {
        self.rests = rests;
        self
    }

    /// The shape at `scale` along each axis.
    pub fn scaled(mut self, scale: [f32; 3]) -> ShapeDraw {
        self.scale = scale;
        self
    }

    /// The shape with `skin` in place of its first material's texture.
    pub fn skinned(mut self, skin: impl Into<String>) -> ShapeDraw {
        self.skin = Some(skin.into());
        self
    }

    /// Whether the renderer may draw it: a path, and a scale that is finite and within [`MAX_SCALE`].
    pub fn is_drawable(&self) -> bool {
        !self.shape.is_empty()
            && self
                .scale
                .iter()
                .all(|s| s.is_finite() && s.abs() <= MAX_SCALE)
    }
}

/// The datablocks a client holds, as a draw function reads them: a field of a datablock by its id (a plugin datablock's fields are
/// its record's, by the names the record gives them).
pub trait DatablockFields {
    /// Element `index` of field `name` of datablock `id`.
    fn field(&self, id: u32, name: &str, index: u32) -> Option<&Value>;

    /// The text of field `name` of datablock `id` (its first element).
    fn text(&self, id: u32, name: &str) -> Option<&str> {
        match self.field(id, name, 0)? {
            Value::Str(s) => std::str::from_utf8(s).ok(),
            _ => None,
        }
    }
}

type DrawFn = dyn Fn(&dyn Record, &dyn DatablockFields) -> Option<ShapeDraw> + Send + Sync;

/// How a record of one ghost class becomes a shape ([module docs](self)). A cheap handle: clones share the function.
#[derive(Clone)]
pub struct ShapeFn(Arc<DrawFn>);

impl ShapeFn {
    /// The function `draw` over records of type `G`; the shape of a record of any other type is none. Called every frame the ghost is
    /// in view, so it should only read.
    pub fn new<G: NetRecord>(
        draw: impl Fn(&G, &dyn DatablockFields) -> Option<ShapeDraw> + Send + Sync + 'static,
    ) -> ShapeFn {
        ShapeFn(Arc::new(move |record, datablocks| {
            draw(record.as_any().downcast_ref::<G>()?, datablocks)
        }))
    }

    /// The shape `record` is drawn as, if it is drawn.
    pub fn draw(&self, record: &dyn Record, datablocks: &dyn DatablockFields) -> Option<ShapeDraw> {
        (self.0)(record, datablocks).filter(ShapeDraw::is_drawable)
    }
}

impl fmt::Debug for ShapeFn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ShapeFn")
    }
}

impl PartialEq for ShapeFn {
    fn eq(&self, other: &ShapeFn) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// The ghost classes a client draws as shapes, each with its [`ShapeFn`]: kept by a client's world and read by the renderer. Empty
/// when no plugin draws a class.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DrawnShapes {
    classes: Vec<(PluginClass, ShapeFn)>,
}

impl DrawnShapes {
    /// Draws the ghosts of `class` as `draw` says (a class named again keeps its first).
    pub fn insert(&mut self, class: PluginClass, draw: ShapeFn) {
        if !self.contains(class) {
            self.classes.push((class, draw));
        }
    }

    /// Whether the ghosts of `class` are drawn as shapes.
    pub fn contains(&self, class: PluginClass) -> bool {
        self.classes.iter().any(|(c, _)| *c == class)
    }

    /// Whether the ghosts of `class` are shown between the server's updates (blended from where they were drawn to where the last
    /// update put them) and not run ahead by the client's own simulation.
    pub fn interpolates(&self, class: PluginClass) -> bool {
        self.contains(class) && class.replicate() == Some(Replicate::Interpolated)
    }

    /// The drawn classes, in the order they were added.
    pub fn classes(&self) -> impl Iterator<Item = PluginClass> + '_ {
        self.classes.iter().map(|(c, _)| *c)
    }

    /// Whether no class is drawn.
    pub fn is_empty(&self) -> bool {
        self.classes.is_empty()
    }

    /// The shape the ghost of `class` with `record` is drawn as (none: not a drawn class, or the record is not drawn).
    pub fn draw(
        &self,
        class: PluginClass,
        record: &dyn Record,
        datablocks: &dyn DatablockFields,
    ) -> Option<ShapeDraw> {
        self.classes
            .iter()
            .find(|(c, _)| *c == class)?
            .1
            .draw(record, datablocks)
    }
}
