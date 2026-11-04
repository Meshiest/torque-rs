use std::any::Any;
use std::rc::Rc;

use torque_fonts::{Fallback, Font, FontCache, Glyph, Remap, Tab, Units};

use super::*;
use crate::ext::{FontSource, GuiExtension};

/// A font whose printable characters are each 6 pixels wide.
struct EvenFonts;

impl Fallback for EvenFonts {
    fn make(&self, _face: &str, _size: i32) -> Option<Font> {
        let glyph = Glyph {
            x_increment: 6,
            ..Glyph::default()
        };
        Some(Font {
            height: 12,
            baseline: 10,
            ascent: 10,
            descent: 2,
            glyphs: vec![glyph; 95],
            sheets: Vec::new(),
            remap: Remap::new(32, (0..95).collect()),
            units: Units::Utf16,
            tab: Tab::Spaces,
        })
    }

    fn name(&self) -> &'static str {
        "even"
    }
}

/// The fonts, and TGE's keys when `tge`.
struct Host {
    tge: bool,
}

impl GuiExtension for Host {
    fn into_any(self: Rc<Self>) -> Rc<dyn Any> {
        self
    }

    fn fonts(&self) -> Option<Box<dyn FontSource>> {
        Some(Box::new(FontCache::new("fonts", Box::new(EvenFonts))))
    }

    fn rule(&self, rule: GuiRule) -> bool {
        self.tge && rule == GuiRule::TgeEditKeys
    }
}

const CTRL: u8 = SI_LCTRL;
const SHIFT: u8 = SI_LSHIFT;
const KEY_C: u16 = KEY_A + 2;
const KEY_V: u16 = KEY_A + 21;
const KEY_Y: u16 = KEY_A + 24;
const KEY_Z: u16 = KEY_A + 25;

struct Rig {
    vm: Vm,
    gui: Gui,
    id: ObjId,
}

/// An edit at (10, 10), 200 wide; its text starts at x 13.
fn rig(tge: bool, fields: &str) -> Rig {
    let mut vm = Vm::new();
    let gui = crate::register(&mut vm);
    gui.set_extension(Rc::new(Host { tge }));
    gui.force_canvas_size(Some((400, 300)));
    assert!(crate::canvas::create_canvas(&mut vm, &gui).unwrap());
    let script = format!(
        "new GuiControlProfile(EditProfile) {{ fontType = \"Arial\"; fontSize = 12; canKeyFocus = 1; tab = 1; }};\
         new GuiControl(Main) {{ extent = \"400 300\"; \
           new GuiTextEditCtrl(Ed) {{ profile = \"EditProfile\"; position = \"10 10\"; extent = \"200 20\"; enabled = 1; {fields} }}; }};\
         Canvas.setContent(Main);"
    );
    vm.eval(script.as_bytes()).unwrap();
    let id = vm.objects().find(b"Ed").unwrap();
    Rig { vm, gui, id }
}

impl Rig {
    fn key(&mut self, code: u16, modifier: u8) -> bool {
        let ev = GuiEvent {
            key_code: code as u8,
            modifier,
            action: SI_MAKE,
            ..GuiEvent::default()
        };
        key_down(&mut self.vm, &self.gui, self.id, ev, 0).unwrap()
    }

    fn type_text(&mut self, s: &str) {
        for b in s.bytes() {
            let ev = GuiEvent {
                ascii: u16::from(b),
                key_code: KEY_SPACE as u8,
                action: SI_MAKE,
                ..GuiEvent::default()
            };
            key_down(&mut self.vm, &self.gui, self.id, ev, 0).unwrap();
        }
    }

    fn click(&mut self, x: i32, clicks: u8, modifier: u8) {
        let ev = GuiEvent {
            point: (x, 15),
            clicks,
            modifier,
            ..GuiEvent::default()
        };
        mouse_down(&mut self.vm, &self.gui, self.id, ev).unwrap();
    }

    fn text(&self) -> String {
        String::from_utf8(text(&self.gui, self.id)).unwrap()
    }

    /// The selection and the cursor.
    fn sel(&self) -> (i32, i32, i32) {
        edit(&self.gui, self.id, |e| {
            (e.block_start, e.block_end, e.cursor)
        })
        .unwrap()
    }
}

#[test]
fn ctrl_a_selects_all_and_typing_replaces_it() {
    let mut r = rig(false, "");
    r.type_text("hello world");
    assert!(r.key(KEY_A, CTRL));
    assert_eq!(r.sel(), (0, 11, 11));
    r.type_text("x");
    assert_eq!(r.text(), "x");
}

#[test]
fn tge_keys_keep_ctrl_a_as_home_and_one_undo_copy() {
    let mut r = rig(true, "");
    r.type_text("hello");
    r.key(KEY_A, CTRL);
    assert_eq!(r.sel(), (0, 0, 0), "Ctrl+A is Home");
    r.key(KEY_END, 0);
    r.type_text("!");
    r.key(KEY_Z, CTRL);
    assert_eq!(
        r.text(),
        "hello",
        "the undo copy is the text before the last change"
    );
    r.key(KEY_Z, CTRL);
    assert_eq!(r.text(), "hello!", "a second Ctrl+Z swaps back");
}

#[test]
fn word_moves_stop_at_spaces_and_punctuation() {
    let t = b"foo.bar  baz,qux";
    let rights: Vec<usize> = [0, 3, 4, 7, 12, 13, 16]
        .iter()
        .map(|&p| word_right(t, p))
        .collect();
    assert_eq!(rights, [3, 4, 7, 12, 13, 16, 16]);
    let lefts: Vec<usize> = [16, 13, 12, 9, 4, 3, 0]
        .iter()
        .map(|&p| word_left(t, p))
        .collect();
    assert_eq!(lefts, [13, 12, 9, 4, 3, 0, 0]);
}

#[test]
fn ctrl_left_and_right_jump_words_and_shift_extends_the_selection() {
    let mut r = rig(false, "");
    r.type_text("foo.bar  baz,qux");
    r.key(KEY_LEFT, CTRL);
    assert_eq!(r.sel(), (0, 0, 13));
    r.key(KEY_LEFT, CTRL);
    assert_eq!(r.sel(), (0, 0, 12));
    r.key(KEY_LEFT, CTRL | SHIFT);
    assert_eq!(r.sel(), (9, 12, 9), "Ctrl+Shift+Left selects baz");
    r.key(KEY_LEFT, CTRL | SHIFT);
    assert_eq!(r.sel(), (4, 12, 4), "and on past the spaces over bar");
    r.key(KEY_RIGHT, CTRL | SHIFT);
    assert_eq!(
        r.sel(),
        (7, 12, 7),
        "Ctrl+Shift+Right shrinks it from the cursor's end"
    );
    r.key(KEY_RIGHT, 0);
    assert_eq!(r.sel(), (0, 0, 12), "Right drops the selection at its end");
    r.key(KEY_HOME, SHIFT);
    assert_eq!(r.sel(), (0, 12, 0));
    r.key(KEY_END, SHIFT);
    assert_eq!(r.sel(), (12, 16, 16), "Shift+End selects from the anchor");
    r.key(KEY_LEFT, 0);
    assert_eq!(r.sel(), (0, 0, 12), "Left drops the selection at its start");
}

#[test]
fn ctrl_backspace_and_ctrl_delete_remove_a_word() {
    let mut r = rig(false, "");
    r.type_text("one two, three");
    r.key(KEY_BACKSPACE, CTRL);
    assert_eq!(r.text(), "one two, ");
    r.key(KEY_BACKSPACE, CTRL);
    assert_eq!(r.text(), "one two");
    r.key(KEY_HOME, 0);
    r.key(KEY_DELETE, CTRL);
    assert_eq!(r.text(), " two");
    r.key(KEY_DELETE, CTRL);
    assert_eq!(r.text(), "");
}

#[test]
fn a_password_is_one_word() {
    let mut r = rig(false, "password = 1;");
    r.type_text("ab cd.ef");
    r.key(KEY_LEFT, CTRL);
    assert_eq!(r.sel(), (0, 0, 0));
    r.key(KEY_DELETE, CTRL);
    assert_eq!(r.text(), "");
}

#[test]
fn undo_takes_typing_as_one_step_and_redo_brings_it_back() {
    let mut r = rig(false, "");
    r.type_text("hello");
    r.key(KEY_END, 0);
    r.type_text(" world");
    r.key(KEY_BACKSPACE, CTRL);
    assert_eq!(r.text(), "hello ");
    let undo = |r: &mut Rig| r.key(KEY_Z, CTRL);
    undo(&mut r);
    assert_eq!(r.text(), "hello world");
    undo(&mut r);
    assert_eq!(r.text(), "hello", "the typing after the move is one step");
    undo(&mut r);
    assert_eq!(r.text(), "");
    undo(&mut r);
    assert_eq!(r.text(), "", "nothing more to undo");
    r.key(KEY_Y, CTRL);
    assert_eq!(r.text(), "hello");
    r.key(KEY_Z, CTRL | SHIFT);
    assert_eq!(r.text(), "hello world");
    r.type_text("!");
    r.key(KEY_Y, CTRL);
    assert_eq!(r.text(), "hello world!", "a change drops the redo steps");
}

#[test]
fn backspaces_in_a_row_are_one_step_and_the_history_is_capped() {
    let mut r = rig(false, "");
    r.type_text("abcdef");
    r.key(KEY_HOME, 0);
    r.key(KEY_END, 0);
    for _ in 0..3 {
        r.key(KEY_BACKSPACE, 0);
    }
    assert_eq!(r.text(), "abc");
    r.key(KEY_Z, CTRL);
    assert_eq!(r.text(), "abcdef");
    assert_eq!(r.sel(), (0, 0, 6));

    let mut r = rig(false, "");
    for _ in 0..150 {
        r.type_text("x");
        r.key(KEY_LEFT, 0);
    }
    let mut undone = 0;
    while !r.text().is_empty() && undone < 200 {
        r.key(KEY_Z, CTRL);
        undone += 1;
    }
    assert_eq!(r.text().len(), 150 - MAX_UNDO_STEPS);
}

#[test]
fn paste_replaces_the_selection_within_max_length() {
    let mut r = rig(false, "maxLength = 8;");
    r.type_text("12345");
    r.key(KEY_HOME, 0);
    r.key(KEY_RIGHT, SHIFT);
    r.key(KEY_RIGHT, SHIFT);
    r.key(KEY_C, CTRL);
    r.key(KEY_END, 0);
    r.key(KEY_V, CTRL);
    assert_eq!(r.text(), "1234512");
    input(&r.gui).clipboard = b"abcdef".to_vec();
    r.key(KEY_LEFT, SHIFT);
    r.key(KEY_LEFT, SHIFT);
    r.key(KEY_INSERT, SHIFT);
    assert_eq!(r.text(), "12345abc", "the selection's room counts");
    r.key(KEY_Z, CTRL);
    assert_eq!(r.text(), "1234512", "a paste is one step");
}

#[test]
fn double_and_triple_clicks_select_a_word_and_all_and_shift_click_extends() {
    let mut r = rig(false, "");
    r.type_text("foo bar.baz");
    // Character i spans x 13 + 6i .. 13 + 6i + 6.
    r.click(13 + 6 * 5 + 1, 1, 0);
    assert_eq!(r.sel(), (0, 0, 5));
    r.click(13 + 6 * 5 + 4, 1, 0);
    assert_eq!(
        r.sel(),
        (0, 0, 6),
        "a click on a character's right half goes after it"
    );
    r.click(13 + 6 * 5 + 1, 2, 0);
    assert_eq!(r.sel(), (4, 7, 7), "a double click selects bar");
    r.click(13 + 6 * 5 + 1, 3, 0);
    assert_eq!(r.sel(), (0, 11, 11), "a triple click selects all");
    r.click(13 + 6 * 2, 1, 0);
    r.click(13 + 6 * 8, 1, SHIFT);
    assert_eq!(r.sel(), (2, 8, 8), "Shift+click selects from the cursor");
    r.click(13, 1, SHIFT);
    assert_eq!(r.sel(), (0, 2, 0), "and again from the same anchor");
}
