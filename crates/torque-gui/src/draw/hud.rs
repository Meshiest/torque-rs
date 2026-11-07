//! The HUD classes' `onRender` (TGE 1.4's; the state they read is [`crate::hud`]): GuiHealthBarHud, GuiCrossHairHud,
//! GuiShapeNameHud (`onRender`, `drawName`), GuiGraphCtrl, GuiMLTextEditCtrl. None of them but the cross hair (whose `onRender`
//! is GuiBitmapCtrl's) draws its children, and none runs the script's `onRender`.
//! GuiBitmapCtrl's) draws its children, and none runs the script's `onRender`.
//!
//! A `ColorF` field becomes the colour drawn as `ColorI(ColorF)` does: each channel `(U8)(c * 255 + 0.5)` truncated toward
//! zero, unclamped, its low byte.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/fps/guiHealthBarHud.cpp` (Copyright (c)
//! 2012 GarageGames, LLC), `T3D/fps/guiCrossHairHud.cpp` (Copyright (c) 2012 GarageGames, LLC), `T3D/fps/guiShapeNameHud.cpp`
//! (Copyright (c) 2012 GarageGames, LLC), `gui/editor/guiGraphCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC),
//! `gui/controls/guiMLTextEditCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC); the code here is this crate's own.

use torque_fonts::TextColors;
use torque_ts_vm::host::FileRead;
use torque_ts_vm::sim::ObjId;

use super::{Walk, color_i, int_field, profile_of, text_field};
use crate::control::{flag, point};
use crate::hud::{GraphType, HudCamera, MAX_NAME_BYTES};

/// `(U8)(c * 255 + 0.5)` truncated toward zero, its low byte.
pub fn color_byte(c: f32) -> u8 {
    (f64::from(c * 255.0 + 0.5) as i64) as u8
}

/// A `ColorF` field (`%g %g %g %g`, the alpha 1 when absent) as it is drawn.
fn color_f(vm: &torque_ts_vm::vm::Vm, id: ObjId, name: &[u8]) -> [u8; 4] {
    let mut f = [0.0f32, 0.0, 0.0, 1.0];
    torque_ts_vm::text::scan_floats(&text_field(vm, id, name), &mut f);
    f.map(color_byte)
}

/// The fraction of the bar `value` gives (`ftol(extent * value)`).
fn ftol(x: f32) -> i32 {
    x as i32
}

/// The near plane of the client's camera query (`GameProcessCameraQuery`).
pub const NEAR_PLANE: f64 = 0.1;

/// `GuiTSCtrl::project` with the frustum and modelview `GuiTSCtrl::onRender` set from the camera: the
/// frustum from the horizontal fov over the control's extent cut to the update rectangle `update` (`control` its whole rectangle),
/// the far plane at the visible distance, then `gluProject` with the saved viewport `(update.x, update.y + update.h, update.w,
/// -update.h)` (the flip to the screen). None when the point is behind the near plane or past the far one (`winz` outside 0..1).
pub fn project(
    cam: &HudCamera,
    far: f32,
    control: [i32; 4],
    update: [i32; 4],
    p: [f32; 3],
) -> Option<(f32, f32)> {
    let fov = f64::from(cam.fov * std::f32::consts::PI / 180.0);
    let n = NEAR_PLANE;
    let f = f64::from(far);
    let wwidth = n * (fov * 0.5).tan();
    let (cw, ch) = (f64::from(control[2].max(1)), f64::from(control[3].max(1)));
    let wheight = ch / cw * wwidth;
    let hscale = wwidth * 2.0 / cw;
    let vscale = wheight * 2.0 / ch;
    let left = f64::from(update[0] - control[0]) * hscale - wwidth;
    let right = f64::from(update[0] + update[2] - control[0]) * hscale - wwidth;
    let top = wheight - vscale * f64::from(update[1] - control[1]);
    let bottom = wheight - vscale * f64::from(update[1] + update[3] - control[1]);
    // The point in the camera's frame (the rotation's transpose): x right, y forward, z up; then GL's (x, z, -y).
    let d = [
        f64::from(p[0] - cam.position[0]),
        f64::from(p[1] - cam.position[1]),
        f64::from(p[2] - cam.position[2]),
    ];
    let r = &cam.rotation;
    let col = |c: usize| {
        f64::from(r[0][c]) * d[0] + f64::from(r[1][c]) * d[1] + f64::from(r[2][c]) * d[2]
    };
    let (ex, ey, ez) = (col(0), col(1), col(2));
    let (gx, gy, gz) = (ex, ez, -ey);
    let xc = 2.0 * n / (right - left) * gx + (right + left) / (right - left) * gz;
    let yc = 2.0 * n / (top - bottom) * gy + (top + bottom) / (top - bottom) * gz;
    let zc = -(f + n) / (f - n) * gz - 2.0 * f * n / (f - n);
    let wc = -gz;
    if wc == 0.0 {
        return None;
    }
    let (nx, ny, nz) = (xc / wc, yc / wc, zc / wc);
    let winz = nz * 0.5 + 0.5;
    if !(0.0..=1.0).contains(&winz) {
        return None;
    }
    let vp = [
        f64::from(update[0]),
        f64::from(update[1] + update[3]),
        f64::from(update[2]),
        -f64::from(update[3]),
    ];
    Some((
        (vp[0] + vp[2] * (nx * 0.5 + 0.5)) as f32,
        (vp[1] + vp[3] * (ny * 0.5 + 0.5)) as f32,
    ))
}

impl Walk<'_> {
    /// GuiHealthBarHud::onRender: with a control object, the energy (`displayEnergy`) or one less the damage. The fill over the
    /// update rectangle, the bar (`extent * value` across, from the right when `flipped`; down, from the bottom unless `flipped`,
    /// when the control is taller than wide), the frame. The pulse writes the damage colour's alpha (`a = (t % rate) / (rate /
    /// 2)`, `2 - a` past 1, 1 at or above the threshold), from the virtual clock, for the frame (TGE stores it in the field,
    /// which only this draw reads back).
    pub(super) fn health_bar(&mut self, id: ObjId, update: [i32; 4]) {
        let vm = self.vm;
        let Some(control) = self.gui.0.borrow().hud.control else {
            return;
        };
        let value = if flag(vm, id, b"displayEnergy") {
            control.energy
        } else {
            1.0 - control.damage
        };
        if flag(vm, id, b"showFill") {
            self.fill(update, color_f(vm, id, b"fillColor"));
        }
        let mut damage = [0.0f32, 0.0, 0.0, 1.0];
        torque_ts_vm::text::scan_floats(&text_field(vm, id, b"damageFillColor"), &mut damage);
        let rate = int_field(vm, id, b"pulseRate");
        if rate != 0 {
            let mut threshold = [0.0f32];
            torque_ts_vm::text::scan_floats(&text_field(vm, id, b"pulseThreshold"), &mut threshold);
            damage[3] = if value < threshold[0] {
                let t = self.now % (rate as u32);
                let mut a = t as f32 / (f64::from(rate as f32) * 0.5) as f32;
                if a > 1.0 {
                    a = (2.0 - f64::from(a)) as f32;
                }
                a
            } else {
                1.0
            };
        }
        let ext = point(vm, id, b"extent");
        let mut r = update;
        if ext.0 > ext.1 {
            r[2] = ftol(update[2] as f32 * value);
            if flag(vm, id, b"flipped") {
                r[0] = update[0].wrapping_sub(r[2]).wrapping_add(update[2]);
            }
        } else {
            r[3] = ftol(update[3] as f32 * value);
            if !flag(vm, id, b"flipped") {
                r[1] = update[1].wrapping_sub(r[3]).wrapping_add(update[3]);
            }
        }
        self.fill(r, damage.map(color_byte));
        if flag(vm, id, b"showFrame") {
            self.outline(
                (update[0], update[1]),
                (update[0] + update[2] - 1, update[1] + update[3] - 1),
                color_f(vm, id, b"frameColor"),
            );
        }
    }

    /// GuiCrossHairHud::onRender: the bitmap (GuiBitmapCtrl's `onRender`) only while the control object is a Player or a
    /// Vehicle and the camera is the first-person one. TGE's also draws the target's damage bar from a ray cast into the world;
    /// the host's feed has no ray, so none is drawn (the four damage fields are not read).
    pub(super) fn cross_hair(
        &mut self,
        id: ObjId,
        offset: (i32, i32),
        update: [i32; 4],
        depth: usize,
    ) {
        let Some(control) = self.gui.0.borrow().hud.control else {
            return;
        };
        if !(control.player || control.vehicle) || !control.first_person {
            return;
        }
        self.bitmap(id, offset, update, depth);
    }

    /// GuiShapeNameHud::onRender and `drawName`: see the module docs of [`crate::hud`]. The fill first (even
    /// when nothing else is drawn), the names, the frame.
    pub(super) fn shape_names(&mut self, id: ObjId, update: [i32; 4]) {
        let vm = self.vm;
        if flag(vm, id, b"showFill") {
            self.fill(update, color_f(vm, id, b"fillColor"));
        }
        let Some((control_rect, ts_update)) = self.ts_parent(id) else {
            return;
        };
        let (camera, names, visible, fog) = {
            let s = self.gui.0.borrow();
            if s.hud.control.is_none() {
                return;
            }
            let Some(cam) = s.hud.camera else {
                return;
            };
            (
                cam,
                s.hud.names.clone(),
                s.hud.visible_distance,
                s.hud.fog_distance,
            )
        };
        let mut v_off = [0.0f32];
        torque_ts_vm::text::scan_floats(&text_field(vm, id, b"verticalOffset"), &mut v_off);
        let distance_fade = {
            let text = text_field(vm, id, b"distanceFade");
            let mut f = [0.0f32];
            (!text.trim_ascii().is_empty()).then(|| {
                torque_ts_vm::text::scan_floats(&text, &mut f);
                f[0]
            })
        };
        let half = (camera.fov * std::f32::consts::PI / 180.0 * 0.5) as f64;
        let fov_min = (half + 0.043_633_230_030_536_65) as f32;
        let dir = [
            camera.rotation[0][1],
            camera.rotation[1][1],
            camera.rotation[2][1],
        ];
        for s in &names {
            let d = [
                s.position[0] - camera.position[0],
                s.position[1] - camera.position[1],
                s.position[2] - camera.position[2],
            ];
            let dot = dir[0] * d[0] + dir[1] * d[1] + dir[2] * d[2];
            let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            let Some(opacity) = name_opacity(dist, s.distance, visible, fog, distance_fade) else {
                continue;
            };
            if fov_min > dot || s.hidden {
                continue;
            }
            let mut p = s.position;
            p[2] += v_off[0];
            let Some(proj) = project(&camera, visible, control_rect, ts_update, p) else {
                continue;
            };
            let at = (proj.0 as i32, proj.1 as i32);
            self.draw_name(
                id,
                &s.name,
                at,
                [s.color[0], s.color[1], s.color[2], opacity],
            );
        }
        if flag(vm, id, b"showFrame") {
            self.outline(
                (update[0], update[1]),
                (update[0] + update[2] - 1, update[1] + update[3] - 1),
                color_f(vm, id, b"frameColor"),
            );
        }
    }

    /// `drawName`: the name centred on `at` (`x - (width >> 1)`, `y - font height`), drawn eight times one pixel around
    /// in black (white when the colour's red and green are both under 0.3) at the colour's alpha, then once in the colour.
    fn draw_name(&mut self, id: ObjId, name: &[u8], at: (i32, i32), color: [f32; 4]) {
        let vm = self.vm;
        let Some(profile) = profile_of(vm, id) else {
            return;
        };
        let name = &name[..name.len().min(MAX_NAME_BYTES)];
        let face = String::from_utf8_lossy(&text_field(vm, profile, b"fontType")).into_owned();
        let size = int_field(vm, profile, b"fontSize");
        let read = |p: &str| match vm.file_host().read(p.as_bytes()) {
            FileRead::Found(b) => Some(b),
            _ => None,
        };
        let Some(font) = self.cache.fonts.font(&face, size, &read) else {
            self.out.draws.report.no_font += 1;
            return;
        };
        let w = font.str_width(name) as u32;
        let x = at.0.wrapping_sub((w >> 1) as i32);
        let y = at.1.wrapping_sub(font.height as i32);
        let clamp = |c: f32| c.clamp(0.0, 1.0);
        let light = f64::from(color[0]) >= 0.3 || f64::from(color[1]) >= 0.3;
        let outline = if light {
            [0.0, 0.0, 0.0, color[3]]
        } else {
            [1.0, 1.0, 1.0, color[3]]
        }
        .map(|c| color_byte(clamp(c)));
        for (dx, dy) in [
            (-1, -1),
            (0, -1),
            (1, -1),
            (-1, 0),
            (1, 0),
            (-1, 1),
            (0, 1),
            (1, 1),
        ] {
            let mut colors = TextColors::new(outline, self.cache.saved_color);
            self.glyphs(
                &font,
                &face,
                size,
                (x + dx, y + dy),
                name,
                &mut colors,
                None,
            );
        }
        let mut colors =
            TextColors::new(color.map(|c| color_byte(clamp(c))), self.cache.saved_color);
        self.glyphs(&font, &face, size, (x, y), name, &mut colors, None);
    }

    /// The GameTSCtrl a control's parent is, with the rectangles its last `onRender` saved (its whole one and the update one).
    fn ts_parent(&self, id: ObjId) -> Option<([i32; 4], [i32; 4])> {
        let parent = self.vm.objects().get(id)?.group()?;
        self.ts_rects
            .iter()
            .rev()
            .find(|(p, _, _)| *p == parent)
            .map(|(_, c, u)| (*c, *u))
    }

    /// GuiGraphCtrl::onRender: the profile's border, a half-white fill of the update rectangle, then each plot in its
    /// colour, unblended: a point, a line strip, quads under the line (filled) or bars, from the samples newest first across the
    /// control. The vertices are placed from the control's `position` (not its screen offset), y from the bottom
    /// (`extent.y - ftol(extent.y / (max * 1.05) * v)`). A line strip is drawn one pixel per step of its longer axis and
    /// a filled quad column by column.
    pub(super) fn graph(&mut self, id: ObjId, offset: (i32, i32), update: [i32; 4]) {
        let vm = self.vm;
        let ext = point(vm, id, b"extent");
        if let Some(p) = profile_of(vm, id)
            && int_field(vm, p, b"border") != 0
        {
            self.outline(
                offset,
                (
                    offset.0.wrapping_add(ext.0).wrapping_sub(1),
                    offset.1.wrapping_add(ext.1).wrapping_sub(1),
                ),
                color_i(&text_field(vm, p, b"borderColor")),
            );
        }
        self.fill(update, [255, 255, 255, 128]);
        let Some(state) = self.gui.with(id, |c| c.graph.clone()).flatten() else {
            return;
        };
        let pos = point(vm, id, b"position");
        let (w, h) = ext;
        if w <= 0 {
            return;
        }
        // In i64, cut to i32 at the end: a script's position or extent cannot overflow the arithmetic.
        let (px, py, h64) = (i64::from(pos.0), i64::from(pos.1), i64::from(h));
        let r32 = |v: i64| v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        for plot in &state.plots {
            if self.out.draws.full() {
                self.out.draws.report.truncated = true;
                return;
            }
            let n = plot.data.len() as i32;
            if n == 0 {
                continue;
            }
            let color = plot.color.map(color_byte);
            let color = [color[0], color[1], color[2], 255];
            let scale = h as f32 / (f64::from(plot.max) * 1.05) as f32;
            let y_of = |v: f32| h64 - i64::from(ftol(scale * v)) + py;
            let bottom = h64 + py;
            let x_of = |s: i32| -> i64 {
                if n >= w {
                    i64::from(s)
                } else {
                    i64::from(ftol(w as f32 / n as f32 * s as f32))
                }
            };
            let sample = |s: i32| plot.data.get(s.max(0) as usize).copied().unwrap_or(0.0);
            let cols = w.min(super::MAX_GRAPH_COLUMNS);
            match plot.kind {
                GraphType::Point => {
                    for s in 0..cols {
                        self.fill_unblended([r32(px + x_of(s)), r32(y_of(sample(s))), 1, 1], color);
                    }
                }
                GraphType::Polyline => {
                    let mut last: Option<(i64, i64)> = None;
                    for s in 0..cols {
                        let p = (px + x_of(s), y_of(sample(s)));
                        if let Some(a) = last {
                            self.strip_segment(a, p, color);
                        }
                        last = Some(p);
                    }
                }
                GraphType::Bar => {
                    let mut t1 = 0i64;
                    for s in 0..cols {
                        let t2 = x_of(s);
                        let y = y_of(sample(s));
                        self.fill_unblended(
                            [r32(px + t1), r32(y), r32(t2 - t1), r32(bottom - y)],
                            color,
                        );
                        t1 = t2;
                    }
                }
                GraphType::Filled => {
                    let mut prev = 0i64;
                    for s in 0..cols.saturating_sub(1) {
                        let cur = x_of(s);
                        let (ya, yb) = (y_of(sample(s)), y_of(sample(s + 1)));
                        // At most the columns of the control a segment (a script's samples cannot widen it).
                        for x in prev..cur.min(prev + i64::from(super::MAX_GRAPH_COLUMNS)) {
                            if self.out.draws.full() {
                                self.out.draws.report.truncated = true;
                                return;
                            }
                            let t = (x - prev) as f32 / (cur - prev) as f32;
                            let y = ya + ((yb - ya) as f32 * t) as i64;
                            self.fill_unblended([r32(px + x), r32(y), 1, r32(bottom - y)], color);
                        }
                        prev = cur;
                    }
                }
            }
        }
    }

    /// One segment of a line strip: a pixel per step of the longer axis (at most [`super::MAX_GRAPH_COLUMNS`]), the last left for
    /// the next segment.
    fn strip_segment(&mut self, a: (i64, i64), b: (i64, i64), color: [u8; 4]) {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let steps = dx
            .abs()
            .max(dy.abs())
            .min(i64::from(super::MAX_GRAPH_COLUMNS));
        let r32 = |v: i64| v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        for i in 0..steps {
            if self.out.draws.full() {
                self.out.draws.report.truncated = true;
                return;
            }
            let x = a.0 + dx * i / steps;
            let y = a.1 + dy * i / steps;
            self.fill_unblended([r32(x), r32(y), 1, 1], color);
        }
    }

    /// GuiMLTextEditCtrl::onRender: GuiMLTextCtrl's, then while the control is the canvas's first responder a line
    /// (`dglDrawLine`) from the cursor's top to its bottom in the profile's `cursorColor` (no blink).
    pub(super) fn ml_text_edit(
        &mut self,
        id: ObjId,
        offset: (i32, i32),
        update: [i32; 4],
        depth: usize,
    ) {
        self.ml_text(id, offset, update, depth);
        if crate::input::canvas_first_responder(self.gui) != Some(id) {
            return;
        }
        let Some(p) = profile_of(self.vm, id) else {
            return;
        };
        let (x, top, bottom) = crate::mltext::cursor_rect(self.vm, self.gui, id);
        let color = color_i(&text_field(self.vm, p, b"cursorColor"));
        self.line(
            (offset.0 + x, offset.1 + top),
            (offset.0 + x, offset.1 + bottom),
            color,
        );
    }

    /// An untextured quad drawn with blending off (`glDisable(GL_BLEND)`): the colour's alpha forced opaque, which draws the same.
    fn fill_unblended(&mut self, r: [i32; 4], color: [u8; 4]) {
        self.fill(r, [color[0], color[1], color[2], 255]);
    }
}

/// A name's opacity at `dist` from the camera, `None` when it is not drawn (past its range). `name_distance` is the shape's
/// name distance, `visible` and `fog` the scene's distances.
///
/// With `distance_fade` (GuiShapeNameHud's `distanceFade`, as Torque3D's `guiShapeNameHud.cpp` reads it): none at the camera
/// (0) or past the visible distance (or the name distance, when nearer); full up to `visible * distanceFade` (a negative one
/// read as 0), then down in a line to 0 at the visible distance. A fade start at or past the visible distance draws the name
/// whole until it is out of range, where it is 0 (Torque3D's division by zero there). Without it: full up to the larger of
/// 80% of the name distance and 5 less than it (the fog distance when that is nearer), then down to 0 at the range.
pub fn name_opacity(
    dist: f32,
    name_distance: f32,
    visible: f32,
    fog: f32,
    distance_fade: Option<f32>,
) -> Option<f32> {
    let max_dist = if visible > name_distance {
        name_distance
    } else {
        visible
    };
    if dist > max_dist {
        return None;
    }
    if let Some(fade) = distance_fade {
        if dist == 0.0 {
            return None;
        }
        let start = visible * fade.max(0.0);
        if dist < start {
            return Some(1.0);
        }
        let span = visible - start;
        return Some(if span > 0.0 {
            1.0 - (dist - start) / span
        } else {
            0.0
        });
    }
    let a = (f64::from(name_distance) * 0.8) as f32;
    let b = (f64::from(name_distance) - 5.0) as f32;
    let mut fade_start = if a > b { a } else { b };
    if fog <= fade_start {
        fade_start = fog;
    }
    Some(if fade_start > dist {
        1.0
    } else {
        1.0 - (dist - fade_start) / (max_dist - fade_start)
    })
}

#[cfg(test)]
#[path = "hud_tests.rs"]
mod tests;
