//! What the controls read of their `GuiControlProfile`: the fields as the profile holds them, and the profile's bitmap array
//! (`constructBitmapArray`), which the scroll, window, check box, radio, popup and slider controls draw from.
//!
//! **The bitmap array** (TGE 1.4's algorithm): the profile's `bitmap` read as a `GBitmap`; its top-left pixel is the
//! separator colour; rows are scanned down column 0, skipping separator pixels; on a row that is not a separator the pixels are
//! scanned left to right, each run of non-separator pixels a rectangle whose height runs down its first column to the next separator;
//! then the scan skips to the next separator of column 0. Colours compare in all four channels (an RGB bitmap's alpha is 255). The
//! array is made once per profile (the first control that asks) and kept, as `mBitmapArrayRects`; an empty result is not kept
//! (it is tried again). The scan is capped ([`MAX_ARRAY_RECTS`], [`MAX_ARRAY_PIXELS`]).
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `gui/core/guiTypes.cpp` (Copyright (c) 2012
//! GarageGames, LLC); the code here is this crate's own.

use std::rc::Rc;

use torque_ts_vm::host::FileRead;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use crate::control::{flag, is_a};
use crate::{Gui, atoi};

/// The most rectangles one bitmap array keeps (a GUI's profiles have under 40).
pub const MAX_ARRAY_RECTS: usize = 1024;
/// The most pixels one array's scan reads (a 2,048 x 2,048 bitmap's worth).
pub const MAX_ARRAY_PIXELS: u64 = 1 << 22;

/// A rectangle `(x, y, w, h)`.
pub type Rect = [i32; 4];

/// The profile object a control's `profile` field names, if it is one.
pub fn profile_of(vm: &Vm, id: ObjId) -> Option<ObjId> {
    let name = vm.objects().field(id, b"profile", b"").unwrap_or_default();
    if name.is_empty() {
        return None;
    }
    vm.objects()
        .find(&name)
        .filter(|p| is_a(vm, *p, "GuiControlProfile"))
}

/// A field's text ("" when absent).
pub fn text(vm: &Vm, id: ObjId, name: &[u8]) -> Vec<u8> {
    vm.objects().field(id, name, b"").unwrap_or_default()
}

/// An S32 field.
pub fn int(vm: &Vm, id: ObjId, name: &[u8]) -> i32 {
    atoi(&text(vm, id, name))
}

/// A ColorI field ([`crate::draw::color_i`]).
pub fn color(vm: &Vm, id: ObjId, name: &[u8]) -> [u8; 4] {
    crate::draw::color_i(&text(vm, id, name))
}

/// Entry `i` of an array field (`fontColors[i]`) as a ColorI.
pub fn color_at(vm: &Vm, id: ObjId, name: &[u8], i: usize) -> [u8; 4] {
    crate::draw::color_i(
        &vm.objects()
            .field(id, name, i.to_string().as_bytes())
            .unwrap_or_default(),
    )
}

/// The names TGE gives `fontColors[0..6]`: `fontColor`, `fontColorHL`, `fontColorNA`, `fontColorSEL`, `fontColorLink` and
/// `fontColorLinkHL` are the same colors as the array's first six entries.
const FONT_COLOR_NAMES: [&[u8]; 6] = [
    b"fontColor",
    b"fontColorHL",
    b"fontColorNA",
    b"fontColorSEL",
    b"fontColorLink",
    b"fontColorLinkHL",
];

/// A profile's font color `i` (`fontColors[i]`), or the color set under its name (`fontColorHL` for 1, ...) when the
/// array entry is not set: TGE stores both in one place. An entry no script set reads as transparent black (the class's
/// fresh value), and transparent text is never meant, so that counts as not set.
pub fn font_color(vm: &Vm, id: ObjId, i: usize) -> [u8; 4] {
    let objects = vm.objects();
    let read = |name: &[u8], index: &[u8]| {
        crate::draw::color_i(&objects.field(id, name, index).unwrap_or_default())
    };
    let entry = read(b"fontColors", i.to_string().as_bytes());
    match FONT_COLOR_NAMES.get(i) {
        Some(name) if entry == [0; 4] => read(name, b""),
        _ => entry,
    }
}

/// A profile's flag.
pub fn pflag(vm: &Vm, profile: Option<ObjId>, name: &[u8]) -> bool {
    profile.is_some_and(|p| flag(vm, p, name))
}

/// A profile's S32 (0 without a profile).
pub fn pint(vm: &Vm, profile: Option<ObjId>, name: &[u8]) -> i32 {
    profile.map_or(0, |p| int(vm, p, name))
}

/// The profile's `justify` (`mAlignment`): 0 left, 1 right, 2 center (the field reads as its enum's name or its number).
pub fn justify(vm: &Vm, profile: Option<ObjId>) -> i32 {
    let Some(p) = profile else {
        return 0;
    };
    let t = text(vm, p, b"justify").to_ascii_lowercase();
    match t.as_slice() {
        b"right" => 1,
        b"center" => 2,
        b"left" | b"" => 0,
        other => atoi(other),
    }
}

/// The profile's `borderThickness` (`mBorderThickness`).
pub fn border_thickness(vm: &Vm, profile: Option<ObjId>) -> i32 {
    pint(vm, profile, b"borderThickness")
}

/// The profile's bitmap array (see the module documentation): made on first use and kept in the GUI's state.
pub fn bitmap_array(vm: &Vm, gui: &Gui, profile: ObjId) -> Rc<Vec<Rect>> {
    if let Some(found) = gui.0.borrow().bitmap_arrays.get(&profile) {
        return Rc::clone(found);
    }
    let rects = Rc::new(construct(vm, profile));
    if !rects.is_empty() {
        gui.0
            .borrow_mut()
            .bitmap_arrays
            .insert(profile, Rc::clone(&rects));
    }
    rects
}

/// The bitmap a profile names, decoded (the texture manager's search: the name, `.jpg`, `.png`, parent folders).
pub fn profile_bitmap(vm: &Vm, profile: ObjId) -> Option<torque_images::Bitmap> {
    let name = String::from_utf8_lossy(&text(vm, profile, b"bitmap")).into_owned();
    if name.is_empty() {
        return None;
    }
    for path in crate::textures::candidates(&name) {
        let Some(kind) = torque_images::Kind::of_name(&path) else {
            continue;
        };
        if let FileRead::Found(bytes) = vm.file_host().read(path.as_bytes()) {
            return torque_images::decode(kind, &bytes).ok();
        }
    }
    None
}

/// `GBitmap::getColor` as RGBA (an RGB pixel's alpha 255, an alpha-only one white with its alpha).
fn pixel(b: &torque_images::Bitmap, x: u32, y: u32) -> [u8; 4] {
    let bpp = b.format.bytes_per_pixel();
    let at = (y as usize * b.width as usize + x as usize) * bpp;
    let px = |k: usize| b.bits.get(at + k).copied().unwrap_or(0);
    match b.format {
        torque_images::Format::Rgba => [px(0), px(1), px(2), px(3)],
        torque_images::Format::Rgb => [px(0), px(1), px(2), 255],
        torque_images::Format::Alpha => [255, 255, 255, px(0)],
    }
}

/// `constructBitmapArray` over the profile's bitmap.
fn construct(vm: &Vm, profile: ObjId) -> Vec<Rect> {
    let Some(b) = profile_bitmap(vm, profile) else {
        return Vec::new();
    };
    construct_from(&b)
}

/// The scan of `construct` over a decoded bitmap.
pub fn construct_from(b: &torque_images::Bitmap) -> Vec<Rect> {
    let (w, h) = (b.width, b.height);
    let mut out = Vec::new();
    if w == 0 || h == 0 {
        return out;
    }
    let sep = pixel(b, 0, 0);
    let reads = std::cell::Cell::new(0u64);
    let get = |x: u32, y: u32| {
        reads.set(reads.get() + 1);
        pixel(b, x, y)
    };
    let mut cur_y = 0;
    while cur_y < h && reads.get() < MAX_ARRAY_PIXELS && out.len() < MAX_ARRAY_RECTS {
        if get(0, cur_y) == sep {
            cur_y += 1;
            continue;
        }
        let mut cur_x = 0;
        while cur_x < w && reads.get() < MAX_ARRAY_PIXELS && out.len() < MAX_ARRAY_RECTS {
            if get(cur_x, cur_y) == sep {
                cur_x += 1;
                continue;
            }
            let start_x = cur_x;
            while cur_x < w && get(cur_x, cur_y) != sep {
                cur_x += 1;
            }
            let mut step_y = cur_y;
            while step_y < h && get(start_x, step_y) != sep {
                step_y += 1;
            }
            out.push([
                start_x as i32,
                cur_y as i32,
                (cur_x - start_x) as i32,
                (step_y - cur_y) as i32,
            ]);
        }
        while cur_y < h && get(0, cur_y) != sep {
            cur_y += 1;
        }
    }
    out
}

#[cfg(test)]
#[path = "profile_tests.rs"]
mod tests;
