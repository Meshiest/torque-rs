use std::any::Any;
use std::cell::Cell;
use std::time::Instant;

use torque_fonts::{Bitmap, Fallback, Font, FontCache, Remap, Tab, Units};
use torque_ts_vm::host::{FileHost, FileStat, MemFiles};

use super::*;
use crate::ext::{FontSource, GuiExtension};

/// A PNG of `w` x `h` whose body is not an image (only the header is read to size it), `pad` bytes long past the header.
fn png_header(w: u32, h: u32, pad: usize) -> Vec<u8> {
    let mut file = vec![137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13];
    file.extend_from_slice(b"IHDR");
    file.extend_from_slice(&w.to_be_bytes());
    file.extend_from_slice(&h.to_be_bytes());
    file.extend_from_slice(&[8, 2, 0, 0, 0, 0, 0, 0, 0]);
    file.extend(std::iter::repeat_n(0xab, pad));
    file
}

#[test]
fn a_bitmaps_size_comes_from_its_header_once_per_file_and_the_cache_keeps_taking_names() {
    // The size decoded the whole image, and past 4,096 names every reflow decoded each new one again.
    let file = png_header(640, 480, 64);
    let read = |p: &str, max: usize| {
        (p.eq_ignore_ascii_case("ui/pic.png")).then(|| file[..file.len().min(max)].to_vec())
    };
    let paid = |_: u64| true;
    let mut sizes = BitmapSizes::default();
    for name in ["ui/pic", "UI/Pic", "ui/PIC.png", "Ui/pIc"] {
        assert_eq!(sizes.size(name, &read, &paid), Some((640, 480)), "{name}");
    }
    assert_eq!(sizes.reads, 1, "one header read for every spelling");
    // Past the cap of names the cache keeps answering (the least recently used names go).
    for i in 0..(crate::textures::MAX_TEXTURES + 100) {
        assert_eq!(sizes.size(&format!("missing/{i}"), &read, &paid), None);
    }
    assert_eq!(sizes.size("UI/PIC", &read, &paid), Some((640, 480)));
    assert_eq!(sizes.reads, 1);
}

#[test]
fn a_bitmap_is_sized_from_its_first_bytes_and_every_lookup_is_charged() {
    let file = png_header(640, 480, 4 << 20);
    let read =
        |p: &str, max: usize| (p == "ui/big.png").then(|| file[..file.len().min(max)].to_vec());
    let charged = Cell::new(0u64);
    let charge = |units: u64| {
        charged.set(charged.get() + units);
        true
    };
    let mut sizes = BitmapSizes::default();
    assert_eq!(sizes.size("ui/big", &read, &charge), Some((640, 480)));
    assert_eq!(sizes.bytes_read, BITMAP_HEADER_BYTES as u64);
    // Two candidates tried (`ui/big.jpg`, then `ui/big.png`), and the bytes read.
    assert_eq!(
        charged.get(),
        2 * BITMAP_LOOKUP_WORK + BITMAP_HEADER_BYTES as u64
    );
    // A budget that refuses stops the lookups, and the name is not remembered as missing.
    let refuse = |_: u64| false;
    assert_eq!(sizes.size("ui/other", &read, &refuse), None);
    assert_eq!(sizes.size("ui/big2", &read, &charge), None);
}

/// A font of `size` rows and one sheet of `8 size` x `8 size` pixels, made for any face and size.
struct SizedFonts;

impl Fallback for SizedFonts {
    fn make(&self, _face: &str, size: i32) -> Option<Font> {
        let side = size.clamp(1, 128) as u32;
        Some(Font {
            height: side,
            baseline: side,
            ascent: side,
            descent: 0,
            glyphs: Vec::new(),
            sheets: vec![Bitmap {
                width: side * 8,
                height: side * 8,
                format: torque_images::Format::Alpha,
                bits: vec![0; (side * side * 64) as usize],
            }],
            remap: Remap::default(),
            units: Units::Utf16,
            tab: Tab::Spaces,
        })
    }

    fn name(&self) -> &'static str {
        "sized"
    }
}

struct FontsOnly;

impl GuiExtension for FontsOnly {
    fn into_any(self: Rc<Self>) -> Rc<dyn Any> {
        self
    }

    fn fonts(&self) -> Option<Box<dyn FontSource>> {
        Some(Box::new(FontCache::new("fonts", Box::new(SizedFonts))))
    }
}

/// A GUI with two ML controls on two profiles, `Ml` and `Other`.
fn two_ml_controls() -> (Vm, Gui, ObjId) {
    let mut vm = Vm::new();
    let gui = crate::register(&mut vm);
    gui.set_extension(Rc::new(FontsOnly));
    gui.force_canvas_size(Some((400, 300)));
    assert!(crate::canvas::create_canvas(&mut vm, &gui).unwrap());
    vm.eval(
        b"new GuiControlProfile(MlProfile) { fontType = \"Arial\"; fontSize = 14; };\
          new GuiControlProfile(OtherProfile) { fontType = \"Game\"; fontSize = 20; };\
          new GuiControl(Main) { extent = \"400 300\"; \
            new GuiMLTextCtrl(Ml) { profile = \"MlProfile\"; extent = \"400 100\"; }; \
            new GuiMLTextCtrl(Other) { profile = \"OtherProfile\"; position = \"0 100\"; extent = \"400 100\"; }; };\
          Canvas.setContent(Main);",
    )
    .unwrap();
    let ml = vm.objects().find(b"Ml").expect("Ml");
    (vm, gui, ml)
}

fn fonts_of(gui: &Gui) -> (u32, u64, (u64, usize)) {
    let fonts = gui.0.borrow().fonts.0.clone();
    let fonts = fonts.borrow();
    let cache = fonts.cache();
    (cache.fallbacks, cache.over_budget, cache.markup_usage())
}

#[test]
fn a_reflow_of_a_thousand_font_tags_stays_within_the_markup_quota_and_spares_the_profiles() {
    let (mut vm, gui, ml) = two_ml_controls();
    let text: String = (0..1024)
        .map(|i| format!("<font:f{}:{}>x", i % 64, 1 + i % 128))
        .collect();
    set_text(&mut vm, &gui, ml, text.as_bytes());
    reflow(&mut vm, &gui, ml).unwrap();
    let (made, _, (px, keys)) = fonts_of(&gui);
    // At most 8 faces a reflow, and the markup quota's keys and pixels: the profile's font and a few hundred more.
    assert!(made <= 1 + torque_fonts::MAX_MARKUP_FONTS as u32, "{made}");
    assert!(keys <= torque_fonts::MAX_MARKUP_FONTS);
    assert!(px <= torque_fonts::MAX_MARKUP_SHEET_PIXELS);
    let styles = state(&gui, ml).expect("ml").layout.styles.clone();
    let faces: HashSet<String> = styles.iter().map(|s| s.font.0.clone()).collect();
    assert!(faces.len() <= 1 + MAX_REFLOW_FACES, "{faces:?}");
    // Another control's profile font still loads.
    let other = vm.objects().find(b"Other").expect("Other");
    set_text(&mut vm, &gui, other, b"hello");
    reflow(&mut vm, &gui, other).unwrap();
    assert!(!state(&gui, other).expect("other").layout.lines.is_empty());
    assert!(gui.0.borrow().fonts.0.borrow().cache().contains("Game", 20));
}

#[test]
fn a_reflow_that_cannot_pay_for_its_fonts_stops() {
    let (mut vm, gui, ml) = two_ml_controls();
    let text: String = (0..1024)
        .map(|i| format!("<font:f{}:{}>x", i % 8, 64 + i % 64))
        .collect();
    set_text(&mut vm, &gui, ml, text.as_bytes());
    let saved = vm.begin_host_budget(2_000_000);
    reflow(&mut vm, &gui, ml).unwrap();
    let refused = !vm.charge_work(0);
    vm.end_host_budget(saved);
    let (made, _, _) = fonts_of(&gui);
    // Each font costs at least `FONT_LOAD_WORK` and its estimate: about 2,000,000 / 300,000 fonts are paid for.
    assert!(made <= 12, "{made}");
    assert!(refused, "the budget is spent");
}

/// A file host that counts what it is asked: every path looked up, and the bytes it hands out.
struct Counting {
    inner: MemFiles,
    lookups: Rc<Cell<u64>>,
    bytes: Rc<Cell<u64>>,
}

impl FileHost for Counting {
    fn read(&self, path: &[u8]) -> FileRead {
        self.lookups.set(self.lookups.get() + 1);
        let read = self.inner.read(path);
        if let FileRead::Found(b) = &read {
            self.bytes.set(self.bytes.get() + b.len() as u64);
        }
        read
    }

    fn read_prefix(&self, path: &[u8], max: usize) -> FileRead {
        self.lookups.set(self.lookups.get() + 1);
        let read = self.inner.read_prefix(path, max);
        if let FileRead::Found(b) = &read {
            self.bytes.set(self.bytes.get() + b.len() as u64);
        }
        read
    }

    fn stat(&self, path: &[u8]) -> FileStat {
        self.inner.stat(path)
    }
}

#[test]
fn a_reflow_of_five_thousand_bitmap_names_resolves_at_most_its_cap_and_reads_only_headers() {
    let (mut vm, gui, ml) = two_ml_controls();
    let files = MemFiles::new();
    let image = png_header(16, 16, 1 << 20);
    for i in 0..5000 {
        files.insert(&format!("ui/b{i}.png"), &image);
    }
    let (lookups, bytes) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
    vm.set_file_host(Counting {
        inner: files,
        lookups: Rc::clone(&lookups),
        bytes: Rc::clone(&bytes),
    });
    let text: String = (0..5000).map(|i| format!("<bitmap:ui/b{i}>")).collect();
    set_text(&mut vm, &gui, ml, text.as_bytes());
    let (l0, b0) = (lookups.get(), bytes.get());
    reflow(&mut vm, &gui, ml).unwrap();
    let placed = state(&gui, ml).expect("ml").layout.bitmaps.len();
    assert_eq!(placed, MAX_REFLOW_BITMAP_NAMES);
    // Each name is found at its second candidate (`.jpg`, then `.png`), and the profile font's cache file is looked for once;
    // each read is the header's prefix.
    assert_eq!(lookups.get() - l0, 2 * MAX_REFLOW_BITMAP_NAMES as u64 + 1);
    assert!(bytes.get() - b0 <= (MAX_REFLOW_BITMAP_NAMES * BITMAP_HEADER_BYTES) as u64);
}

/// The time one reflow of `n` distinct `<tag:>` ids takes.
fn tag_reflow(n: u32) -> std::time::Duration {
    let (mut vm, gui, ml) = two_ml_controls();
    let text: String = (0..n).map(|i| format!("<tag:{i}>")).collect();
    set_text(&mut vm, &gui, ml, text.as_bytes());
    let start = Instant::now();
    reflow(&mut vm, &gui, ml).unwrap();
    let took = start.elapsed();
    let tags = state(&gui, ml).expect("ml").layout.tags.clone();
    assert_eq!(tags.len(), n as usize);
    // In list order: the last placed first.
    assert_eq!(tags.first().map(|t| t.0), Some(n - 1));
    assert_eq!(tags.last().map(|t| t.0), Some(0));
    took
}

#[test]
fn a_reflow_of_sixty_five_thousand_tags_is_linear() {
    // Each tag was inserted at the front of the list and checked against every other: about 2 billion moves at the cap.
    let small = tag_reflow(8_192);
    let big = tag_reflow(65_536);
    assert!(
        big < small * 32 + std::time::Duration::from_millis(200),
        "{small:?} for 8,192, {big:?} for 65,536"
    );
}

#[test]
fn a_duplicate_tag_id_is_still_counted_once() {
    let (mut vm, gui, ml) = two_ml_controls();
    set_text(&mut vm, &gui, ml, b"<tag:7>a<tag:9>b<tag:7>c");
    reflow(&mut vm, &gui, ml).unwrap();
    let tags = state(&gui, ml).expect("ml").layout.tags.clone();
    assert_eq!(tags.iter().map(|t| t.0).collect::<Vec<_>>(), [9, 7]);
}

#[test]
fn a_bitmap_with_no_header_in_reach_is_read_at_most_once_and_never_past_its_cap() {
    // A big file whose name says PNG but whose bytes are not one: its header is in neither read.
    let file = vec![0u8; 4 << 20];
    let reads = Cell::new(0usize);
    let read = |p: &str, max: usize| {
        (p == "ui/junk.png").then(|| {
            reads.set(reads.get() + 1);
            file[..file.len().min(max)].to_vec()
        })
    };
    let charged = Cell::new(0u64);
    let paid = |units: u64| {
        charged.set(charged.get() + units);
        true
    };
    let mut sizes = BitmapSizes::default();
    assert_eq!(sizes.size("ui/junk", &read, &paid), None);
    assert_eq!(reads.get(), 2);
    assert_eq!(
        sizes.bytes_read,
        (BITMAP_HEADER_BYTES + BITMAP_SIZE_BYTES) as u64
    );
    // Charged before the second read, and remembered as unsized.
    assert!(charged.get() >= (BITMAP_HEADER_BYTES + BITMAP_SIZE_BYTES) as u64);
    assert_eq!(sizes.size("ui/junk", &read, &paid), None);
    assert_eq!(reads.get(), 2);
    // A budget that refuses after the header's read: nothing more is read, and the file is remembered unsized too.
    let mut sizes = BitmapSizes::default();
    let left = Cell::new(BITMAP_LOOKUP_WORK * 2 + BITMAP_HEADER_BYTES as u64);
    let budget = |units: u64| {
        let ok = units <= left.get();
        left.set(left.get().saturating_sub(units));
        ok
    };
    reads.set(0);
    assert_eq!(sizes.size("ui/junk", &read, &budget), None);
    assert_eq!(reads.get(), 1);
    assert_eq!(sizes.size("ui/junk", &read, &paid), None);
    assert_eq!(reads.get(), 1);
}
