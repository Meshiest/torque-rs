//! The console's controls as TGE 1.4 has them: GuiConsole (the log in a scroll control) and GuiConsoleTextCtrl
//! (an expression shown as text). GuiConsoleEditCtrl is a GuiTextEditCtrl ([`crate::input::text_edit`]). Drawing is [`crate::draw`]'s.
//!
//! - **GuiConsole** (`onWake`: the profile's font; `onPreRender`): when the console log's length is not
//!   `extent.y / cell height`, the widest of the new lines (`getStrNWidth`, plus 6) widens the cell (its height the font's), the size
//!   becomes `(1, lines)` and the control `(cell width, cell height x lines)`; when the scroll parent showed the bottom before, the
//!   last line is scrolled into view. The log is the `Vm`'s console history ([`torque_ts_vm::vm::Vm::console_history`]).
//! - **GuiConsoleTextCtrl** (`onWake`: the font; `onPreRender`): the `expression` evaluated as `$temp = <expr>;`
//!   (`Con::evaluatef`; the result is what the statement returns), then with the profile's `autoSizeWidth` the width is the result's
//!   `getStrNWidth` plus 4 (the height `font height + 4` with `autoSizeHeight` too), else with `autoSizeHeight` alone the height.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `gui/controls/guiConsole.cpp` (Copyright (c)
//! 2012 GarageGames, LLC), `gui/controls/guiConsoleTextCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC),
//! `gui/controls/guiConsoleEditCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC); the code here is this crate's own.

use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Vm, VmError};

use crate::Gui;
use crate::control::{bounds, is_a, parent};
use crate::profile::{pflag, profile_of};

/// GuiConsole's `onPreRender`.
pub fn pre_render_console(vm: &mut Vm, gui: &Gui, id: ObjId) {
    let Some(font) = crate::textlist::font(vm, gui, id) else {
        return;
    };
    let count = vm.console_history().map_or(0, |h| h.len());
    let mut s = crate::textlist::state(gui, id);
    let (pos, ext) = bounds(vm, id);
    let prev = if s.cell.1 == 0 { 0 } else { ext.1 / s.cell.1 };
    if count as i32 == prev {
        return;
    }
    let scroll = parent(vm, id).filter(|p| is_a(vm, *p, "GuiScrollCtrl"));
    // "Scrolled to the bottom": the child's bottom at or above the content's.
    let at_bottom = scroll.is_some_and(|p| {
        let st = crate::scroll::state(gui, p);
        st.child_ext.1.wrapping_add(st.child_pos.1)
            <= st.content_ext.1.wrapping_add(st.content_pos.1)
    });
    // The widest of the new lines (from the old count to the last), plus 6; 0 when the range is empty or backwards.
    let from = prev.max(0) as usize;
    let mut widest = 0i32;
    let mut new_width = 0i32;
    if let Some(h) = vm.console_history()
        && prev >= 0
        && from < h.len()
    {
        vm.charge_work((h.len() - from) as u64 * 16);
        for line in h.iter().skip(from) {
            widest = widest.max(font.str_width(&line.text));
        }
        new_width = widest.wrapping_add(6);
    }
    if s.cell.0 < new_width {
        s.cell = (new_width, font.height as i32);
    }
    s.size = (1, count as i32);
    gui.with_mut(id, |c| c.list = Some(s));
    crate::control::resize(
        vm,
        gui,
        id,
        pos,
        (s.cell.0, s.cell.1.wrapping_mul(count as i32)),
    );
    if at_bottom
        && let Some(p) = scroll
        && count > 0
    {
        let row = count as i32 - 1;
        crate::scroll::scroll_rect_visible(
            vm,
            gui,
            p,
            [0, s.cell.1.wrapping_mul(row), s.cell.0, s.cell.1],
        );
    }
}

/// GuiConsoleTextCtrl's `onPreRender`: the result into [`crate::Control::text`], then the automatic size.
pub fn pre_render_text(vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<(), VmError> {
    let expr = vm
        .objects()
        .field(id, b"expression", b"")
        .unwrap_or_default();
    if !expr.is_empty() {
        let mut code = b"$temp = ".to_vec();
        code.extend_from_slice(&expr);
        code.extend_from_slice(b";");
        let result = vm.eval(&code)?;
        gui.with_mut(id, |c| c.text = Some(result));
    }
    let Some(result) = gui.with(id, |c| c.text.clone()).flatten() else {
        return Ok(());
    };
    let p = profile_of(vm, id);
    let (aw, ah) = (
        pflag(vm, p, b"autoSizeWidth"),
        pflag(vm, p, b"autoSizeHeight"),
    );
    if !aw && !ah {
        return Ok(());
    }
    let Some(font) = crate::textlist::font(vm, gui, id) else {
        return Ok(());
    };
    let (pos, ext) = bounds(vm, id);
    let new = if aw {
        (
            font.str_width(&result).wrapping_add(4),
            if ah { font.height as i32 + 4 } else { ext.1 },
        )
    } else {
        (ext.0, font.height as i32 + 4)
    };
    crate::control::resize(vm, gui, id, pos, new);
    Ok(())
}
