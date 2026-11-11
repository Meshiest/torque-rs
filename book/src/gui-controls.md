# GUI controls from plugins

A GUI control class is a Rust type. A plugin adds it to the engine while the engine is built, and every client then has the
class: a script says `new GuiTileCtrl(Red) { color = "220 60 60 255"; }` as it would for a class of the engine's. Any number
of plugins can each add controls. The engine's own controls (`GuiButtonCtrl`, `GuiTextEditCtrl` and the rest) are drawn and
handled as they always were; a control of a plugin's class is drawn and handled by its class alone.

## A first control

A control class has two parts. Its **fields** are a struct with `#[derive(EngineClass)]`, the same derive that declares any
console class ([Plugins](plugins.md#prefs-and-classes)): a script writes them, `dump()` lists them, and the methods below read
them back as the struct. Its **behavior** is the methods of `GuiControlClass`; every one has `GuiControl`'s behavior as its
default, so a class writes only what it changes.

```rust
use torque::engine::{EngineBuilder, EngineClass, Plugin, PluginId};
use torque::gui::custom::{ControlCx, GuiBuilderExt, GuiControlClass, Place, PointerEvent};
use torque::gui::draw::DrawCtx;
use torque::script::vm::vm::VmError;

/// A block of one color that tells the script when it is clicked.
#[derive(EngineClass)]
#[class(name = "GuiTileCtrl", parent = "GuiControl")]
struct Tile {
    /// The fill.
    #[field(default = "255 128 0 255")]
    color: [u8; 4],
}

/// What a control keeps beside its fields: made with `Default` when the control is made, gone with it.
#[derive(Default)]
struct Clicks(u32);

impl GuiControlClass for Tile {
    type State = Clicks;

    fn draw(&self, _clicks: &mut Clicks, d: &mut DrawCtx<'_, '_>, at: &Place) {
        d.fill(at.rect(), self.color);
    }

    fn mouse_down(&self, clicks: &mut Clicks, cx: &mut ControlCx<'_>, _ev: &PointerEvent) -> Result<(), VmError> {
        clicks.0 += 1;
        cx.call("onClick", &[clicks.0.to_string().into_bytes()])?;
        Ok(())
    }
}

#[derive(PluginId)]
#[plugin(id = "example.tile")]
struct TilePlugin;

impl Plugin for TilePlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.add_gui_control::<Tile>();
    }
}
```

The script defines what a click does, as for any class: `function GuiTileCtrl::onClick(%this, %count) { ... }`.

## What a control can do

| Method | When | Default |
|---|---|---|
| `draw` | every frame it is drawn, in the clip its parent gave it | the profile's fill and border |
| `resize` | its position or extent is set, by the layout or a script | `cx.base_resize(position, extent)`: `GuiControl::resize` |
| `update` | once a frame while it is awake and shown on the canvas (not in a host's overlay), before the frame is drawn, parents before children; an error is printed and the other controls still update | nothing |
| `wake`, `sleep` | it wakes (after the script's `onWake`) and goes to sleep (before the script's `onSleep`) | nothing |
| `mouse_down`, `mouse_up`, `mouse_move`, `mouse_dragged`, `mouse_enter`, `mouse_leave` | the left button and the pointer; `PointerEvent` has the pointer on the canvas and from the control's corner | nothing; a move then goes to the parent, as a `GuiControl`'s does |
| `key_down` | a key while the control has the key focus; true takes it, false passes it to the parent | false |

- A control's script `onRender` runs once, before its first draw, and its children are drawn after `draw`, so a class that
  is a container needs no drawing of its own. `d.control_box(at.id, at.offset)` draws the profile's fill and border from a
  `draw` that wants them under its own drawing.
- The methods get a `ControlCx` with the script engine, the GUI and the control: `cx.call(name, args)` calls the control's
  script method, `cx.lock_mouse()` and `cx.unlock_mouse()` take and give back the pointer (a drag that leaves the control
  still reaches it), `cx.bounds()` and `cx.screen_position()` read where it is.
- A control that lays out its children places them with `cx.resize_control(child, position, extent, 1)` (the child's own
  `resize`, so what is inside it follows its sizing flags), `cx.set_bounds_of(id, position, extent)` (the bounds alone) and
  `cx.parent_resized_control(child, old, new, 1)` (the child's sizing flags); `cx.gui.control_data::<T, _>(id, f)` keeps a
  value of its own type on any control, dropped with it. The [flex and grid containers](gui-layout.md) are built this way.
- `draw` runs no script and cannot print: a control whose fields cannot be read is not drawn.
- While one of a control's methods runs, the control is busy: a call that reaches it again (a script that sends the same
  control another event) is skipped, not run inside the first. The state is moved out for the call and back after it, so a
  method never holds a borrow of the GUI across script.
- `ACTIVE` (true by default) is whether a new control takes the mouse; set it to false for a control that only shows.
- The first key focus of a control needs a profile with `canKeyFocus`, as for the engine's controls.

## What the build checks

Two plugins that each add a class of the same name (names compare without case) fail the build, and so does a class named
like one of the engine's own GUI classes, so a plugin cannot take over `GuiButtonCtrl`. Each error names the plugin:

```text
the script class `GuiTileCtrl` is declared by both `example.tile` and `example.other`; only one plugin may declare it
the plugin `example.taker`: `GuiButtonCtrl` is one of the engine's own GUI classes: a control class needs a name of its own
```

A class goes below `GuiControl` or below a control class added before it (`parent = "GuiTileCtrl"`); the engine's other GUI
classes are drawn and handled by their names, so a class below one would lose that, and the build refuses it.

## Where the controls are added

`GuiPlugin` (`torque.gui`, in `DefaultPlugins`) hands the controls to each client's GUI as it makes the client, so a game that
turns the GUI plugin off has none, and a dedicated server has no GUI at all. The classes are registered with the rest of
the client's script registry, so a control class shows in the script reference of an engine that has its plugin.

The older seam, `GuiExtension::draw_control`, is deprecated: it is still called for a control class no plugin control
handles, so an extension written before keeps drawing, but a game that draws its own classes through its extension should
write them as control classes instead (the `draw` method takes the place of `draw_control`, and the class gets fields, state,
layout and input of its own). The extension keeps its other jobs (looks for the engine's controls, key rules, fonts).

## More than one extension

A GUI has one primary extension (`GuiExtension`) and any number added after it, so two plugins can each hook the engine's
controls (an opacity, a fill, an outline, a scroll rule) without one replacing the other. A plugin gives the engine a factory,
and the GUI plugin calls it for each client's GUI:

```rust,ignore
engine.gui_primary(|gui, vm| Rc::new(MyExtension::new(gui, vm)));   // one only
engine.gui_extension(|gui, vm| Rc::new(OtherExtension::new(gui, vm))); // any number
```

A second `gui_primary` fails the build and names both plugins. The members are asked primary first, then the added ones in the
order they were added, and each method has a rule for joining their answers:

| Rule | Methods |
|------|---------|
| primary only (what the whole GUI is) | `fonts`, `begin_draw`, `default_canvas_size`, `key_tables`, `rule`, `missing_bitmap`, `message`, `classic_boxes` |
| all, in order; the first `Err` stops the rest | `removed`, `before_control`, `scrolled`, `thumb_dragged`, `mouse_pressed`, `focused`, `before_canvas` |
| the first answer (`Some` or `true`) | `draw_control`, `key_press`, `list_color`, `list_pad`, `text_outline`, `bitmap_fit`, `ml_max_bitmap_height`, `scroll_line_sizes`, `scroll_rects`, `scroll_thumbs` |
| any true | `starts_active`, `replaces_fill`, `ml_auto_resize` |
| true unless a member says no | `clip_to_parent` |
| combined | `opacity` (the product of the answers), `shift` (the sum), `layer_stamp` (a hash of the answers; none when a member cannot say) |

With nothing added, `Gui::ext()` is the primary itself. `Gui::extension::<T>()` finds the primary or the first added extension
of type `T`. A game with a host of its own makes its GUI (`torque_gui::register`) and calls `gui.install_extensions(vm, &engine)`, which
applies the engine's primary and added extensions the way the GUI plugin does; `Gui::set_extension` and `Gui::add_extension` set
one directly.
