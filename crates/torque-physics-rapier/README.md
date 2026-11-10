# torque-physics-rapier

A deterministic [Rapier](https://rapier.rs) world for torque-rs: rigid bodies stepped at the engine's 32 ms tick against a
map's terrain and interiors, with every value checked and every count capped, giving the same state bit for bit on native
and wasm32. It is the shared core of a server's persistent world (props and joints) and of a scratch world a rigid-body
solver builds for one tick.

It is not a plugin, and nothing in the default plugin sets depends on it: a crate that wants it adds it.

```rust
use torque_physics_rapier::{BodyDesc, BodyId, Config, Shape, World};

let mut world = World::new(Config::default());
world.add_map(terrain_id, terrain_shape)?; // a torque_collide::map::MapShape, as the engine's collision holds it
world.add_body(BodyId(7), &BodyDesc { shape: Shape::Box { half: [0.5; 3] }, position: [0.0, 0.0, 30.0], ..BodyDesc::default() }, &mut charge)?;
world.step(&mut charge)?; // one tick, in the world's fixed sub-steps
let state = world.body(BodyId(7));
```

## What it does

- **Map pieces as colliders, by region.** The xy plane is cut into 64-unit cells, built the first time a body comes near:
  a terrain's squares as triangles cut along the engine's own diagonal for each square (a height field's single diagonal
  would not match the engine's collision), an interior's hulls as convex polyhedra.
- **Shapes.** Boxes, balls, and convex hulls of points (a shape's collision mesh: `Shape::from_mesh_hull`). A hull of more
  than `Limits::max_hull_vertices` points, a flat one, or one the hull builder cannot make becomes the box of its points. A
  `Shape::Compound` holds several hulls (a prop's collision meshes) in one body, each made so and placed at its own middle; more
  than `Limits::max_compound_hulls` (64), or none, becomes the box of all its points. Points in one plane (at any tilt), on one
  line or on one point become their box too, before the hull builder runs: a panic inside it cannot be caught on wasm32, so that
  check is the guard there.
- **Determinism.** Rapier's `enhanced-determinism` (scalar math, `libm`, ordered maps), no threads, a fixed step and
  sub-step count per world, bodies kept and walked in id order, cells built in a fixed order. `just trace-hash` runs the
  `rapier-drop` scenario (64 boxes and 8 hulls dropped onto the example hills and the keep for 600 ticks) natively and as
  wasm32 and compares the hashes.

## Limits

Everything that reaches the world is checked first: a non-finite value refuses the call (`Error::NonFinite`), magnitudes are
held to `Limits` (positions, speeds, spins, impulses, sizes, densities), rotations are normalized. The caps: bodies (1,024),
colliders (16,384, map pieces included), hull vertices (256), solver contacts after a sub-step (16,384; past it the tick's
remaining sub-steps are skipped), map cells a query (64). A body that leaves the world's reach (100,000 units) or whose state
stops being finite is put back where it was before the step, at rest. Work is charged to the caller's budget (the units of
`torque_collide::work`) before it is done; a refused step moves nothing. Native builds also catch a panic inside Rapier and
poison the world; a wasm32 build aborts on one, which the checks are there to prevent.

## Tests

- `cargo test -p torque-physics-rapier`: the terrain's triangles against the engine's terrain heights, cells built on first
  use, rest on terrain and interiors, refusals, caps, the budget, and two worlds holding the same bits.
- `TORQUE_FUZZ_RAPIER=100000 cargo test -p torque-physics-rapier --test fuzz`: hostile hulls, terrains, interiors,
  transforms and limits: no panic, every count within its cap.
- `cargo bench -p torque-physics-rapier --bench step`: 64 boxes for 100 ticks, and a one-body scratch world built and stepped.

## Licenses

Rapier, parry, nalgebra, simba and approx are under the Apache License 2.0. Their notice is [`NOTICE`](NOTICE) and the
license's text [`LICENSE-APACHE`](LICENSE-APACHE); both are compiled into a binary that links this crate
(`torque_physics_rapier::NOTICE`, `torque_physics_rapier::APACHE_LICENSE`), and a test checks the notice against the
lockfile.
