//! The window's hold on the system cursor while the game has it (in game, mouse look), as TGE's Windows platform layer holds it.
//!
//! TGE's `winWindow.cpp`: `lockMouse` and a GameTSCtrl's click set `windowLocked` and call `setMouseClipping`: `ClipCursor(NULL)`;
//! when the window is active the cursor is hidden, and when it is locked the cursor's place in the window is kept, the cursor
//! clipped to the window's rect and put at its centre; when unlocked it goes back where it was kept. `WM_ACTIVATE` (in the window
//! procedure) sets the active flag and on deactivation calls `setMouseClipping` (which then frees the cursor and shows it). Every
//! main-loop pass `CheckCursorPos`, while locked and active, posts the cursor's offset from the centre as the mouse's x and y axes
//! and puts it back at the centre, so the cursor never leaves the window and is held again as soon as the window is active again.
//!
//! The engine reads the mouse look from raw input (winit's `DeviceEvent::MouseMotion`), so only the hold is TGE's here: the cursor is
//! kept at the window's centre ([`Action::Centre`] every pass while held and active; winit's `Locked` grab clips it to that point,
//! or `Confined` where a platform has no lock), freed when the window loses focus, held again when it gets focus back (Windows drops
//! a clip on focus loss, and other programs may change it, so the grab is renewed on focus and every [`RENEW_MS`]), and put back where
//! it was when the game lets go. A window that may not grab (`may_grab` false, a free-cursor option) never clips or moves it.
//!
//! The hold lets go at once when the window loses focus or is hidden ([`CursorLock::on_focus`] returns the release for the
//! caller to apply in the same event, the cursor shown), and never takes the cursor while the window is unfocused (the renewal is
//! a held pass's, and a pass holds only when focused). A Windows key pressed while held ([`CursorLock::on_system_key`]) lets go
//! too, before the shell's shortcut opens its overlay (Win+Shift+S's snipping overlay may not take the focus): the hold is taken
//! again only when the focus comes back or on a click in the window ([`CursorLock::on_click`]), as TGE takes it again on
//! `WM_ACTIVATE` or a GameTSCtrl click.
//!
//! Credit: follows TGE's Windows window layer as Torque3D carries it (MIT: `platformWin32/winWindow.cpp`, Copyright (c) 2012
//! GarageGames, LLC).

/// How often the clip is renewed while held and focused (another program may have changed it).
pub const RENEW_MS: u64 = 500;

/// What the window does to the system cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Keep the cursor's place (`(x, y)` in the client area), put it at the centre and clip it there.
    Hold { keep: (i32, i32) },
    /// Put the cursor at the centre again (`CheckCursorPos`), and renew the clip when `renew`.
    Centre { renew: bool },
    /// Free the cursor (no clip); `restore`: back to the kept place (`setMouseClipping` unlocked); `show`: the system cursor shown
    /// (the window lost focus or a Windows key let go; while the game is in front the canvas draws its own).
    Free {
        restore: Option<(i32, i32)>,
        show: bool,
    },
}

/// The hold's state.
#[derive(Debug, Clone, Default)]
pub struct CursorLock {
    /// Whether the clip is applied now.
    held: bool,
    focused: bool,
    /// Where the cursor was in the client area when the game took the mouse.
    kept: Option<(i32, i32)>,
    /// When the clip was last renewed (ms of the caller's clock).
    renewed_ms: u64,
    /// The focus came back since the last pass (the clip must be applied again).
    refocused: bool,
    /// A Windows key let go of the cursor: not held again until the focus comes back or a click.
    suspended: bool,
}

impl CursorLock {
    /// A window that has focus (winit sends `Focused(true)` when it gets it; a new window usually has it).
    pub fn new() -> CursorLock {
        CursorLock {
            focused: true,
            ..CursorLock::default()
        }
    }

    pub fn held(&self) -> bool {
        self.held
    }

    /// The window's focus changed (`WM_ACTIVATE`; a hidden or minimised window counts as unfocused). Losing it while held frees
    /// the cursor at once and shows it: the action to apply now, not at the next pass.
    pub fn on_focus(&mut self, focused: bool) -> Option<Action> {
        if focused && !self.focused {
            self.refocused = true;
            self.suspended = false;
        }
        self.focused = focused;
        if focused { None } else { self.release() }
    }

    /// A Windows key went down (the shell's shortcuts open overlays that may not take the focus): the hold lets go now, until the
    /// focus comes back or a click.
    pub fn on_system_key(&mut self) -> Option<Action> {
        self.suspended = true;
        self.release()
    }

    /// A mouse button went down in the window: a hold a Windows key let go of may be taken again.
    pub fn on_click(&mut self) {
        if self.suspended && self.focused {
            self.suspended = false;
            self.refocused = true;
        }
    }

    fn release(&mut self) -> Option<Action> {
        if !self.held {
            return None;
        }
        self.held = false;
        Some(Action::Free {
            restore: None,
            show: true,
        })
    }

    /// The window moved or changed size (a screen mode): the clip at the old centre is renewed at the next pass.
    pub fn on_resize(&mut self) {
        self.refocused = true;
    }

    /// One pass of the window's loop: `wants` (the game holds the mouse: the canvas's cursor is off or `lockMouse(1)`), `may_grab`
    /// (no free-cursor option), the cursor's last place in the client area and the clock. What to do to the system cursor, if anything.
    pub fn pass(
        &mut self,
        wants: bool,
        may_grab: bool,
        cursor: (i32, i32),
        now_ms: u64,
    ) -> Option<Action> {
        let hold = wants && may_grab && self.focused && !self.suspended;
        if !wants {
            // The game let go: the cursor goes back where it was (only if this hold moved it).
            let kept = self.kept.take();
            if self.held {
                self.held = false;
                return Some(Action::Free {
                    restore: kept,
                    show: false,
                });
            }
            return None;
        }
        if !hold {
            if self.held {
                // Focus lost (or a free-cursor option, or a Windows key): freed, the place kept for when the game lets go.
                self.held = false;
                return Some(Action::Free {
                    restore: None,
                    show: !self.focused || self.suspended,
                });
            }
            return None;
        }
        if !self.held {
            self.held = true;
            self.refocused = false;
            self.renewed_ms = now_ms;
            let keep = *self.kept.get_or_insert(cursor);
            return Some(Action::Hold { keep });
        }
        let renew = std::mem::take(&mut self.refocused)
            || now_ms.saturating_sub(self.renewed_ms) >= RENEW_MS;
        if renew {
            self.renewed_ms = now_ms;
        }
        Some(Action::Centre { renew })
    }
}

#[cfg(test)]
#[path = "cursor_lock_tests.rs"]
mod tests;
