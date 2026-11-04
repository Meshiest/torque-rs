# Networked objects

A Torque server does not send its world to a client as a whole. Each connection keeps a table of **ghosts**: the
objects in the client's scope, each with an index on that connection. A new ghost's first record carries its class and
its full state; later records carry only the parts whose **mask bits** are dirty. When an object leaves the scope or
is deleted, its ghost is killed. **Datablocks**, the shared static data (a shape's file, an item's mass), are sent once,
before ghosting starts, and the ghosts' records name them by id.

Three crates make up the layer:

| Crate | What it does |
|---|---|
| `torque-net` | The packet layer, the event layer and the ghost manager: scope, priorities, the ghost section of a packet, the notify chains, `GhostClient` (the client's ghost table). It knows nothing about any class's fields. |
| `torque-ghosts` | The records: the layout framework, the parent parts, TGE 1.4's object layouts (`TgeGhosts`) and datablock layouts (`TgeDatablocks`). |
| `torque-ghost-world` | The client's world: the records merged into ghosts, the datablocks, poses, mounted images, script threads and effects, over TGE's set (`TgeClientSet`) or a game's. |

## One layout per record

A record's layout is one function: the calls a `Codec` makes, in wire order. The same function is run by the encoder (it
reads the record and writes bits), by the decoder (it reads bits from hostile input and fills the record) and by a
logging encoder (it lists every field with its width, which the tests pin). The two directions cannot disagree about
order, widths or which blocks are present.

```rust,ignore
use torque_ghosts::codec::{Codec, Layout, R, opt};
use torque_ghosts::parts::ShapeBasePart;
use torque_ghosts::TgeShapeBase;

/// A game's own ShapeBase class: TGE's ShapeBase part, then a colour block.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Beacon {
    pub shape: TgeShapeBase,
    /// `flag(mask & ColorMask)` and a 24-bit colour.
    pub color: Option<u32>,
}

/// The class's first mask bit is the part's next free one.
pub const COLOR_MASK: u32 = TgeShapeBase::NEXT_FREE_MASK;

impl Layout for Beacon {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.shape.layout(c)?;
        opt(c, "beacon.color?", &mut self.color, |c, v| c.int("beacon.color", v, 24))
    }
}
```

`torque_ghosts::encode` writes a record and `torque_ghosts::decode` reads one. The decoder never panics and never
allocates from a count on the wire without checking it against the bits left and a named cap. Every id width (ghost
index, datablock id) comes from the connection's `NetWidths`, never from a literal.

## Packing: the host's view of an object

The server packs a record from an `ObjectView`, the host's answer to "what is field `name`, element `i`, of this
object, as this connection sees it". The engine's packers read TGE's fields by plain names (`position`, `scale`,
`transform`, `datablock`, `rotate`, ...) and apply the class's mask rules, as `packUpdate` does:

```rust,ignore
let ctx = PackCtx::for_writer(&w, widths, now_ms);
let (record, still_dirty) = TgeGhosts::pack(TgeClass::Item, &view, mask, &ctx);
TgeGhosts::encode(&record, &mut w, &table, widths)?;
```

A view can answer `link` (a pointer field resolved to the ghost index on this connection) and `net_string` (a string
sent through the connection's string table). The host's `ObjectSource` (torque-net) wraps the view: it tells the ghost
manager which objects exist, their net class ids, their scope (`scope_query`) and their priority, and packs them.

## Registering a ghost codec

A game's classes are a `GhostCodecSet`: a marker type with a class enum and a record enum, which names its classes
by their engine names, packs, decodes and encodes them. `TgeGhosts` is TGE's. A game that adds classes or changes a
layout writes its own set, reusing the engine's records for the classes it leaves alone:

```rust,ignore
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MyClass { Tge(TgeClass), Beacon }

#[derive(Debug, Clone, PartialEq)]
pub enum MyRecord { Tge(TgeRecord), Beacon(Beacon) }

pub struct MyGhosts;

impl GhostCodecSet for MyGhosts {
    type Class = MyClass;
    type Record = MyRecord;
    fn classes() -> &'static [MyClass] { /* TGE's and Beacon */ }
    fn name(class: MyClass) -> &'static str { /* "Beacon", or TgeGhosts::name */ }
    fn class_of(record: &MyRecord) -> MyClass { /* ... */ }
    fn pack(class: MyClass, view: &dyn ObjectView, mask: u32, ctx: &PackCtx) -> (MyRecord, u32) { /* ... */ }
    fn decode(class: MyClass, r: &mut BitReader, table: &Table, charge: &mut ChargeFn, ctx: &ReadCtx)
        -> Result<MyRecord, NetError> { /* ... */ }
    fn encode(record: &MyRecord, w: &mut BitWriter, table: &Table, widths: NetWidths) -> R { /* ... */ }
}
```

The class ids on the wire come from the **net class table**, which both sides derive from the same list of class
representatives (`torque_net::classes::NetClasses::derive`, Torque's class-sorting rule). TGE's list is
`torque_ghosts::tge_reps`; a game appends its classes to it, so a new class shifts the ids the same way on both
sides. Datablocks work the same way through `DatablockCodecSet` and `TgeDatablocks`.

## Parent parts

Every GameBase-derived record starts with the GameBase part (scale and datablock), and every ShapeBase-derived record
with a ShapeBase part. The ShapeBase part is a seam, `torque_ghosts::parts::ShapeBasePart`: a game whose ShapeBase
block differs from TGE's implements the trait on its own part, and the class bodies that follow it (the mission markers,
for example) are generic over the part, so they are written once. A part declares `NEXT_FREE_MASK`, the first mask bit
the class after it may take.

## The client's world

On the client, torque-net's `GhostClient` reads a packet's ghost section through a `GhostCodec` (for TGE's records,
`torque_ghost_world::tge::TgeCodec`) and logs what changed. `ClientWorld<S>` takes those changes and keeps the world
a renderer and a game read: each ghost's class, its record with every update merged in, its pose, its datablock, its
mounted images (with TGE's image state machine), its script threads, and the effects the records started.

```rust,ignore
let codec = TgeCodec::new(class_names, widths, Table::torque());
let mut ghosts = GhostClient::new(codec, class_bits, widths.ghost_bits);
ghosts.enable_log();
let mut world: ClientWorld<TgeClientSet> = ClientWorld::default();

// For every server packet, after the head and the events:
ghosts.read_section(&mut reader)?;
let changes = ghosts.take_log();
world.set_clock(now_ms);
world.apply_ghosts(packet_number, &changes, &ghosts, compression_point, &mut ());
for (index, ghost) in &world.ghosts {
    println!("{index}: {:?} at {:?}", ghost.class, ghost.pose());
}
```

The world is generic over a `ClientSet`: a game's classes, records, datablocks, control-object data and the part it
keeps on each ghost. Where a game's client runs mounted images or script threads differently, its set overrides that one
method (`image_rules`, `apply_threads`) and keeps TGE's behaviour for the rest.

The [Server and client](server-and-client.md) chapter puts these pieces on a real connection.
