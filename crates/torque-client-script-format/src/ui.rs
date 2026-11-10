//! The `ui` tier's controls: kinds, props (ids, types, ranges, defaults), the rich-text markup, content paths and caps.
//!
//! **Layout.** Two kinds, `flex` and `grid`, are containers: they hold controls and lay them out by the rules of CSS's flexbox
//! and grid, and have no look of their own (a `panel` inside one is its background). Every control has the item props (`grow`,
//! `column`, `margin`, ...) that tell the container above it how to place it; they do nothing under any other parent. All of it
//! is untrusted data: the reader here checks the ranges of each prop and [`MAX_TRACKS`]; the guest and the GUI's applier check
//! [`MAX_CONTAINER_DEPTH`] and [`MAX_CONTAINER_CHILDREN`], which need the tree. An engine without a layout class keeps the
//! containers as plain groups.

use crate::codec::{Malformed, Reader, Writer};

/// The most live controls in a realm (roots not counted).
pub const MAX_CONTROLS: usize = 1024;
/// How many levels a control may be below its root (a root's child is level 1).
pub const MAX_DEPTH: usize = 32;
/// The most bytes of text props in one control (`text`, `tooltip`, `placeholder`), a richtext's `text` and a list's
/// `rows` not counted.
pub const MAX_CONTROL_TEXT: usize = 4 << 10;
/// The most bytes of a richtext's `text`.
pub const MAX_RICH_TEXT: usize = 16 << 10;
/// The most bytes of a list's `rows`, all rows together.
pub const MAX_LIST_TEXT: usize = 16 << 10;
/// The most bytes of one row of a list.
pub const MAX_ROW_TEXT: usize = 4 << 10;
/// The most bytes of text props in all live controls together.
pub const MAX_TEXT: usize = 1 << 20;
/// The most distinct images live controls show.
pub const MAX_IMAGES: usize = 256;
/// The widest and tallest an image may be, in pixels.
pub const MAX_IMAGE_SIDE: u32 = 2048;
/// The most bytes the images live controls show take decoded (four a pixel), all together.
pub const MAX_IMAGE_BYTES: usize = 64 << 20;
/// The most rows of a list.
pub const MAX_ROWS: usize = 1024;
/// How long after an activating event of the realm's own controls `ui.focus` is allowed, in microseconds.
pub const ACTIVATION_US: u64 = 1_000_000;
/// The longest content path.
pub const MAX_PATH: usize = 256;
/// How many containers may be nested (a container inside a container inside ...), the container itself counted.
pub const MAX_CONTAINER_DEPTH: usize = 8;
/// The most children a container has.
pub const MAX_CONTAINER_CHILDREN: usize = 256;
/// The most tracks of a grid's `columns` or `rows`, and the most a child's `row`, `column` or span names.
pub const MAX_TRACKS: usize = 32;
/// The most a gap, a padding side or a margin side is, in UI units.
pub const MAX_SPACING: f64 = 1024.0;
/// The most a `grow` or `shrink` or a track's `fr` is.
pub const MAX_SHARE: f64 = 1000.0;
/// The most a length of layout (a track's size, a `basis`, a `minSize`) is, in UI units.
pub const MAX_LENGTH: f64 = 16384.0;
/// How many markup tags may be open at once in rich text.
pub const MAX_RICH_NESTING: usize = 8;
/// The extensions of image content paths.
pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg"];

/// A control's kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Kind {
    /// A rectangle: fill, border, radius.
    Panel = 0,
    /// Plain text, shown literally.
    Text = 1,
    /// Text in the markup subset ([`check_rich`]).
    RichText = 2,
    /// An image from content.
    Image = 3,
    /// A button with text and an optional image.
    Button = 4,
    /// A check box with text.
    Toggle = 5,
    /// A one-line text field (never masked).
    TextField = 6,
    /// A list of text rows, one selectable.
    List = 7,
    /// A scrolling area.
    Scroll = 8,
    /// A slider over a range.
    Slider = 9,
    /// A progress bar.
    Progress = 10,
    /// A container that lays its children out in a row or a column ([`Container::Flex`]).
    Flex = 11,
    /// A container that lays its children out in a grid ([`Container::Grid`]).
    Grid = 12,
}

/// The layout a container kind has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Container {
    /// Flexbox: children in a row or a column.
    Flex,
    /// Grid: children in the cells of rows and columns.
    Grid,
}

impl Kind {
    /// The layout of a container kind; none for the others.
    pub fn container(self) -> Option<Container> {
        match self {
            Kind::Flex => Some(Container::Flex),
            Kind::Grid => Some(Container::Grid),
            _ => None,
        }
    }
}

/// A prop's id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Prop {
    /// `pos`: vec2, the top-left corner in the parent, in UI units.
    Pos = 1,
    /// `size`: vec2.
    Size = 2,
    /// `hsize`: how the control follows its parent's width (GuiControl's horizontal sizing rules).
    HSize = 3,
    /// `vsize`: how the control follows its parent's height (GuiControl's vertical sizing rules).
    VSize = 4,
    /// `visible`.
    Visible = 5,
    /// `enabled`: a disabled control is drawn dimmed and sends no events.
    Enabled = 6,
    /// `tooltip`: plain text.
    Tooltip = 7,
    /// `opacity`: 0-1, multiplied down the tree.
    Opacity = 8,
    /// `fill`: a panel's (or a progress bar's) fill color.
    Fill = 16,
    /// `border`: a panel's border color.
    Border = 17,
    /// `radius`: a panel's corner radius.
    Radius = 18,
    /// `text`: plain text.
    Text = 19,
    /// `text` of a richtext: markup.
    RichText = 20,
    /// `font`: a font role.
    Font = 21,
    /// `fontSize`.
    FontSize = 22,
    /// `color`: the text's (or a progress bar's) color.
    Color = 23,
    /// `align`.
    Align = 24,
    /// `wrap`.
    Wrap = 25,
    /// `image`: a content path, or empty for none.
    Image = 26,
    /// `tint`.
    Tint = 27,
    /// `fit`: how an image fills its control.
    Fit = 28,
    /// `style`: a button's look.
    Style = 29,
    /// `on`: a toggle's state.
    On = 30,
    /// `placeholder`: a text field's hint while empty.
    Placeholder = 31,
    /// `maxLength`: a text field's most characters.
    MaxLength = 32,
    /// `numeric`: a text field that takes numbers only.
    Numeric = 33,
    /// `rows`: a list's rows.
    Rows = 34,
    /// `selected`: a list's selected row (1-based; 0 none), at most its row count.
    Selected = 35,
    /// `contentSize`: a scroll area's content size.
    ContentSize = 36,
    /// `range`: a slider's lowest and highest value (x below y).
    Range = 37,
    /// `value` of a slider: within its range.
    SliderValue = 38,
    /// `value` of a progress bar: 0-1.
    Progress = 39,
    /// `step`: a slider's step (0: none).
    Step = 40,
    /// `grow`: the share of the room left over a flex child takes.
    Grow = 41,
    /// `shrink`: the share of the shortage a flex child gives up.
    Shrink = 42,
    /// `basis`: a flex child's size along the main axis before growing or shrinking (-1: its own size).
    Basis = 43,
    /// `alignSelf`: this child's own cross-axis alignment.
    AlignSelf = 44,
    /// `justifySelf`: this child's own alignment across its grid cell's width.
    JustifySelf = 45,
    /// `margin`: the room around a child.
    Margin = 46,
    /// `row`: the grid row (1-based) a child starts in (0: placed by the auto-flow).
    Row = 47,
    /// `column`: the grid column (1-based) a child starts in (0: placed by the auto-flow).
    Column = 48,
    /// `rowSpan`: the rows a grid child covers.
    RowSpan = 49,
    /// `columnSpan`: the columns a grid child covers.
    ColumnSpan = 50,
    /// `minSize`: the least size a child is laid out at (0: its own).
    MinSize = 51,
    /// `maxSize`: the largest size a child is laid out at (0: none).
    MaxSize = 52,
    /// `direction`: a flex container's main axis.
    Direction = 53,
    /// `wrap` of a flex container: whether and how children wrap onto more lines.
    LineWrap = 54,
    /// `justifyContent`: where the children sit along the main axis (of a grid: where the columns sit).
    JustifyContent = 55,
    /// `alignItems`: where the children sit across their line (of a grid: across their cell's height).
    AlignItems = 56,
    /// `alignContent`: where the lines sit across the container (of a grid: where the rows sit).
    AlignContent = 57,
    /// `gap`: the room between rows, then between columns.
    Gap = 58,
    /// `padding`: the room inside the container's edges.
    Padding = 59,
    /// `columns`: a grid's column tracks.
    Columns = 60,
    /// `rows` of a grid: its row tracks.
    TrackRows = 61,
    /// `autoFlow`: how a grid places children that have no row or column.
    AutoFlow = 62,
    /// `justifyItems`: where a grid's children sit across their cell's width.
    JustifyItems = 63,
}

/// A prop's type and range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PropType {
    /// `bool`.
    Bool,
    /// A finite `f64` in `min..=max`.
    Number {
        /// The lowest value.
        min: f64,
        /// The highest value.
        max: f64,
    },
    /// A `u32` in `min..=max`.
    Int {
        /// The lowest value.
        min: u32,
        /// The highest value.
        max: u32,
    },
    /// Two finite `f64`s, each in `min..=max`.
    Vec2 {
        /// The lowest value of either.
        min: f64,
        /// The highest value of either.
        max: f64,
    },
    /// Four bytes, RGBA.
    Color,
    /// Plain text of at most `max` bytes ([`check_text`]).
    Text {
        /// The most bytes.
        max: usize,
    },
    /// Rich text of at most `max` bytes ([`check_rich`]).
    Rich {
        /// The most bytes.
        max: usize,
    },
    /// An index into these names.
    Enum(&'static [&'static str]),
    /// An image content path, or empty.
    Image,
    /// Rows of plain text.
    Rows,
    /// Four finite `f64`s (top, right, bottom, left), each in `min..=max`.
    Sides {
        /// The lowest value of any side.
        min: f64,
        /// The highest value of any side.
        max: f64,
    },
    /// At most `max` grid tracks ([`Track`]).
    Tracks {
        /// The most tracks.
        max: usize,
    },
}

/// One grid track: how wide a column or how tall a row is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Track {
    /// As much as its content needs.
    Auto,
    /// A length in UI units, 0 to [`MAX_LENGTH`].
    Px(f64),
    /// A share of the room left over, 0 to [`MAX_SHARE`] (`1fr`).
    Fr(f64),
}

/// A prop's value.
#[derive(Clone, Debug, PartialEq)]
pub enum PropValue {
    /// A bool.
    Bool(bool),
    /// A number.
    Number(f64),
    /// A whole number.
    Int(u32),
    /// A pair of numbers.
    Vec2(f64, f64),
    /// RGBA.
    Color([u8; 4]),
    /// Text, rich text or an image path.
    Text(String),
    /// An index into the prop's names.
    Enum(u8),
    /// Rows of text.
    Rows(Vec<String>),
    /// Top, right, bottom and left.
    Sides([f64; 4]),
    /// Grid tracks.
    Tracks(Vec<Track>),
}

/// A prop's default, as a constant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PropDefault {
    /// A bool.
    Bool(bool),
    /// A number.
    Number(f64),
    /// A whole number.
    Int(u32),
    /// A pair of numbers.
    Vec2(f64, f64),
    /// RGBA.
    Color([u8; 4]),
    /// Text (or an empty path).
    Text(&'static str),
    /// An index into the prop's names.
    Enum(u8),
    /// No rows.
    NoRows,
    /// The same on all four sides.
    Sides(f64),
    /// No tracks.
    NoTracks,
}

impl PropDefault {
    /// The default as a value.
    pub fn value(self) -> PropValue {
        match self {
            PropDefault::Bool(b) => PropValue::Bool(b),
            PropDefault::Number(n) => PropValue::Number(n),
            PropDefault::Int(n) => PropValue::Int(n),
            PropDefault::Vec2(x, y) => PropValue::Vec2(x, y),
            PropDefault::Color(c) => PropValue::Color(c),
            PropDefault::Text(t) => PropValue::Text(t.to_string()),
            PropDefault::Enum(i) => PropValue::Enum(i),
            PropDefault::NoRows => PropValue::Rows(Vec::new()),
            PropDefault::Sides(n) => PropValue::Sides([n; 4]),
            PropDefault::NoTracks => PropValue::Tracks(Vec::new()),
        }
    }
}

/// A prop: its id, its Lua name, its type and its default.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PropDef {
    /// The id.
    pub prop: Prop,
    /// The name scripts use.
    pub name: &'static str,
    /// The type and range.
    pub ty: PropType,
    /// What a control has until it is set.
    pub default: PropDefault,
}

const WHITE: [u8; 4] = [255, 255, 255, 255];
const CLEAR: [u8; 4] = [0, 0, 0, 0];
const COORD: f64 = 16384.0;

/// Every prop, by id.
pub const PROPS: &[PropDef] = &[
    PropDef {
        prop: Prop::Pos,
        name: "pos",
        ty: PropType::Vec2 {
            min: -COORD,
            max: COORD,
        },
        default: PropDefault::Vec2(0.0, 0.0),
    },
    PropDef {
        prop: Prop::Size,
        name: "size",
        ty: PropType::Vec2 {
            min: 0.0,
            max: COORD,
        },
        default: PropDefault::Vec2(128.0, 32.0),
    },
    PropDef {
        prop: Prop::HSize,
        name: "hsize",
        ty: PropType::Enum(&["right", "width", "left", "center", "relative"]),
        default: PropDefault::Enum(0),
    },
    PropDef {
        prop: Prop::VSize,
        name: "vsize",
        ty: PropType::Enum(&["bottom", "height", "top", "center", "relative"]),
        default: PropDefault::Enum(0),
    },
    PropDef {
        prop: Prop::Visible,
        name: "visible",
        ty: PropType::Bool,
        default: PropDefault::Bool(true),
    },
    PropDef {
        prop: Prop::Enabled,
        name: "enabled",
        ty: PropType::Bool,
        default: PropDefault::Bool(true),
    },
    PropDef {
        prop: Prop::Tooltip,
        name: "tooltip",
        ty: PropType::Text { max: 256 },
        default: PropDefault::Text(""),
    },
    PropDef {
        prop: Prop::Opacity,
        name: "opacity",
        ty: PropType::Number { min: 0.0, max: 1.0 },
        default: PropDefault::Number(1.0),
    },
    PropDef {
        prop: Prop::Fill,
        name: "fill",
        ty: PropType::Color,
        default: PropDefault::Color(CLEAR),
    },
    PropDef {
        prop: Prop::Border,
        name: "border",
        ty: PropType::Color,
        default: PropDefault::Color(CLEAR),
    },
    PropDef {
        prop: Prop::Radius,
        name: "radius",
        ty: PropType::Number {
            min: 0.0,
            max: 64.0,
        },
        default: PropDefault::Number(0.0),
    },
    PropDef {
        prop: Prop::Text,
        name: "text",
        ty: PropType::Text {
            max: MAX_CONTROL_TEXT,
        },
        default: PropDefault::Text(""),
    },
    PropDef {
        prop: Prop::RichText,
        name: "text",
        ty: PropType::Rich { max: MAX_RICH_TEXT },
        default: PropDefault::Text(""),
    },
    PropDef {
        prop: Prop::Font,
        name: "font",
        ty: PropType::Enum(&["sans", "mono", "title"]),
        default: PropDefault::Enum(0),
    },
    PropDef {
        prop: Prop::FontSize,
        name: "fontSize",
        ty: PropType::Number {
            min: 8.0,
            max: 72.0,
        },
        default: PropDefault::Number(14.0),
    },
    PropDef {
        prop: Prop::Color,
        name: "color",
        ty: PropType::Color,
        default: PropDefault::Color(WHITE),
    },
    PropDef {
        prop: Prop::Align,
        name: "align",
        ty: PropType::Enum(&["left", "center", "right"]),
        default: PropDefault::Enum(0),
    },
    PropDef {
        prop: Prop::Wrap,
        name: "wrap",
        ty: PropType::Bool,
        default: PropDefault::Bool(false),
    },
    PropDef {
        prop: Prop::Image,
        name: "image",
        ty: PropType::Image,
        default: PropDefault::Text(""),
    },
    PropDef {
        prop: Prop::Tint,
        name: "tint",
        ty: PropType::Color,
        default: PropDefault::Color(WHITE),
    },
    PropDef {
        prop: Prop::Fit,
        name: "fit",
        ty: PropType::Enum(&["stretch", "contain", "cover", "center"]),
        default: PropDefault::Enum(0),
    },
    PropDef {
        prop: Prop::Style,
        name: "style",
        ty: PropType::Enum(&["normal", "primary", "danger", "flat"]),
        default: PropDefault::Enum(0),
    },
    PropDef {
        prop: Prop::On,
        name: "on",
        ty: PropType::Bool,
        default: PropDefault::Bool(false),
    },
    PropDef {
        prop: Prop::Placeholder,
        name: "placeholder",
        ty: PropType::Text { max: 256 },
        default: PropDefault::Text(""),
    },
    PropDef {
        prop: Prop::MaxLength,
        name: "maxLength",
        ty: PropType::Int { min: 1, max: 256 },
        default: PropDefault::Int(256),
    },
    PropDef {
        prop: Prop::Numeric,
        name: "numeric",
        ty: PropType::Bool,
        default: PropDefault::Bool(false),
    },
    PropDef {
        prop: Prop::Rows,
        name: "rows",
        ty: PropType::Rows,
        default: PropDefault::NoRows,
    },
    PropDef {
        prop: Prop::Selected,
        name: "selected",
        ty: PropType::Int {
            min: 0,
            max: MAX_ROWS as u32,
        },
        default: PropDefault::Int(0),
    },
    PropDef {
        prop: Prop::ContentSize,
        name: "contentSize",
        ty: PropType::Vec2 {
            min: 0.0,
            max: 65536.0,
        },
        default: PropDefault::Vec2(0.0, 0.0),
    },
    PropDef {
        prop: Prop::Range,
        name: "range",
        ty: PropType::Vec2 {
            min: -1e9,
            max: 1e9,
        },
        default: PropDefault::Vec2(0.0, 1.0),
    },
    PropDef {
        prop: Prop::SliderValue,
        name: "value",
        ty: PropType::Number {
            min: -1e9,
            max: 1e9,
        },
        default: PropDefault::Number(0.0),
    },
    PropDef {
        prop: Prop::Progress,
        name: "value",
        ty: PropType::Number { min: 0.0, max: 1.0 },
        default: PropDefault::Number(0.0),
    },
    PropDef {
        prop: Prop::Step,
        name: "step",
        ty: PropType::Number { min: 0.0, max: 1e9 },
        default: PropDefault::Number(0.0),
    },
    PropDef {
        prop: Prop::Grow,
        name: "grow",
        ty: PropType::Number {
            min: 0.0,
            max: MAX_SHARE,
        },
        default: PropDefault::Number(0.0),
    },
    PropDef {
        prop: Prop::Shrink,
        name: "shrink",
        ty: PropType::Number {
            min: 0.0,
            max: MAX_SHARE,
        },
        default: PropDefault::Number(1.0),
    },
    PropDef {
        prop: Prop::Basis,
        name: "basis",
        ty: PropType::Number {
            min: -1.0,
            max: MAX_LENGTH,
        },
        default: PropDefault::Number(-1.0),
    },
    PropDef {
        prop: Prop::AlignSelf,
        name: "alignSelf",
        ty: PropType::Enum(&["auto", "stretch", "start", "end", "center", "baseline"]),
        default: PropDefault::Enum(0),
    },
    PropDef {
        prop: Prop::JustifySelf,
        name: "justifySelf",
        ty: PropType::Enum(&["auto", "stretch", "start", "end", "center"]),
        default: PropDefault::Enum(0),
    },
    PropDef {
        prop: Prop::Margin,
        name: "margin",
        ty: PropType::Sides {
            min: 0.0,
            max: MAX_SPACING,
        },
        default: PropDefault::Sides(0.0),
    },
    PropDef {
        prop: Prop::Row,
        name: "row",
        ty: PropType::Int {
            min: 0,
            max: MAX_TRACKS as u32,
        },
        default: PropDefault::Int(0),
    },
    PropDef {
        prop: Prop::Column,
        name: "column",
        ty: PropType::Int {
            min: 0,
            max: MAX_TRACKS as u32,
        },
        default: PropDefault::Int(0),
    },
    PropDef {
        prop: Prop::RowSpan,
        name: "rowSpan",
        ty: PropType::Int {
            min: 1,
            max: MAX_TRACKS as u32,
        },
        default: PropDefault::Int(1),
    },
    PropDef {
        prop: Prop::ColumnSpan,
        name: "columnSpan",
        ty: PropType::Int {
            min: 1,
            max: MAX_TRACKS as u32,
        },
        default: PropDefault::Int(1),
    },
    PropDef {
        prop: Prop::MinSize,
        name: "minSize",
        ty: PropType::Vec2 {
            min: 0.0,
            max: MAX_LENGTH,
        },
        default: PropDefault::Vec2(0.0, 0.0),
    },
    PropDef {
        prop: Prop::MaxSize,
        name: "maxSize",
        ty: PropType::Vec2 {
            min: 0.0,
            max: MAX_LENGTH,
        },
        default: PropDefault::Vec2(0.0, 0.0),
    },
    PropDef {
        prop: Prop::Direction,
        name: "direction",
        ty: PropType::Enum(&["row", "column", "row-reverse", "column-reverse"]),
        default: PropDefault::Enum(0),
    },
    PropDef {
        prop: Prop::LineWrap,
        name: "wrap",
        ty: PropType::Enum(&["nowrap", "wrap", "wrap-reverse"]),
        default: PropDefault::Enum(0),
    },
    PropDef {
        prop: Prop::JustifyContent,
        name: "justifyContent",
        ty: PropType::Enum(DISTRIBUTE),
        default: PropDefault::Enum(0),
    },
    PropDef {
        prop: Prop::AlignItems,
        name: "alignItems",
        ty: PropType::Enum(&["stretch", "start", "end", "center", "baseline"]),
        default: PropDefault::Enum(0),
    },
    PropDef {
        prop: Prop::AlignContent,
        name: "alignContent",
        ty: PropType::Enum(DISTRIBUTE),
        default: PropDefault::Enum(0),
    },
    PropDef {
        prop: Prop::Gap,
        name: "gap",
        ty: PropType::Vec2 {
            min: 0.0,
            max: MAX_SPACING,
        },
        default: PropDefault::Vec2(0.0, 0.0),
    },
    PropDef {
        prop: Prop::Padding,
        name: "padding",
        ty: PropType::Sides {
            min: 0.0,
            max: MAX_SPACING,
        },
        default: PropDefault::Sides(0.0),
    },
    PropDef {
        prop: Prop::Columns,
        name: "columns",
        ty: PropType::Tracks { max: MAX_TRACKS },
        default: PropDefault::NoTracks,
    },
    PropDef {
        prop: Prop::TrackRows,
        name: "rows",
        ty: PropType::Tracks { max: MAX_TRACKS },
        default: PropDefault::NoTracks,
    },
    PropDef {
        prop: Prop::AutoFlow,
        name: "autoFlow",
        ty: PropType::Enum(&["row", "column", "row dense", "column dense"]),
        default: PropDefault::Enum(0),
    },
    PropDef {
        prop: Prop::JustifyItems,
        name: "justifyItems",
        ty: PropType::Enum(&["stretch", "start", "end", "center"]),
        default: PropDefault::Enum(0),
    },
];

/// Where a container puts its content (or its lines) when it leaves room: the words of CSS's `justify-content`.
const DISTRIBUTE: &[&str] = &[
    "start",
    "end",
    "center",
    "stretch",
    "space-between",
    "space-around",
    "space-evenly",
];

/// The props that place a control in its parent: every kind has them.
const PLACE: &[Prop] = &[
    Prop::Pos,
    Prop::Size,
    Prop::HSize,
    Prop::VSize,
    Prop::Visible,
];

/// The props of a control's look and input: every kind but the containers has them.
const LOOK: &[Prop] = &[Prop::Enabled, Prop::Tooltip, Prop::Opacity];

/// The props that tell a container how to place a child: every kind has them (and they do nothing under a parent that is not
/// a container).
const ITEM: &[Prop] = &[
    Prop::Grow,
    Prop::Shrink,
    Prop::Basis,
    Prop::AlignSelf,
    Prop::JustifySelf,
    Prop::Margin,
    Prop::Row,
    Prop::Column,
    Prop::RowSpan,
    Prop::ColumnSpan,
    Prop::MinSize,
    Prop::MaxSize,
];

/// An event a control sends (`h:on(name, fn)`), its id the `ui` event kind's ([`crate::event::ui`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Event {
    /// A click of a panel, text or image.
    Click = 0,
    /// The pointer went over (or off) a panel or image.
    Hover = 1,
    /// A button was pressed.
    Press = 2,
    /// A toggle, text field or slider changed.
    Change = 3,
    /// Enter in a text field.
    Submit = 4,
    /// A list's selection changed.
    Select = 5,
    /// A scroll area moved.
    Scroll = 6,
}

impl Event {
    /// Every control event, by id.
    pub const ALL: [Event; 7] = [
        Event::Click,
        Event::Hover,
        Event::Press,
        Event::Change,
        Event::Submit,
        Event::Select,
        Event::Scroll,
    ];

    /// The control event with the `ui` event kind `id` (`screen` and `closed` are not a control's).
    pub fn from_id(id: u8) -> Option<Event> {
        Event::ALL.get(usize::from(id)).copied()
    }

    /// The name scripts use.
    pub fn name(self) -> &'static str {
        match self {
            Event::Click => "click",
            Event::Hover => "hover",
            Event::Press => "press",
            Event::Change => "change",
            Event::Submit => "submit",
            Event::Select => "select",
            Event::Scroll => "scroll",
        }
    }

    /// Whether it is the player acting on the control, which lets `ui.focus` follow within [`ACTIVATION_US`].
    pub fn activates(self) -> bool {
        !matches!(self, Event::Hover | Event::Scroll)
    }
}

/// A kind: its id, its Lua name, its own props (beyond [`KindDef::props`]'s common ones) and the events its controls send.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KindDef {
    /// The kind.
    pub kind: Kind,
    /// The name scripts use (`ui.new(name, ...)`).
    pub name: &'static str,
    /// Its props beyond the common ones.
    pub own: &'static [Prop],
    /// The events (`h:on(name, fn)`) its controls send.
    pub events: &'static [Event],
}

impl KindDef {
    /// Whether the kind has `prop` (common, an item prop or its own).
    pub fn has(&self, prop: Prop) -> bool {
        self.props().any(|p| p == prop)
    }

    /// The kind's prop named `name`.
    pub fn prop_named(&self, name: &str) -> Option<&'static PropDef> {
        self.props().map(prop_def).find(|d| d.name == name)
    }

    /// Every prop of the kind: the common ones first (a container has no look props), then the item props, then its own.
    pub fn props(&self) -> impl Iterator<Item = Prop> + '_ {
        let look = if self.kind.container().is_some() {
            &[][..]
        } else {
            LOOK
        };
        PLACE
            .iter()
            .chain(look)
            .chain(ITEM)
            .chain(self.own)
            .copied()
    }
}

/// Every kind, by id.
pub const KINDS: &[KindDef] = &[
    KindDef {
        kind: Kind::Panel,
        name: "panel",
        own: &[Prop::Fill, Prop::Border, Prop::Radius],
        events: &[Event::Click, Event::Hover],
    },
    KindDef {
        kind: Kind::Text,
        name: "text",
        own: &[
            Prop::Text,
            Prop::Font,
            Prop::FontSize,
            Prop::Color,
            Prop::Align,
            Prop::Wrap,
        ],
        events: &[Event::Click],
    },
    KindDef {
        kind: Kind::RichText,
        name: "richtext",
        own: &[
            Prop::RichText,
            Prop::Font,
            Prop::FontSize,
            Prop::Color,
            Prop::Wrap,
        ],
        events: &[],
    },
    KindDef {
        kind: Kind::Image,
        name: "image",
        own: &[Prop::Image, Prop::Tint, Prop::Fit],
        events: &[Event::Click, Event::Hover],
    },
    KindDef {
        kind: Kind::Button,
        name: "button",
        own: &[
            Prop::Text,
            Prop::Font,
            Prop::FontSize,
            Prop::Image,
            Prop::Style,
        ],
        events: &[Event::Press],
    },
    KindDef {
        kind: Kind::Toggle,
        name: "toggle",
        own: &[
            Prop::Text,
            Prop::Font,
            Prop::FontSize,
            Prop::Color,
            Prop::On,
        ],
        events: &[Event::Change],
    },
    KindDef {
        kind: Kind::TextField,
        name: "textfield",
        own: &[
            Prop::Text,
            Prop::Font,
            Prop::FontSize,
            Prop::Color,
            Prop::Placeholder,
            Prop::MaxLength,
            Prop::Numeric,
        ],
        events: &[Event::Change, Event::Submit],
    },
    KindDef {
        kind: Kind::List,
        name: "list",
        own: &[
            Prop::Rows,
            Prop::Selected,
            Prop::Font,
            Prop::FontSize,
            Prop::Color,
        ],
        events: &[Event::Select],
    },
    KindDef {
        kind: Kind::Scroll,
        name: "scroll",
        own: &[Prop::ContentSize],
        events: &[Event::Scroll],
    },
    KindDef {
        kind: Kind::Slider,
        name: "slider",
        own: &[Prop::Range, Prop::SliderValue, Prop::Step],
        events: &[Event::Change],
    },
    KindDef {
        kind: Kind::Progress,
        name: "progress",
        own: &[Prop::Progress, Prop::Fill, Prop::Color],
        events: &[],
    },
    KindDef {
        kind: Kind::Flex,
        name: "flex",
        own: &[
            Prop::Direction,
            Prop::LineWrap,
            Prop::JustifyContent,
            Prop::AlignItems,
            Prop::AlignContent,
            Prop::Gap,
            Prop::Padding,
        ],
        events: &[],
    },
    KindDef {
        kind: Kind::Grid,
        name: "grid",
        own: &[
            Prop::Columns,
            Prop::TrackRows,
            Prop::AutoFlow,
            Prop::JustifyItems,
            Prop::JustifyContent,
            Prop::AlignItems,
            Prop::AlignContent,
            Prop::Gap,
            Prop::Padding,
        ],
        events: &[],
    },
];

/// The kind with id `id`.
pub fn kind_from_id(id: u8) -> Option<&'static KindDef> {
    KINDS.get(usize::from(id))
}

/// The kind named `name`.
pub fn kind_named(name: &str) -> Option<&'static KindDef> {
    KINDS.iter().find(|k| k.name == name)
}

/// A kind's definition.
pub fn kind_def(kind: Kind) -> &'static KindDef {
    &KINDS[kind as usize]
}

/// A prop's definition.
///
/// # Panics
///
/// Never: [`PROPS`] has every prop (a test checks it).
pub fn prop_def(prop: Prop) -> &'static PropDef {
    prop_from_id(prop as u8).expect("PROPS has every prop")
}

/// The prop with id `id`.
pub fn prop_from_id(id: u8) -> Option<&'static PropDef> {
    PROPS.iter().find(|d| d.prop as u8 == id)
}

/// Whether `c` is a control character (C0, DEL, C1).
/// Plain text: at most `max` bytes, no character [`crate::is_hidden`] names but `\n` and `\t`.
pub fn check_text(text: &str, max: usize) -> Result<(), &'static str> {
    if text.len() > max {
        return Err("the text is too long");
    }
    if text
        .chars()
        .any(|c| crate::is_hidden(c) && c != '\n' && c != '\t')
    {
        return Err("the text has a control, separator or direction character");
    }
    Ok(())
}

/// Rich text: plain text ([`check_text`]) with markup tags in angle brackets. `<<` is a literal `<`; a `>` alone is
/// itself. The tags: `<b>` `</b>` (bold), `<i>` `</i>` (italic), `<color:rrggbb>` or `<color:rrggbbaa>` (hex) and
/// `</color>`, `<size:N>` (N a whole number 8-72) and `</size>`, `<align:left>` / `center` / `right` (from there to the
/// end of the line; no closing tag), `<br>` (a line break). A closing tag closes the innermost open tag of its name; at
/// most [`MAX_RICH_NESTING`] are open at once; tags still open at the end close there. Nothing else: no links, images,
/// fonts or any other tag.
pub fn check_rich(text: &str, max: usize) -> Result<(), &'static str> {
    check_text(text, max)?;
    let mut open: Vec<&str> = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find('<') {
        rest = &rest[at + 1..];
        if let Some(after) = rest.strip_prefix('<') {
            rest = after;
            continue;
        }
        let Some(end) = rest.find('>') else {
            return Err("a `<` without its `>` (write `<<` for the character)");
        };
        let tag = &rest[..end];
        rest = &rest[end + 1..];
        if let Some(name) = tag.strip_prefix('/') {
            match open.pop() {
                Some(top) if top == name => {}
                _ => return Err("a closing tag that closes nothing open"),
            }
            continue;
        }
        let (name, arg) = tag.split_once(':').unwrap_or((tag, ""));
        let ok = match (name, arg) {
            ("b" | "i", "") => true,
            ("color", hex) => {
                matches!(hex.len(), 6 | 8) && hex.bytes().all(|b| b.is_ascii_hexdigit())
            }
            ("size", n) => {
                n.len() <= 2
                    && !n.is_empty()
                    && n.bytes().all(|b| b.is_ascii_digit())
                    && n.parse::<u32>().is_ok_and(|n| (8..=72).contains(&n))
            }
            ("align", "left" | "center" | "right") | ("br", "") => {
                continue;
            }
            _ => false,
        };
        if !ok {
            return Err("a tag that is not in the markup (b, i, color, size, align, br)");
        }
        if open.len() >= MAX_RICH_NESTING {
            return Err("tags nested too deep");
        }
        open.push(name);
    }
    Ok(())
}

/// A content path: `mods/<mod id>/<parts>`, every part [`crate::is_plain_part`], at most [`MAX_PATH`] bytes, its
/// extension (lowercase) one of `extensions`. No backslash, drive, NUL or absolute path. Only the form: whether it is
/// this server's content is the host's check, by looking it up in the server's file list (never on disk).
pub fn check_content_path(path: &str, extensions: &[&str]) -> Result<(), &'static str> {
    if path.len() > MAX_PATH {
        return Err("the path is too long");
    }
    let mut parts = path.split('/');
    if parts.next() != Some("mods") {
        return Err("a content path starts with mods/<mod id>/");
    }
    let mut count = 0;
    for part in parts {
        count += 1;
        if !crate::is_plain_part(part) {
            return Err("a path part that is not plain (letters, digits, `_`, `-`, `.`)");
        }
    }
    if count < 2 {
        return Err("a content path names a file of a mod");
    }
    let extension = path.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    if !extensions.contains(&extension) {
        return Err("a content path of the wrong kind of file");
    }
    Ok(())
}

/// Checks `value` against `def`'s type and range.
pub fn check_value(def: &PropDef, value: &PropValue) -> Result<(), &'static str> {
    let in_range = |v: f64, min: f64, max: f64| v.is_finite() && v >= min && v <= max;
    match (def.ty, value) {
        (PropType::Bool, PropValue::Bool(_)) | (PropType::Color, PropValue::Color(_)) => Ok(()),
        (PropType::Number { min, max }, PropValue::Number(v)) => {
            if in_range(*v, min, max) {
                Ok(())
            } else {
                Err("a number out of range")
            }
        }
        (PropType::Int { min, max }, PropValue::Int(v)) => {
            if (min..=max).contains(v) {
                Ok(())
            } else {
                Err("a number out of range")
            }
        }
        (PropType::Vec2 { min, max }, PropValue::Vec2(x, y)) => {
            if in_range(*x, min, max) && in_range(*y, min, max) {
                Ok(())
            } else {
                Err("a number out of range")
            }
        }
        (PropType::Text { max }, PropValue::Text(t)) => check_text(t, max),
        (PropType::Rich { max }, PropValue::Text(t)) => check_rich(t, max),
        (PropType::Image, PropValue::Text(t)) => {
            if t.is_empty() {
                Ok(())
            } else {
                check_content_path(t, IMAGE_EXTENSIONS)
            }
        }
        (PropType::Enum(names), PropValue::Enum(i)) => {
            if usize::from(*i) < names.len() {
                Ok(())
            } else {
                Err("not one of the names")
            }
        }
        (PropType::Sides { min, max }, PropValue::Sides(sides)) => {
            if sides.iter().all(|&v| in_range(v, min, max)) {
                Ok(())
            } else {
                Err("a number out of range")
            }
        }
        (PropType::Tracks { max }, PropValue::Tracks(tracks)) => {
            if tracks.len() > max {
                return Err("too many tracks");
            }
            tracks.iter().try_for_each(check_track)
        }
        (PropType::Rows, PropValue::Rows(rows)) => {
            if rows.len() > MAX_ROWS {
                return Err("too many rows");
            }
            for row in rows {
                check_text(row, MAX_ROW_TEXT)?;
            }
            if rows.iter().map(String::len).sum::<usize>() > MAX_LIST_TEXT {
                return Err("the rows hold too much text");
            }
            Ok(())
        }
        _ => Err("a value of the wrong type"),
    }
}

/// One track within its range.
fn check_track(track: &Track) -> Result<(), &'static str> {
    let ok = match *track {
        Track::Auto => true,
        Track::Px(v) => v.is_finite() && (0.0..=MAX_LENGTH).contains(&v),
        Track::Fr(v) => v.is_finite() && (0.0..=MAX_SHARE).contains(&v),
    };
    if ok {
        Ok(())
    } else {
        Err("a track out of range")
    }
}

/// The most characters one number of [`field_text`] takes: a sign, up to ten digits (the widest range is 1e9), a point and
/// [`NUMBER_DECIMALS`] decimals.
pub const MAX_NUMBER_TEXT: usize = 16;
/// The decimals [`number_text`] keeps.
const NUMBER_DECIMALS: usize = 4;

/// A finite number as short text: at most [`NUMBER_DECIMALS`] decimals, none trailing, and no `-0` (a value that rounds to
/// nothing is `0`). `1e-300` is `0`, not three hundred digits.
pub fn number_text(n: f64) -> String {
    let fixed = format!("{n:.NUMBER_DECIMALS$}");
    let trimmed = if fixed.contains('.') {
        fixed.trim_end_matches('0').trim_end_matches('.')
    } else {
        &fixed
    };
    match trimmed {
        "-0" | "" => "0".to_string(),
        other => other.to_string(),
    }
}

/// A prop's value as the words of CSS that a layout class reads from a console field: a number as itself (to four
/// decimals, [`number_text`]), a pair as `x y`, sides as `top right bottom left`, tracks as `120 1fr auto`, an enum as its
/// name, a flag as `1` or `0`. Text and rows have no such form (empty). A number is at most [`MAX_NUMBER_TEXT`] characters.
pub fn field_text(def: &PropDef, value: &PropValue) -> String {
    match value {
        PropValue::Bool(on) => String::from(if *on { "1" } else { "0" }),
        // A negative basis is "its own size", which CSS spells `auto` (a negative length is read as none).
        PropValue::Number(n) if def.prop == Prop::Basis && *n < 0.0 => "auto".to_string(),
        PropValue::Number(n) => number_text(*n),
        PropValue::Int(n) => n.to_string(),
        PropValue::Vec2(x, y) => format!("{} {}", number_text(*x), number_text(*y)),
        PropValue::Sides(sides) => sides
            .iter()
            .map(|&v| number_text(v))
            .collect::<Vec<_>>()
            .join(" "),
        PropValue::Enum(i) => match def.ty {
            PropType::Enum(names) => names.get(usize::from(*i)).copied().unwrap_or_default(),
            _ => "",
        }
        .to_owned(),
        PropValue::Tracks(tracks) => tracks
            .iter()
            .map(|t| match t {
                Track::Auto => "auto".to_string(),
                Track::Px(v) => number_text(*v),
                Track::Fr(v) => format!("{}fr", number_text(*v)),
            })
            .collect::<Vec<_>>()
            .join(" "),
        PropValue::Color(_) | PropValue::Text(_) | PropValue::Rows(_) => String::new(),
    }
}

/// The bytes a value counts for the text caps.
pub fn text_bytes(value: &PropValue) -> usize {
    match value {
        PropValue::Text(t) => t.len(),
        PropValue::Rows(rows) => rows.iter().map(String::len).sum(),
        _ => 0,
    }
}

/// Writes one prop's payload.
fn write_value(out: &mut Writer, value: &PropValue) {
    match value {
        PropValue::Bool(b) => out.bool(*b),
        PropValue::Number(n) => out.f64(*n),
        PropValue::Int(n) => out.u32(*n),
        PropValue::Vec2(x, y) => {
            out.f64(*x);
            out.f64(*y);
        }
        PropValue::Color(c) => c.iter().for_each(|&b| out.u8(b)),
        PropValue::Text(t) => out.text(t),
        PropValue::Enum(i) => out.u8(*i),
        PropValue::Rows(rows) => {
            out.u16(rows.len() as u16);
            for row in rows {
                out.text(row);
            }
        }
        PropValue::Sides(sides) => sides.iter().for_each(|&v| out.f64(v)),
        PropValue::Tracks(tracks) => {
            // A list past the cap is the reader's to refuse (a count of 255 is past every cap), never a wrapped count.
            out.u8(u8::try_from(tracks.len()).unwrap_or(u8::MAX));
            for track in tracks {
                match *track {
                    Track::Auto => out.u8(TRACK_AUTO),
                    Track::Px(v) => {
                        out.u8(TRACK_PX);
                        out.f64(v);
                    }
                    Track::Fr(v) => {
                        out.u8(TRACK_FR);
                        out.f64(v);
                    }
                }
            }
        }
    }
}

/// A track's tag on the wire: `auto`, then a length (`f64`), then a share (`f64`).
const TRACK_AUTO: u8 = 0;
const TRACK_PX: u8 = 1;
const TRACK_FR: u8 = 2;

/// Writes props: a count and the pairs, which the caller gives in ascending id order (a `BTreeMap<Prop, _>`'s order),
/// each checked, at most one of each prop.
pub fn write_props<'a>(
    out: &mut Writer,
    props: impl ExactSizeIterator<Item = (Prop, &'a PropValue)>,
) {
    out.u8(props.len() as u8);
    for (prop, value) in props {
        out.u8(prop as u8);
        write_value(out, value);
    }
}

/// Reads one payload of `def`'s type.
fn read_value(reader: &mut Reader<'_>, def: &PropDef) -> Result<PropValue, Malformed> {
    Ok(match def.ty {
        PropType::Bool => PropValue::Bool(reader.bool()?),
        PropType::Number { .. } => PropValue::Number(reader.finite()?),
        PropType::Int { .. } => PropValue::Int(reader.u32()?),
        PropType::Vec2 { .. } => PropValue::Vec2(reader.finite()?, reader.finite()?),
        PropType::Color => {
            let b = reader.take(4)?;
            PropValue::Color([b[0], b[1], b[2], b[3]])
        }
        PropType::Text { .. } | PropType::Rich { .. } | PropType::Image => {
            PropValue::Text(reader.text()?.to_string())
        }
        PropType::Enum(_) => PropValue::Enum(reader.u8()?),
        PropType::Rows => {
            let count = usize::from(reader.u16()?);
            if count > MAX_ROWS {
                return Err(Malformed("more rows than MAX_ROWS"));
            }
            let mut rows = Vec::with_capacity(count.min(reader.rest().len() / 4));
            for _ in 0..count {
                rows.push(reader.text()?.to_string());
            }
            PropValue::Rows(rows)
        }
        PropType::Sides { .. } => PropValue::Sides([
            reader.finite()?,
            reader.finite()?,
            reader.finite()?,
            reader.finite()?,
        ]),
        PropType::Tracks { max } => {
            let count = usize::from(reader.u8()?);
            if count > max {
                return Err(Malformed("more tracks than the prop's cap"));
            }
            // Every track takes at least a byte, so the bytes left bound what is reserved.
            let mut tracks = Vec::with_capacity(count.min(reader.rest().len()));
            for _ in 0..count {
                tracks.push(match reader.u8()? {
                    TRACK_AUTO => Track::Auto,
                    TRACK_PX => Track::Px(reader.finite()?),
                    TRACK_FR => Track::Fr(reader.finite()?),
                    _ => return Err(Malformed("an unknown track")),
                });
            }
            PropValue::Tracks(tracks)
        }
    })
}

/// Reads the props of a control of `kind`: ids ascending, each the kind's, each value of its type and in range (the
/// rules that need the control's other state, such as a slider's value within its range, are the reader's to check).
pub fn read_props(
    reader: &mut Reader<'_>,
    kind: &KindDef,
) -> Result<Vec<(Prop, PropValue)>, Malformed> {
    let count = reader.u8()?;
    let mut props = Vec::with_capacity(usize::from(count));
    let mut last = 0u8;
    for _ in 0..count {
        let id = reader.u8()?;
        if id <= last {
            return Err(Malformed("prop ids not strictly ascending"));
        }
        last = id;
        let def = prop_from_id(id).ok_or(Malformed("an unknown prop"))?;
        if !kind.has(def.prop) {
            return Err(Malformed("a prop the kind does not have"));
        }
        let value = read_value(reader, def)?;
        check_value(def, &value).map_err(|_| Malformed("a prop value out of its type or range"))?;
        props.push((def.prop, value));
    }
    Ok(props)
}
