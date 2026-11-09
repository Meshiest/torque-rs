//! The layout of one outermost container and the containers nested in it: a taffy tree built from the controls' fields,
//! computed, and written back as the controls' positions and extents.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use taffy::prelude::*;
use taffy::{LayoutOutput, compute_leaf_layout};
use torque_engine::ScriptClass;
use torque_gui::custom::ControlCx;
use torque_gui::{Gui, MAX_TREE_DEPTH, flag, is_a, point};
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use crate::parse::{self, MAX_LENGTH, number};
use crate::{Container, FlexCtrl, GridCtrl, LayoutState, is_container};

/// The fields a container reads of each child (dynamic fields: any control can have them).
///
/// | Field | Words | Default |
/// |---|---|---|
/// | `flexGrow`, `flexShrink` | how much of the room left over the child takes, and gives up when there is too little | 0, 1 |
/// | `flexBasis` | its size along the main axis before growing or shrinking: a length, a percentage or `auto` | `auto`: its size |
/// | `alignSelf` | `alignItems` for this child alone (`auto`: the container's) | `auto` |
/// | `justifySelf` | `justifyItems` for this child alone, in a grid | `auto` |
/// | `margin` | room around it: one to four lengths or `auto`, top right bottom left | 0 |
/// | `gridRow`, `gridColumn` | the line it starts at (1 is the first, -1 the last) | auto-placed |
/// | `gridRowSpan`, `gridColumnSpan` | how many tracks it covers | 1 |
/// | `layoutWidth`, `layoutHeight` | its size: a length, a percentage of the container or `auto` (its content's) | its `extent` |
/// | `layoutMinExtent`, `layoutMaxExtent` | its least and largest size, `"w h"` (`auto`: none; a largest size of 0 is none too) | `minExtent` (a container: its content's), none |
/// | `layoutIgnore` | 1: left out of the layout, placed by its own position and sizing flags | 0 |
pub const CHILD_FIELDS: [&str; 15] = [
    "flexGrow",
    "flexShrink",
    "flexBasis",
    "alignSelf",
    "justifySelf",
    "margin",
    "gridRow",
    "gridColumn",
    "gridRowSpan",
    "gridColumnSpan",
    "layoutWidth",
    "layoutHeight",
    "layoutMinExtent",
    "layoutMaxExtent",
    "layoutIgnore",
];

/// The most controls one layout places (the outermost container's children and every nested container's): more are left
/// where they are.
pub const MAX_NODES: usize = 4096;

/// The deepest nesting of containers one layout follows; a container deeper is laid out as a plain child.
pub const MAX_NESTING: usize = 16;

/// [`MAX_LENGTH`] in whole pixels: the largest extent a container is laid out at (a larger asked extent, `minExtent` or least
/// size is clamped, so a huge extent cannot drive a grid's automatic repeats or taffy's work).
const MAX_PIXELS: i32 = MAX_LENGTH as i32;

/// The longest field text a layout reads (a longer one is cut there): every field it reads is a few words.
pub const MAX_FIELD_BYTES: usize = 1024;

/// What one control costs a layout (`Vm::charge_work` units): reading its fields, its node, and its share of taffy's work.
const NODE_WORK: u64 = 512;

/// What one control costs a fingerprint.
const PRINT_WORK: u64 = 128;

/// What taffy does for a grid beyond its nodes ([`grid_work`]): per cell a child covers (placing it, checking the cell is free),
/// per track and per track a child crosses (sizing them), and per cell a dense auto-flow rescans for each child.
const CELL_WORK: u64 = 8;
const TRACK_WORK: u64 = 64;
const DENSE_WORK: u64 = 2;

/// A field's text.
fn text(vm: &Vm, id: ObjId, name: &str) -> String {
    let raw = vm
        .objects()
        .field(id, name.as_bytes(), b"")
        .unwrap_or_default();
    let mut end = raw.len().min(MAX_FIELD_BYTES);
    // Cut at a character boundary of the UTF-8 (a cut inside one would read as a replacement character).
    while end > 0 && end < raw.len() && (raw[end] & 0xC0) == 0x80 {
        end -= 1;
    }
    String::from_utf8_lossy(&raw[..end]).into_owned()
}

/// A child's own size: its `extent`, except that an extent the layout gave it is not one (the size it had before stays).
#[derive(Debug, Default)]
struct Natural {
    size: Option<(i32, i32)>,
    given: (i32, i32),
}

fn natural_of(vm: &Vm, gui: &Gui, id: ObjId) -> (i32, i32) {
    let now = point(vm, id, b"extent");
    gui.peek_control_data::<Natural, _>(id, |n| match n.size {
        Some(size) if n.given == now => size,
        _ => now,
    })
    .unwrap_or(now)
}

/// The topmost container of the layout `id` is part of, when that is not `id` itself: the chain of containers above it
/// whose child it is (not left out with `layoutIgnore`).
///
/// A chain of nested containers deeper than [`MAX_NESTING`] is cut into layouts of that many levels: the container at each
/// multiple of it below the top is laid out as a plain child by the layout above and is the outermost container of its own.
pub(crate) fn root_of(vm: &Vm, id: ObjId) -> Option<ObjId> {
    let mut chain = vec![id];
    let mut at = id;
    while chain.len() <= MAX_TREE_DEPTH {
        let Some(parent) = torque_gui::parent(vm, at) else {
            break;
        };
        if !is_container(vm, parent) || parse_bool(&text(vm, at, "layoutIgnore")) {
            break;
        }
        chain.push(parent);
        at = parent;
    }
    // `chain[k]` is `k` levels above `id`; the top of the chain is `chain.len() - 1` levels above.
    let up = (chain.len() - 1) % MAX_NESTING;
    (up != 0).then(|| chain[up])
}

/// Whether a container `level` levels below the outermost one is part of its layout (deeper ones start their own).
fn nests(level: usize) -> bool {
    level < MAX_NESTING
}

fn parse_bool(text: &str) -> bool {
    let t = text.trim();
    t.eq_ignore_ascii_case("true") || number(t).is_some_and(|v| v != 0.0)
}

/// Whether a grid's template has an automatic repeat, whose count needs the grid's size.
fn has_auto_repeat(c: &Container) -> bool {
    let auto = |t: &str| {
        let t = t.to_ascii_lowercase();
        t.contains("auto-fill") || t.contains("auto-fit")
    };
    matches!(c, Container::Grid(g) if auto(&g.columns) || auto(&g.rows))
}

/// The container's own style: display, axes, alignment, gaps, padding and tracks. `size` is its width and height where
/// known (a grid's automatic repeats are counted in it; one repetition where it is not known).
pub(crate) fn container_style(c: &Container, size: Size<Option<f32>>) -> Style {
    let lp = |t: &str| parse::sides(t, parse::length_percentage, LengthPercentage::ZERO);
    // The room inside the padding, for a grid's automatic repeats.
    let (room, gaps) = match c {
        Container::Grid(g) => {
            let pad = parse::padding_pixels(&g.padding);
            let room = Size {
                width: size.width.map(|w| (w - pad.left - pad.right).max(0.0)),
                height: size.height.map(|h| (h - pad.top - pad.bottom).max(0.0)),
            };
            (room, parse::gap_pixels(&g.gap))
        }
        Container::Flex(_) => (
            Size {
                width: None,
                height: None,
            },
            (0.0, 0.0),
        ),
    };
    match c {
        Container::Flex(f) => Style {
            display: Display::Flex,
            flex_direction: parse::direction(&f.direction),
            flex_wrap: parse::wrap(&f.wrap),
            justify_content: parse::align_content(&f.justify_content)
                .unwrap_or(AlignContent::START),
            align_items: parse::align_items(&f.align_items).unwrap_or(AlignItems::STRETCH),
            align_content: parse::align_content(&f.align_content).unwrap_or(AlignContent::START),
            gap: parse::gap(&f.gap),
            padding: lp(&f.padding),
            ..Style::default()
        },
        Container::Grid(g) => Style {
            display: Display::Grid,
            grid_template_columns: parse::template(&g.columns, room.width, gaps.1),
            grid_template_rows: parse::template(&g.rows, room.height, gaps.0),
            grid_auto_columns: parse::auto_tracks(&g.auto_columns),
            grid_auto_rows: parse::auto_tracks(&g.auto_rows),
            grid_auto_flow: parse::auto_flow(&g.auto_flow),
            justify_items: parse::align_items(&g.justify_items).unwrap_or(AlignItems::STRETCH),
            align_items: parse::align_items(&g.align_items).unwrap_or(AlignItems::STRETCH),
            justify_content: parse::align_content(&g.justify_content)
                .unwrap_or(AlignContent::START),
            align_content: parse::align_content(&g.align_content).unwrap_or(AlignContent::START),
            gap: parse::gap(&g.gap),
            padding: lp(&g.padding),
            ..Style::default()
        },
    }
}

/// Whether a child of `parent` is stretched across its width and its height (its own size there is then the container's).
fn stretches(
    parent: &Container,
    align_self: Option<AlignItems>,
    justify_self: Option<AlignItems>,
) -> (bool, bool) {
    let is = |own: Option<AlignItems>, field: &str| {
        own.or_else(|| parse::align_items(field))
            .unwrap_or(AlignItems::STRETCH)
            == AlignItems::STRETCH
    };
    match parent {
        Container::Flex(f) => {
            let cross = is(align_self, &f.align_items);
            match parse::direction(&f.direction) {
                FlexDirection::Row | FlexDirection::RowReverse => (false, cross),
                FlexDirection::Column | FlexDirection::ColumnReverse => (cross, false),
            }
        }
        Container::Grid(g) => (
            is(justify_self, &g.justify_items),
            is(align_self, &g.align_items),
        ),
    }
}

/// Which way a layout is computed: as it is drawn, or for the content's least size (a child of a one-line flex container that
/// can shrink is its least size along the main axis).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    Least,
}

/// A size part of `layoutMinExtent`: a length or a percentage (`auto`: none, the content's for a container).
fn least_bound(word: &str) -> Option<LengthPercentageAuto> {
    parse::length_percentage_auto(word).map(|v| {
        if number(word).is_some_and(|n| n < 0.0) {
            LengthPercentageAuto::ZERO
        } else {
            v
        }
    })
}

/// A size part of `layoutMaxExtent`: a length or a percentage; `auto`, 0 or less is none.
fn most_bound(word: &str) -> Option<LengthPercentageAuto> {
    if number(word).is_some_and(|v| v <= 0.0) {
        return None;
    }
    parse::length_percentage_auto(word)
}

/// The style of the child `id` of `parent`: `own` is its own container style when it is a container (its size then follows
/// its content), `natural` its own size when it is not.
fn child_style(
    vm: &Vm,
    id: ObjId,
    parent: &Container,
    own: Option<(&Container, Size<Option<f32>>)>,
    natural: (i32, i32),
    mode: Mode,
) -> Style {
    let field = |name: &str| text(vm, id, name);
    let mut s = own.map_or_else(Style::default, |(c, size)| container_style(c, size));
    let own = own.map(|(c, _)| c);
    s.flex_grow = number(&field("flexGrow")).unwrap_or(0.0).max(0.0);
    s.flex_shrink = number(&field("flexShrink")).unwrap_or(1.0).max(0.0);
    s.flex_basis = parse::dimension(&field("flexBasis")).unwrap_or(Dimension::auto());
    s.align_self = parse::align_items(&field("alignSelf"));
    s.justify_self = parse::align_items(&field("justifySelf"));
    s.margin = parse::sides(
        &field("margin"),
        parse::length_percentage_auto,
        LengthPercentageAuto::ZERO,
    );
    s.grid_row = parse::placement(&field("gridRow"), &field("gridRowSpan"));
    s.grid_column = parse::placement(&field("gridColumn"), &field("gridColumnSpan"));
    let (wide, tall) = stretches(parent, s.align_self, s.justify_self);
    let leaf = own.is_none();
    let size = |set: Option<Dimension>, stretched: bool, natural: i32| {
        set.unwrap_or(if leaf && !stretched {
            Dimension::length(natural.max(0) as f32)
        } else {
            Dimension::auto()
        })
    };
    s.size = Size {
        width: size(parse::dimension(&field("layoutWidth")), wide, natural.0),
        height: size(parse::dimension(&field("layoutHeight")), tall, natural.1),
    };
    let least = parse::pair(&field("layoutMinExtent"), least_bound);
    let min_extent = point(vm, id, b"minExtent");
    let fallback = |v: i32| {
        if leaf {
            LengthPercentageAuto::length(v.max(0) as f32)
        } else {
            LengthPercentageAuto::auto()
        }
    };
    s.min_size = Size {
        width: least.width.unwrap_or_else(|| fallback(min_extent.0)),
        height: least.height.unwrap_or_else(|| fallback(min_extent.1)),
    };
    let most = parse::pair(&field("layoutMaxExtent"), most_bound);
    s.max_size = Size {
        width: most.width.unwrap_or(LengthPercentageAuto::auto()),
        height: most.height.unwrap_or(LengthPercentageAuto::auto()),
    };
    if mode == Mode::Least
        && leaf
        && s.flex_shrink > 0.0
        && let Container::Flex(f) = parent
        && parse::wrap(&f.wrap) == FlexWrap::NoWrap
    {
        let row = matches!(
            parse::direction(&f.direction),
            FlexDirection::Row | FlexDirection::RowReverse
        );
        let to = |v: LengthPercentageAuto| Dimension::from(v);
        if row {
            s.size.width = to(s.min_size.width);
        } else {
            s.size.height = to(s.min_size.height);
        }
        s.flex_basis = Dimension::auto();
    }
    s
}

/// One control the layout places.
struct Placed {
    node: NodeId,
    id: ObjId,
    /// Its own size, for a control that is not a container.
    natural: Option<(i32, i32)>,
    /// How many levels below the outermost container.
    below: usize,
}

/// A layout's tree: the nodes in tree order (parents first), and the children left out (`layoutIgnore`) with their parents.
struct Built {
    tree: TaffyTree<Size<f32>>,
    root: NodeId,
    placed: Vec<Placed>,
    ignored: Vec<(ObjId, ObjId)>,
    /// A nested grid's automatic repeat was counted without its size: a second pass with the sizes found is needed.
    needs_sizes: bool,
}

struct Builder<'a> {
    vm: &'a Vm,
    gui: &'a Gui,
    mode: Mode,
    tree: TaffyTree<Size<f32>>,
    placed: Vec<Placed>,
    ignored: Vec<(ObjId, ObjId)>,
    /// The sizes of nested containers a previous pass found, for their grids' automatic repeats.
    known: &'a HashMap<ObjId, Size<f32>>,
    /// A nested grid with an automatic repeat had no known size (counted as one repetition).
    needs_sizes: bool,
}

impl Builder<'_> {
    fn children(&mut self, id: ObjId, this: &Container, below: usize) -> Vec<NodeId> {
        let vm = self.vm;
        let mut nodes = Vec::new();
        // For a grid: the spans of each child, which its cost depends on; and where this level began, to take it back.
        let mut spans: Vec<(u64, u64)> = Vec::new();
        let (placed_at, ignored_at) = (self.placed.len(), self.ignored.len());
        for child in torque_gui::children(vm, id) {
            if self.placed.len() >= MAX_NODES || !vm.charge_work(NODE_WORK) {
                break;
            }
            if !vm.objects().contains(child)
                || !is_a(vm, child, "GuiControl")
                || !flag(vm, child, b"visible")
            {
                continue;
            }
            if parse_bool(&text(vm, child, "layoutIgnore")) {
                self.ignored.push((child, id));
                continue;
            }
            let nested = if nests(below + 1) {
                Container::read(vm, child)
            } else {
                None
            };
            let at = self.placed.len();
            let node = if let Some(own) = nested {
                // The node is made after its children, so the entry is held in tree order and filled after.
                self.placed.push(Placed {
                    node: NodeId::from(0u64),
                    id: child,
                    natural: None,
                    below: below + 1,
                });
                let found = self.known.get(&child).copied();
                self.needs_sizes |= found.is_none() && has_auto_repeat(&own);
                let size = Size {
                    width: found.map(|s| s.width),
                    height: found.map(|s| s.height),
                };
                let style = child_style(vm, child, this, Some((&own, size)), (0, 0), self.mode);
                let kids = self.children(child, &own, below + 1);
                self.tree.new_with_children(style, &kids)
            } else {
                let natural = natural_of(vm, self.gui, child);
                self.placed.push(Placed {
                    node: NodeId::from(0u64),
                    id: child,
                    natural: Some(natural),
                    below: below + 1,
                });
                let style = child_style(vm, child, this, None, natural, self.mode);
                let content = Size {
                    width: natural.0.max(0) as f32,
                    height: natural.1.max(0) as f32,
                };
                self.tree.new_leaf_with_context(style, content)
            };
            let Ok(node) = node else {
                self.placed.truncate(at);
                continue;
            };
            self.placed[at].node = node;
            nodes.push(node);
            if matches!(this, Container::Grid(_)) {
                spans.push((
                    span_of(vm, child, "gridRowSpan"),
                    span_of(vm, child, "gridColumnSpan"),
                ));
            }
        }
        // A grid costs what its tracks, spans and auto-flow make taffy do, not only its nodes: one too dear for the frame's
        // budget is left as it was (its children keep their bounds), whatever the fields say.
        if let Container::Grid(grid) = this
            && !vm.charge_work(grid_work(grid, &spans))
        {
            self.placed.truncate(placed_at);
            self.ignored.truncate(ignored_at);
            return Vec::new();
        }
        nodes
    }
}

/// How many rows or columns a span field says (at least 1, at most [`parse::MAX_TRACKS`]).
fn span_of(vm: &Vm, id: ObjId, field: &str) -> u64 {
    number(&text(vm, id, field)).map_or(1, |s| s.clamp(1.0, parse::MAX_TRACKS as f32) as u64)
}

/// The tracks a template names, a repeat counted out (at most [`parse::MAX_TRACKS`], as the template reader gives them).
fn track_count(template: &str) -> u64 {
    parse::template(template, None, 0.0)
        .iter()
        .map(|part| match part {
            GridTemplateComponent::Single(_) => 1,
            GridTemplateComponent::Repeat(r) => match r.count {
                RepetitionCount::Count(n) => u64::from(n).saturating_mul(r.tracks.len() as u64),
                _ => r.tracks.len() as u64,
            },
        })
        .sum()
}

/// The work taffy does for the grid `grid` whose children span `spans` (rows, columns) beyond one unit a node: the cells the
/// children cover (placing and checking them), the tracks the grid has and every track a child crosses (sizing them), and, in
/// a dense auto-flow, a rescan of every cell for every child. The rows are those the children need when they flow into the
/// columns there are, so a hostile grid (many children of the widest span) is charged for the implicit rows it makes.
fn grid_work(grid: &GridCtrl, spans: &[(u64, u64)]) -> u64 {
    let (mut area, mut crossings, mut widest) = (0u64, 0u64, 1u64);
    for &(rows, columns) in spans {
        area = area.saturating_add(rows.saturating_mul(columns));
        crossings = crossings.saturating_add(rows + columns);
        widest = widest.max(columns);
    }
    let columns = track_count(&grid.columns).max(widest);
    let rows = track_count(&grid.rows).max(area.div_ceil(columns));
    let mut work = CELL_WORK.saturating_mul(area).saturating_add(
        TRACK_WORK.saturating_mul(crossings.saturating_add(columns).saturating_add(rows)),
    );
    if matches!(
        parse::auto_flow(&grid.auto_flow),
        GridAutoFlow::RowDense | GridAutoFlow::ColumnDense
    ) {
        let cells = columns.saturating_mul(rows);
        work = work.saturating_add(
            DENSE_WORK
                .saturating_mul(cells)
                .saturating_mul(spans.len() as u64),
        );
    }
    work
}

/// The tree of the container `id` (`this`), its own width and height `size` (none: its content's); `known` the sizes of
/// nested containers a previous pass found.
fn build(
    vm: &Vm,
    gui: &Gui,
    id: ObjId,
    this: &Container,
    size: Size<Option<f32>>,
    known: &HashMap<ObjId, Size<f32>>,
    mode: Mode,
) -> Option<Built> {
    let mut b = Builder {
        vm,
        gui,
        mode,
        tree: TaffyTree::new(),
        placed: Vec::new(),
        ignored: Vec::new(),
        known,
        needs_sizes: false,
    };
    let kids = b.children(id, this, 0);
    let style = Style {
        size: Size {
            width: size.width.map_or(Dimension::auto(), Dimension::length),
            height: size.height.map_or(Dimension::auto(), Dimension::length),
        },
        ..container_style(this, size)
    };
    let root = b.tree.new_with_children(style, &kids).ok()?;
    Some(Built {
        tree: b.tree,
        root,
        placed: b.placed,
        ignored: b.ignored,
        needs_sizes: b.needs_sizes,
    })
}

/// Computes the tree: a child that is not a container measures as its own size.
fn compute(built: &mut Built, available: Size<AvailableSpace>) -> Option<()> {
    built
        .tree
        .compute_layout_with_measure(built.root, available, |inputs, _, content, style| {
            let content = content.map_or(Size::ZERO, |c| *c);
            leaf(inputs, style, content)
        })
        .ok()
}

fn leaf(inputs: taffy::LayoutInput, style: &Style, content: Size<f32>) -> LayoutOutput {
    compute_leaf_layout(
        inputs,
        style,
        |_, _| 0.0,
        |known, _| Size {
            width: known.width.unwrap_or(content.width),
            height: known.height.unwrap_or(content.height),
        },
    )
}

/// The tree of the container `id` built and computed in `available`. When a nested grid's automatic repeats needed its size,
/// a second pass counts them in the size the first gave it.
fn laid_out(
    vm: &Vm,
    gui: &Gui,
    id: ObjId,
    this: &Container,
    size: Size<Option<f32>>,
    mode: Mode,
    available: Size<AvailableSpace>,
) -> Option<Built> {
    let mut known = HashMap::new();
    let mut first = build(vm, gui, id, this, size, &known, mode)?;
    compute(&mut first, available)?;
    if !first.needs_sizes {
        return Some(first);
    }
    for p in first.placed.iter().filter(|p| p.natural.is_none()) {
        if let Ok(l) = first.tree.layout(p.node) {
            known.insert(p.id, l.size);
        }
    }
    let mut second = build(vm, gui, id, this, size, &known, mode)?;
    compute(&mut second, available)?;
    Some(second)
}

/// The least size the content of the container `id` needs: the least width, and the height it needs at `width` (or at the
/// least width when that is more).
fn least_size(vm: &Vm, gui: &Gui, id: ObjId, this: &Container, width: i32) -> (i32, i32) {
    let root = |b: Built| b.tree.layout(b.root).ok().map(|l| l.size);
    let least_width = laid_out(
        vm,
        gui,
        id,
        this,
        Size {
            width: None,
            height: None,
        },
        Mode::Least,
        Size {
            width: AvailableSpace::MinContent,
            height: AvailableSpace::MaxContent,
        },
    )
    .and_then(root)
    .map_or(0.0, |s| s.width);
    let at = least_width.max(width as f32).clamp(0.0, MAX_LENGTH);
    let least_height = laid_out(
        vm,
        gui,
        id,
        this,
        Size {
            width: Some(at),
            height: None,
        },
        Mode::Least,
        Size {
            width: AvailableSpace::Definite(at),
            height: AvailableSpace::MaxContent,
        },
    )
    .and_then(root)
    .map_or(0.0, |s| s.height);
    (
        least_width.clamp(0.0, MAX_LENGTH).ceil() as i32,
        least_height.clamp(0.0, MAX_LENGTH).ceil() as i32,
    )
}

/// Lays out the outermost container `cx.id` (`this`) at `position`, asked for `extent`: its extent is raised to its
/// `minExtent` and its content's least size, then every control of the layout is placed.
pub(crate) fn lay_out(
    this: Container,
    state: &mut LayoutState,
    cx: &mut ControlCx<'_>,
    position: (i32, i32),
    extent: (i32, i32),
) {
    let id = cx.id;
    let clamp = |(w, h): (i32, i32)| (w.clamp(0, MAX_PIXELS), h.clamp(0, MAX_PIXELS));
    let extent = clamp(extent);
    let min = clamp(point(cx.vm, id, b"minExtent"));
    let least = least_size(cx.vm, cx.gui, id, &this, extent.0.max(min.0));
    let actual = clamp((
        extent.0.max(min.0).max(least.0),
        extent.1.max(min.1).max(least.1),
    ));
    let old = point(cx.vm, id, b"extent");
    cx.set_bounds_of(id, position, actual);
    let (w, h) = (actual.0 as f32, actual.1 as f32);
    let built = laid_out(
        cx.vm,
        cx.gui,
        id,
        &this,
        Size {
            width: Some(w),
            height: Some(h),
        },
        Mode::Normal,
        Size {
            width: AvailableSpace::Definite(w),
            height: AvailableSpace::Definite(h),
        },
    );
    if let Some(built) = built {
        apply(cx, &built, id, old, actual);
    }
    let least_text = format!("{} {}", least.0, least.1);
    cx.vm
        .set_object_field(id, b"contentMinExtent", b"", least_text.as_bytes());
    state.asked = extent;
    state.placed = (position, actual);
    state.fingerprint = Some(fingerprint(cx.vm, id, &this));
}

/// An extent before and after a layout.
type Change = ((i32, i32), (i32, i32));

/// Writes the computed layout back: the containers' bounds set directly, every other control resized (so what is inside it
/// follows its sizing flags), then the children left out told their parent's new extent.
fn apply(cx: &mut ControlCx<'_>, built: &Built, root: ObjId, old: (i32, i32), new: (i32, i32)) {
    // Each container placed: its extent before and after.
    let mut resized: HashMap<ObjId, Change> = HashMap::new();
    resized.insert(root, (old, new));
    for p in &built.placed {
        let Ok(l) = built.tree.layout(p.node) else {
            continue;
        };
        if !cx.vm.objects().contains(p.id) {
            continue;
        }
        let position = (l.location.x.round() as i32, l.location.y.round() as i32);
        let extent = (l.size.width.round() as i32, l.size.height.round() as i32);
        match p.natural {
            None => {
                let before = point(cx.vm, p.id, b"extent");
                cx.set_bounds_of(p.id, position, extent);
                resized.insert(p.id, (before, extent));
            }
            Some(natural) => {
                cx.resize_control(p.id, position, extent, p.below);
                let given = point(cx.vm, p.id, b"extent");
                cx.gui.control_data::<Natural, _>(p.id, |n| {
                    n.size = Some(natural);
                    n.given = given;
                });
            }
        }
    }
    for &(child, parent) in &built.ignored {
        if let Some(&(old, new)) = resized.get(&parent)
            && old != new
            && cx.vm.objects().contains(child)
        {
            cx.parent_resized_control(child, old, new, 1);
        }
    }
}

/// A fingerprint of everything the layout of the container `id` reads: its own fields and extent, and for each child its
/// id, whether it is shown, its position, extent, `minExtent` and layout fields, a nested container's own fields and children
/// included. Two equal fingerprints lay out the same.
pub(crate) fn fingerprint(vm: &Vm, id: ObjId, this: &Container) -> u64 {
    let mut h = DefaultHasher::new();
    point(vm, id, b"extent").hash(&mut h);
    point(vm, id, b"minExtent").hash(&mut h);
    let mut count = 0usize;
    hash_container(vm, id, this, &mut h, 0, &mut count);
    h.finish()
}

/// `count`: the children hashed so far in the whole fingerprint (at most [`MAX_NODES`]).
fn hash_container(
    vm: &Vm,
    id: ObjId,
    this: &Container,
    h: &mut DefaultHasher,
    below: usize,
    count: &mut usize,
) {
    let fields = match this {
        Container::Flex(_) => FlexCtrl::FIELDS,
        Container::Grid(_) => GridCtrl::FIELDS,
    };
    for f in fields {
        text(vm, id, f.name).hash(h);
    }
    for child in torque_gui::children(vm, id) {
        *count += 1;
        if *count > MAX_NODES || !vm.charge_work(PRINT_WORK) {
            break;
        }
        child.hash(h);
        flag(vm, child, b"visible").hash(h);
        point(vm, child, b"position").hash(h);
        point(vm, child, b"extent").hash(h);
        point(vm, child, b"minExtent").hash(h);
        for name in CHILD_FIELDS {
            text(vm, child, name).hash(h);
        }
        if nests(below + 1)
            && let Some(own) = Container::read(vm, child)
        {
            hash_container(vm, child, &own, h, below + 1, count);
        }
    }
}
