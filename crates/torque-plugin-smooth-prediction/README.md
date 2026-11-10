# torque-plugin-smooth-prediction

A client predictor for a torque-rs game that never shows a snap. When the server corrects the client's prediction, the
simulation corrects exactly as TGE's client does (the same moves, checksums and packets, so it joins any server), and what is
drawn eases the correction out: the body, its vehicle, the eye and the camera start where the old prediction was and move to the
new one over a few ticks. A jump longer than the snap distance (a teleport, a respawn) is drawn at once.

It is not part of the default plugins: a game adds it.

```rust
use torque::app::plugins::DefaultPlugins;
use torque::engine::EngineBuilder;
use torque_plugin_smooth_prediction::{SmoothPrediction, SmoothPredictionPlugin};

let engine = EngineBuilder::new()
    .add_plugins((DefaultPlugins, SmoothPredictionPlugin::new(SmoothPrediction::default())))
    .build()?;
```

`SmoothPredictionPlugin::optional` registers it without making it the game's predictor; a script then swaps with
`setPredictor("smooth")` and back with `setPredictor("tge")`.

| Setting | Default | What it does |
|---|---|---|
| `keep` | 0.75 | the share of the drawn error left after each tick (0 draws as TGE's) |
| `snap_distance` | 5 units | a longer jump is drawn at once |
| `classes` | `Player`, `WheeledVehicle` | the control objects smoothed; others draw as TGE's |

The rotation is drawn as the simulation has it. The book's "Networked plugins" chapter ("Choosing a predictor") compares it with
TGE's.
