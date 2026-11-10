# Flex and grid layout

Torque places every control by hand: a `position` and an `extent` from its parent's corner, and sizing flags (`horizSizing`,
`vertSizing`) that say how it moves when its parent is resized. That is enough for a fixed dialog, but a menu whose buttons
come and go, a list of items that should fill the width it has, or a panel that must work at every window size takes a lot of
arithmetic in script. The layout plugin adds two container controls that do that arithmetic with the rules of CSS flexbox and
CSS grid (computed by the [taffy](https://crates.io/crates/taffy) crate):

- `GuiFlexCtrl` puts its children in a row or a column, wraps them onto more lines if asked, shares out the room left over and
  aligns them;
- `GuiGridCtrl` puts them in the cells of a grid of fixed, `fr` and `auto` tracks, by row and column or in the order they come.

Everything else stays as it was. A container is an ordinary control inside an ordinary GUI, its children are any controls (the
engine's buttons, text, bitmaps, a plugin's controls, plain `GuiControl`s with their own absolute children), and a GUI can mix
absolute parts and laid-out parts at any depth.

## Adding the plugin

The containers are not in `DefaultPlugins`; a game adds `GuiLayoutPlugin` beside them:

```rust
use torque::app::plugins::DefaultPlugins;
use torque::engine::EngineBuilder;
use torque_plugin_gui_layout::GuiLayoutPlugin;

let engine = EngineBuilder::new()
    .add_plugins((DefaultPlugins, GuiLayoutPlugin))
    .build()?;
```

The two classes then exist in every client, and the [script reference](reference/script/classes.md) lists them with their
fields.

## A first menu

```c
new GuiFlexCtrl(MainMenu) {
   profile = "PanelProfile";
   position = "24 24";
   extent = "220 552";
   vertSizing = "height";        // the container itself is placed the Torque way
   direction = "column";
   padding = 16;
   gap = 10;

   new GuiTextCtrl() { profile = "HeadProfile"; extent = "188 40"; text = "Main Menu"; };
   new GuiButtonCtrl() { profile = "ButtonProfile"; extent = "188 44"; text = "Play"; command = "play();"; };
   new GuiButtonCtrl() { profile = "ButtonProfile"; extent = "188 44"; text = "Options"; command = "options();"; };
   new GuiControl() { flexGrow = 1; };                     // takes the room left over
   new GuiButtonCtrl() { profile = "QuitProfile"; extent = "188 44"; text = "Quit"; command = "quit();"; };
};
```

The buttons stack from the top, 10 pixels apart, as wide as the panel inside its padding (a column stretches its children
across by default). The empty control grows into the room that is left, so Quit sits at the bottom whatever the panel's
height; when the window grows taller, the panel grows by its `vertSizing` and the gap grows with it. Add or hide a button and
the rest move up in the next frame.

A grid of items beside it:

```c
new GuiGridCtrl(Items) {
   position = "268 24";
   extent = "508 552";
   horizSizing = "width";
   vertSizing = "height";
   columns = "repeat(auto-fill, minmax(104, 1fr))";   // as many columns of at least 104 as fit, sharing the rest
   autoRows = 80;
   gap = 12;
   padding = 16;

   new GuiControl(Featured) { profile = "TileProfile"; gridColumnSpan = 2; gridRowSpan = 2; };
   // ...one GuiControl per item; each flows into the next free cell
};
```

At 800 x 600 that is four columns; when the window is 1280 wide it is eight, and the items reflow.

## The flex container

| Field | Words | Default |
|---|---|---|
| `direction` | `row` (left to right), `column` (top to bottom), `row-reverse`, `column-reverse` | `row` |
| `wrap` | `nowrap` (one line), `wrap` (as many lines as needed), `wrap-reverse` | `nowrap` |
| `justifyContent` | where the children sit along the line when they leave room: `start`, `end`, `center`, `space-between`, `space-around`, `space-evenly` | `start` |
| `alignItems` | where they sit across the line: `stretch` (fill it), `start`, `end`, `center`, `baseline` | `stretch` |
| `alignContent` | how the lines of a wrapped container share its height (or width): `justifyContent`'s words, or `stretch` | `start` |
| `gap` | room between children: one length, or the room between rows then between columns (`"8 16"`) | 0 |
| `padding` | room inside the edges: one to four lengths, top right bottom left | 0 |

## The grid container

| Field | Words | Default |
|---|---|---|
| `columns`, `rows` | the tracks: `120` (pixels), `25%`, `1fr` (a share of the room left), `auto` (as big as the content), `min-content`, `max-content`, `minmax(80, 1fr)`, `fit-content(200)`, `repeat(3, 1fr)`, `repeat(auto-fill, 96)`, `repeat(auto-fit, 96)` | none |
| `autoColumns`, `autoRows` | the size of a track the template does not name (one the auto-flow or a placement makes) | `auto` |
| `autoFlow` | how children without a place fill the grid: `row`, `column`, `row dense`, `column dense` | `row` |
| `justifyItems`, `alignItems` | where children sit across their cell's width and height: `stretch`, `start`, `end`, `center` (`alignItems` also `baseline`) | `stretch` |
| `justifyContent`, `alignContent` | where the columns and rows sit when they leave room: as the flex container's `justifyContent`, and `stretch` | `start` |
| `gap`, `padding` | as for the flex container | 0 |

A template is at most 256 tracks, every `repeat` counted out. An automatic repeat (`auto-fill`, `auto-fit`) is as many
repetitions as fit the grid's size inside its padding; its tracks need a fixed size (a length, or a `minmax` with one), else
they repeat once, and once too where the grid's size is not known (when its least size is measured). `auto-fit` keeps its empty
tracks, as `auto-fill` does. A field is read to its first 1,024 bytes.

## The children's fields

A container reads these fields of each child. They are dynamic fields, so any control can have them, set in its declaration
or later from script:

| Field | What it does | Default |
|---|---|---|
| `flexGrow` | how much of the room left over the child takes, against its siblings' | 0 |
| `flexShrink` | how much it gives up when there is too little room | 1 |
| `flexBasis` | its size along the line before growing or shrinking: a length, a percentage or `auto` | `auto` (its size) |
| `alignSelf` | `alignItems` for this child alone; `auto` is the container's | `auto` |
| `justifySelf` | `justifyItems` for this child alone (a grid) | `auto` |
| `margin` | room around it: one to four lengths or `auto`, top right bottom left (`auto` margins push it over) | 0 |
| `gridRow`, `gridColumn` | the grid line it starts at: 1 is the first, -1 the last | placed by the auto-flow |
| `gridRowSpan`, `gridColumnSpan` | how many tracks it covers | 1 |
| `layoutWidth`, `layoutHeight` | its size: a length, a percentage of the container, or `auto` (its content's) | its `extent` |
| `layoutMinExtent`, `layoutMaxExtent` | its least and largest size, `"width height"`, each a length, a percentage or `auto` | its `minExtent`; no largest |
| `layoutIgnore` | 1: left out of the layout; it keeps its own position and follows its sizing flags | 0 |

**A child's size.** A child keeps the `extent` it was declared with (or one a script or the text control's own sizing gave it)
unless `layoutWidth`/`layoutHeight` say otherwise, it grows or shrinks along a flex line, or it is stretched: a stretched child
(`alignItems = "stretch"`, the default, across a flex line; `justifyItems` and `alignItems` in a grid cell) takes the container's
size on that axis. The extent the layout gives is not taken as the child's own: put the same child back in a container that does
not stretch, and it has its declared size again. It never shrinks below its `minExtent` (`layoutMinExtent` sets another least
size).

**Hidden children** (`visible = 0`) take no room; show them and the others make way. **Sizing flags** of a laid-out child are not
used for the child itself (the container places it), but they still place what is inside it: a `GuiControl` in a row, with a
child of `horizSizing = "width"`, grows and its child grows with it.

## Nesting

A container inside a container is one layout with it. Its size follows its content unless its own `layoutWidth` and
`layoutHeight` set it, and a change deep inside it (a child added to the inner column) lays out the whole thing again, so the
outer row makes room. Any mix works: a grid in a column of a row, a container inside a plain `GuiControl` that is itself a cell of
a grid. A container that is the child of an absolute control is laid out to the extent Torque gives it.

Nesting is followed 16 levels deep. A chain of containers deeper than that is cut there: the container 16 levels below the
outermost one is a plain child of that layout (of its own `extent`, which its content does not change) and the outermost
container of a layout of its own, and so on every 16 levels. Each layout still lays out once per change.

## Minimum size

The outermost container of a layout never takes an extent smaller than its content needs, the way any control never goes below
its `minExtent`: when a parent's sizing flags or a script make it smaller, it stays as big as its content (children that can shrink
count with their least sizes, children that cannot with their full sizes). So a panel of buttons in a window that is resized too
small overflows its parent rather than squashing its buttons. After each layout the least size is in the container's
`contentMinExtent` field (`"width height"`), for a script that sizes a window or a scroll area to fit.

## When it lays out

- when the container is resized: the canvas's size changed, its parent's sizing flags moved it, a script called `resize`;
- when it wakes (after its children's text controls sized themselves);
- in the frame after anything it reads changed: a child added, removed, shown, hidden or resized; a layout field of a child or a
  field of the container set; a nested container's children or fields changed.

The last check is a fingerprint of those inputs compared once a frame, so a GUI that does not change is not laid out again. A
script that changes a field reads the new positions after the next frame, not on the next line.

A container shown as one of the host's overlays (`Gui::set_overlay`), or inside one, has no such check: it lays out when it
is resized and when it wakes only.

## From Lua

The fields are ordinary fields, so a client's Lua sets them through the object API:

```lua
local menu = ts.object("MainMenu")
menu.direction = "row"
menu.gap = 12
ts.object("PlayButton").flexGrow = 1
```

## In a server's client scripts

A server's client scripts (see [Server scripts](server-scripts.md)) make controls from a closed set of kinds, not from GUI
files, and they cannot name `GuiFlexCtrl`. When the engine has this plugin they get two more kinds, `flex` and `grid`, laid
out by these same classes:

```lua
local menu = ui.new("flex", { size = { 240, 400 }, direction = "column", gap = { 8, 8 }, padding = 16 })
ui.new("button", { text = "Play", size = { 100, 30 } }, menu)
ui.new("button", { text = "Options", size = { 100, 30 } }, menu)
ui.new("panel", { grow = 1 }, menu)                      -- the room left over
ui.new("button", { text = "Quit", size = { 100, 30 } }, menu)
```

The props are the fields above under the names the script uses (a pair or four numbers where the field takes words, a list for
the tracks of a grid):

| Container prop | Field | Lua value |
|---|---|---|
| `direction`, `wrap` (flex) | `direction`, `wrap` | the CSS words, `"column"`, `"wrap-reverse"` |
| `columns`, `rows` (grid) | `columns`, `rows` | a list: `{ 100, "1fr", "auto" }` (lengths, shares, `auto`) |
| `autoFlow`, `justifyItems` (grid) | `autoFlow`, `justifyItems` | the CSS words, `"row dense"` |
| `justifyContent`, `alignItems`, `alignContent` | same | the CSS words |
| `gap` | `gap` | `{ between rows, between columns }` |
| `padding` | `padding` | a number, `{ vertical, horizontal }` or `{ top, right, bottom, left }` |

Every control, of any kind, has the item props that tell the container above it how to place it: `grow`, `shrink`, `basis`
(-1 is the control's own size), `alignSelf`, `justifySelf`, `margin`, `row`, `column`, `rowSpan`, `columnSpan`, `minSize` and
`maxSize` (0 is none). They are the children's fields of the same names (`flexGrow`, `gridColumn`, ...) and do nothing under a
parent that is not a container. A container has no look: no `enabled`, `tooltip` or `opacity`, no events, it draws nothing and
is clicked through (put a `panel` in it, or under it, for a background). A container inside another is laid out at the `size`
the script gave it; without one it follows its content.

The server is not trusted, so every number must be finite and within a cap, and the engine refuses the rest (it stops the
realm; the Lua side refuses first, with an error): containers nest at most 8 deep, a container has at most 256 children, a grid
at most 32 columns and 32 rows, gaps, padding and margins are at most 1,024, a share at most 1,000 and a length at most 16,384.

An engine without this plugin keeps a container as a plain group: it places nothing, its children stay where their own `pos` and
`size` put them, and the item props are ignored. Everything else a script does is unaffected.

## Differences from CSS

- Lengths are pixels and have no unit; percentages are of the container's size (inside its padding).
- `alignContent` and `justifyContent` default to `start` (CSS: `normal`, which stretches the lines of a wrapped flex
  container and the tracks of a grid).
- There are no named grid lines or areas, no `order` and no `display: none` (use `visible`); a placement is a start line and a
  span (`gridColumn = 2; gridColumnSpan = 3;`).
- A child's default size is its `extent`, not its content's: Torque's controls do not measure themselves. Nested containers are
  the exception: they are as big as their content.
