# torque-plugin-rope

Ropes for a torque-rs game: cables, tethers and washing lines that a script hangs between two points or from an object. A rope is
simulated on the client (position-based dynamics in substeps of 1/240 s, run for each drawn frame and shown between its last two states: it sags, swings, takes a push and keeps its length) and
drawn as a lit, fogged tube that casts a shadow from the sun, or, by the datablock's `style`, as a ribbon or a line.

It is not part of the default plugins: a game adds it. It needs the engine's renderer to be seen (the `renderer` feature of
`torque-app`, on by default); a game that draws with another renderer, or a dedicated server, runs no rope.

```rust
use torque::app::plugins::DefaultPlugins;
use torque::engine::EngineBuilder;
use torque_plugin_rope::RopePlugin;

let engine = EngineBuilder::new()
    .add_plugins((DefaultPlugins, RopePlugin::default()))
    .build()?;
```

The game's client scripts then describe a kind of rope and hang it:

```
datablock RopeData(WashingLine)
{
   segments = 16;                  // pieces, 2 to 64
   length = 9;                     // 0: 1.1 times the distance between the ends
   thickness = 0.04;
   stiffness = 0;                  // 0 (a cord) to 1
   color = "200 200 190 255";
   castShadow = 1;                 // 1, 0, or -1 (the default) for the style's own
   receiveShadow = 1;              // the same
   texture = "art/braid.png";      // optional: a picture along the rope, tinted by color
   textureLength = 0.4;            // world units a repeat (0: the width)
   textureScroll = 0;              // repeats a second along the rope
   sim = "verlet";                 // "none": driven, see Beams below
   emissiveIntensity = 1;          // HDR brightness of a glow
   additive = false;               // a ribbon or line adds its light
   endThickness = -1;              // a taper to this width (-1: none)
   colorEnd = "";                  // a gradient to this color (empty: none)
   style = "tube";                 // "tube" (the default), "ribbon" or "line": see Styles below
   emissive = false;               // ribbon and line only: no light or shade, like a laser
};

%rope = ropeCreate("-4 0 3", "4 0 3", WashingLine);   // both ends pinned, sagging between the points
ropeAttach(%rope, 1, PostB);                          // the far end follows the object PostB
ropeImpulse(%rope, "0 3 0");                          // a push (units a second)
ropeRelease(%rope, 0);                                // the near end lets go
ropeDelete(%rope);
```

## Console functions

| Function | Does |
|---|---|
| `ropeCreate(start, end, datablock)` | makes a rope between two points (`"x y z"`), both ends pinned; returns its number |
| `ropeAttach(rope, end, object [, node])` | hangs an end (0 or `start`, 1 or `end`) from an object, which it follows |
| `ropePin(rope, end, point)` | pins an end to a point |
| `ropeRelease(rope, end)` | lets an end go, so it falls with the rest |
| `ropeImpulse(rope, velocity)` | gives the free part of a rope a velocity |
| `ropeDelete(rope)`, `ropeClear()`, `ropeCount()` | take a rope or every rope away; count them |
| `ropeFloor(z)`, `ropeNoFloor()` | keep the free points above a height, or not |

An object is read every client tick through the plugin's `AnchorResolver`: by default its `position` field (or the first three
numbers of its `transform`), so a script object that moves itself carries a rope; `RopePlugin::with_anchors` takes a game's own
reader. When the reader says which ghost an object is (`AnchorResolver::ghost`), the end follows where the frame *draws* that ghost,
each frame, exactly (`FrameAnchors`; the default `DrawnShapes` puts an empty node at the shape's origin and a number at the middle
of that mesh as animated, and any other node at the origin; a game supplies its own mounts). A rope hung from an object that goes
lets go of it.

## Styles

| `style` | Drawn as | Simulated | Casts a shadow | Takes a shadow |
|---|---|---|---|---|
| `"tube"` | a lit, fogged eight-sided tube | yes | yes | yes |
| `"ribbon"` | a flat strip that always faces the camera, along a smooth Catmull-Rom curve through the simulated points | yes | no | yes |
| `"line"` | a straight strip between the two ends, no sag | no (an end follows its pin; a released end stays) | no | yes, unless `emissive` |

`castShadow` and `receiveShadow` (1, 0, or -1 for the style's own, the default) change the last two columns for a rope. A ribbon or a
line that casts makes a band of its thickness that faces the sun, so the shadow on the ground does not change as the camera moves.
Taking a shadow darkens the rope as the renderer's own surfaces are darkened, through the same cascades and filter.

A ribbon reads as a smooth curve even with a few segments: cheap cables, vines, beams and tethers. A line is a laser, a tripwire
or a power line seen from far away; it is one segment against the caps. `thickness` is the strip's width; on the screen it is never
less than two pixels, so a far strip stays a thin line, and a strip seen end-on keeps a width (its ends are pushed out by half of
it). The `color`'s alpha makes a strip see-through and `emissive = true` draws it without light or shade (fog still covers it).
The edges are faded across the last pixel. A strip is drawn with the frame's depth: things in front of it hide it, and it hides
what is behind it; it has no collision. A style that is none of the three prints a warning when the rope is made and makes a tube.

```
datablock RopeData(Laser) { style = "line"; thickness = 0.04; color = "255 40 40 220"; emissive = true; };
%beam = ropeCreate("-4 0 1.2", "4 0 1.2", Laser);
```

## Textures

`texture` is a `.png` or `.jpg` read through the engine's asset loaders (their caps and trust rules, and their hot reload). `u` runs along
the rope by arc length, a repeat every `textureLength` world units (default: the rope's width, a square tile), so the texture keeps its shape
as the rope sags and runs on across joints and along a ribbon's curve; `v` runs around a tube or across a strip. `color` multiplies it,
`textureScroll` runs it along the rope in repeats a second, and `textureFilter = "nearest"` keeps hard texels (else linear with mips). Ribbons
and lines blend by the texture's alpha; a tube cuts out under half alpha (and the cut-out casts no shadow). A file that is missing or does
not load warns once and the rope is drawn plain.

`ropeSetColor`, `ropeSetTexture`, `ropeSetTextureScroll` and `ropeSetEmissive` (with `ropeGet...` readers, and `ropeClearLook`) change one
rope and leave the datablock alone; the values live in a small `RopeLook` per rope.

## Beams and driven shapes

`sim = "none"` (or `ropeSetCurve`) makes a rope's shape driven, not simulated: straight, or a cubic Bezier from the start (leaving along
`startDir`, `""` toward the other end or `"mount"` for the way the end's mount points) to the far end (leaving along `endDir`), the handles
`bend` times the distance long. Both ends and the mounts' directions follow what is drawn each frame. With `additive` (the strip adds
its light), `emissiveIntensity` above 1 (brighter than white, for the bloom), `endThickness` (a taper) and `colorEnd` (a gradient) a
ribbon is an energy beam. `MuzzleAnchors` attaches an end to the muzzle of a weapon a ghost holds, as the frame drew it (the
first-person weapon's for the player whose view it is, with the predicted aim; the third-person mount's for the rest and in third
person); a shape with no muzzle node ties the end to its origin pointing along its forward. The arena example has it
(`cargo run -p torque-example-fps --features rope --bin fps-client -- --beam-shots <dir>`).

## What it does and does not do

- **Caps.** At most 256 ropes and 4,096 segments over all of them, 64 segments to a rope. A call past a cap prints a console error
  and makes nothing. A datablock's numbers are clamped to their ranges when they are written and again when a rope reads them, and
  a number that is not a number reads as the field's default.
- **Hostile input.** A point, a velocity or a time step that is not finite is made finite on the way in; a coordinate past 100,000 is
  held there; a speed past 200 units a second is held to it; an end follows its anchor at most 60 units a second, so an object that
  jumps across the map is followed over a few ticks; a rope whose points still go bad is laid out again between its ends.
- **Clients.** The ropes belong to the engine, not to a client, and each drawn frame runs them: one client per engine. Ropes are
  not sent over the network yet; each client makes its own.
- **Drawing.** Eight sides to a tube, lit as the frame lights its surfaces (the sun's diffuse and ambient light) and fogged. The tube
  casts a shadow into the sun's cascades and takes the sun's shadows (both can be turned off), and has no motion vector, so a fast rope smears a little under the
  anti-aliasing. Ribbons and lines are drawn after the translucent surfaces, alpha blended over the frame's depth, their triangles
  made again each frame the camera or the ropes move.
- **Time.** The ropes run per drawn frame, 240 substeps a second at most eight a frame, and are shown between their last two
  states. A client that draws nothing runs them from its 32 ms tick (after eight ticks with no frame).

## Tests

`cargo test -p torque-plugin-rope`: the simulation's stability over ten thousand steps, its caps and hostile numbers, lines and ribbons (`tests/sim.rs`); the strips'
geometry, in `src/strip.rs` and `tests/strip.rs`;
the console functions and the client's ticks (`tests/script.rs`); and pictures drawn offscreen of a rope between two posts, lit and
casting a shadow, swinging and bouncing, and of a ribbon and a line hidden by a post in front of them (`tests/picture.rs`; `TORQUE_ROPE_PICTURES=<folder>` writes them as PNGs). The tests that
draw skip themselves when the machine has no GPU.

The book's "Writing a plugin" chapter builds a smaller version of this plugin step by step; its code is `examples/walkthrough`.
