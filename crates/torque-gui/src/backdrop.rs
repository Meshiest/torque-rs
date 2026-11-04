//! Backdrops: what a GUI control asks the renderer to draw under it before it draws (a blur of what the frame holds there, a
//! tint over it, a rounded rectangle). A GUI extension puts a [`BackdropDraw`] in the draw list as an extra
//! ([`crate::draw::ExtraDraw`]); a renderer that claims it draws it between the batches before and after it.

use crate::GuiDraws;

/// The largest blur radius (pixels) and corner radius a backdrop may have (larger ones are capped).
pub const MAX_BLUR_RADIUS: u32 = 64;
pub const MAX_CORNER_RADIUS: u32 = 64;

/// A control's backdrop: before the control draws, what the frame holds under `rect` (cut by `clip`) is blurred by `radius`
/// pixels (0: not blurred), the `tint` laid over it, and the result put back inside the rounded rectangle of corner radius
/// `corner` (1-pixel anti-aliased edge), its alpha times `opacity` (the control's opacity). The one that makes it caps every
/// value: radius to [`MAX_BLUR_RADIUS`], corner to [`MAX_CORNER_RADIUS`], opacity to 0..=1; the renderer caps them again.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BackdropDraw {
    /// The batch the backdrop goes before.
    pub at_batch: usize,
    /// The control's rectangle and the clip rectangle it draws in (x, y, width, height).
    pub rect: [i32; 4],
    pub clip: [i32; 4],
    pub radius: u32,
    pub tint: [u8; 4],
    pub corner: u32,
    pub opacity: f32,
}

/// The backdrops of a frame, in order.
pub fn backdrops(draws: &GuiDraws) -> impl Iterator<Item = &BackdropDraw> {
    draws.extras_of::<BackdropDraw>().map(|(_, b)| b)
}
