//! The caps the importers and the consumers share. Each consumer keeps its own work budget (`&mut dyn FnMut(u64) -> bool`,
//! [`Charge`]) and its own query caps; these are the sizes an importer may hand over at most, so the consumers can size their
//! work against them.

/// A work budget: charges `units` and says whether there was room (false: the caller stops and gives up what it was making).
pub type Charge<'a> = dyn FnMut(u64) -> bool + 'a;

/// The samples a side of every heightfield (the grid is square and wraps). A legacy `.ter` is 256; a format
/// with another side adds it here first.
pub const HEIGHTFIELD_SIDE: usize = 256;

/// The samples of a heightfield.
pub const HEIGHTFIELD_SAMPLES: usize = HEIGHTFIELD_SIDE * HEIGHTFIELD_SIDE;

/// The most material layers a heightfield has (legacy `.ter`: eight slots).
pub const MAX_HEIGHTFIELD_LAYERS: usize = 8;

/// The square size a heightfield is placed with, at most (and at least 1).
pub const MAX_SQUARE_SIZE: i32 = 4_096;

/// The longest side of a baked lightmap page a model's rectangles are read from (a larger page gets no rectangles).
pub const MAX_SOURCE_SIDE: u32 = 2048;

/// The longest side of one lightmap rectangle (a larger rectangle is dropped: its surfaces are drawn unlit by the bake).
pub const MAX_RECT_SIDE: u32 = 512;
