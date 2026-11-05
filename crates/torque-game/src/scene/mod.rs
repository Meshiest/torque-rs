//! The Torque scene classes the server hosts, apart from the host's: `SceneObject` (the transform, the boxes, the type mask and
//! their console methods, [`object`]), `ShapeBase` as data (the shape instance, threads, fades, mounts' transforms,
//! [`shapebase`]), `Camera` and its `processTick` ([`camera`], [`camera_tick`]), each class's own `castRay` and `buildPolyList`
//! ([`rays`]; a Player's posed hitboxes, [`hitboxes`]; lag compensation's rewound casts, [`lag`]), an `InteriorInstance`'s
//! `.dif` and its collision ([`interior`]), and the server container with its queries ([`container`]) and its view as `collide`'s world
//! ([`container_world`]), and the per-class answers a game adds for its own scene classes ([`classes`]).
//!
//! **State.** An object's Rust state is one [`ext::ObjectExt`] in its `SimObject::ext` slot; the host's own state for the
//! object is one value in that struct's [`game`](ext::ObjectExt::game) slot. The scripts' fields stay the VM's static field
//! slots ([`access`]).
//!
//! **Where the host reaches in.** The container is made of a [`container::ContainerConfig`]: the shape of its bins (TGE 1.4's
//! 16 x 16 cells of 64 units by default) and the spatial indexes asked ahead of them, in order (a host may give its own grid
//! and its own index first). A host's own scene classes answer rays, polygon lists, convexes, `setTransform`, world-box changes
//! and the orbit camera's see-through test through the [`classes`] registry; the engine's classes answer here.

pub mod access;
pub mod camera;
pub mod camera_tick;
pub mod classes;
pub mod container;
pub mod container_world;
pub mod ext;
pub mod ghost;
pub mod hitboxes;
pub mod interior;
pub mod lag;
pub mod lights;
pub mod object;
pub mod rays;
pub mod shapebase;
