//! What goes on the wire: the rope's datablock record ([`RopeDataRecord`]), the ghost record ([`RopeGhost`]) and the field types it
//! is made of. Each group of the ghost is one `Option` with its own mask bit, so an update sends only the groups that changed.

use torque_ghosts::ObjectView;
use torque_ghosts::codec::{Codec, F32, R};
use torque_ghosts::plugin::{DatablockRef, FieldSpec, NetField};
use torque_ghosts::{Link, Value};
use torque_net::NetError;

use crate::data::{RopeLook, parse_color};
use crate::driver::MAX_DRIVER_NAME_BYTES;
use crate::sim::{Curve, Tangent};

/// The mask bit of the first end's anchor.
pub const MASK_START: u32 = 1 << 0;
/// The mask bit of the far end's anchor.
pub const MASK_END: u32 = 1 << 1;
/// The mask bit of the length and its reel.
pub const MASK_LENGTH: u32 = 1 << 2;
/// The mask bit of the rope's own look ([`RopeLook`]).
pub const MASK_LOOK: u32 = 1 << 3;
/// The mask bit of the driven curve.
pub const MASK_CURVE: u32 = 1 << 4;
/// The mask bit of the lifetime.
pub const MASK_LIFE: u32 = 1 << 5;
/// The mask bit of what a new ghost is told once: the predict id.
pub const MASK_INIT: u32 = 1 << 6;
/// The mask bit of the driver the rope follows.
pub const MASK_DRIVER: u32 = 1 << 7;

/// The longest node name an anchor carries.
pub const MAX_NODE_BYTES: usize = 24;
/// The longest texture file a look carries.
pub const MAX_TEXTURE_BYTES: usize = 96;

/// `bytes` as text of at most `max` bytes: a byte that is not UTF-8 reads as U+FFFD (which is wider), and the text is cut at a
/// character.
fn text_of(bytes: &[u8], max: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

// ANCHOR: datablock
/// `RopeData` as it travels: the same console fields as the class, by the same names (a test keeps the two in step). The texture is
/// text here, as the class's `Filename` is.
#[derive(Debug, Clone, PartialEq, Default, torque_engine::DatablockCodec)]
pub struct RopeDataRecord {
    #[net(bits = 8)]
    segments: i32,
    length: f32,
    #[net(max_len = 8)]
    style: String,
    thickness: f32,
    stiffness: f32,
    damping: f32,
    gravity: f32,
    color: [u8; 4],
    #[net(bits = 2)]
    #[net(name = "castShadow")]
    cast_shadow: i32,
    #[net(bits = 2)]
    #[net(name = "receiveShadow")]
    receive_shadow: i32,
    emissive: bool,
    #[net(max_len = 8)]
    sim: String,
    #[net(name = "emissiveIntensity")]
    emissive_intensity: f32,
    additive: bool,
    #[net(name = "endThickness")]
    end_thickness: f32,
    #[net(max_len = 24)]
    #[net(name = "colorEnd")]
    color_end: String,
    #[net(max_len = 96)]
    texture: String,
    #[net(name = "textureLength")]
    texture_length: f32,
    #[net(name = "textureScroll")]
    texture_scroll: f32,
    #[net(max_len = 8)]
    #[net(name = "textureFilter")]
    texture_filter: String,
}
// ANCHOR_END: datablock

/// What holds one end of a networked rope.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum AnchorWire {
    /// Nothing: the end falls with the rest.
    #[default]
    Free,
    /// A point in the world.
    Point([f32; 3]),
    /// A ghost on the connection, and the place on it.
    Ghost {
        /// The ghost's index.
        ghost: u32,
        /// The node (or `"muzzle"`); empty for the origin.
        node: String,
        /// The point held, as an offset from the node in the ghost's own frame (zero: the node itself).
        offset: [f32; 3],
    },
    /// A ghost the server has not made for this client yet: sent as free, and sent again once it has.
    Waiting,
}

impl NetField for AnchorWire {
    const MIN_BITS: usize = 2;
    const COMPOUND: bool = true;

    fn layout<C: Codec>(&mut self, c: &mut C, name: &'static str, _spec: FieldSpec) -> R {
        let mut kind = match self {
            AnchorWire::Free | AnchorWire::Waiting => 0,
            AnchorWire::Point(_) => 1,
            AnchorWire::Ghost { .. } => 2,
        };
        c.int(name, &mut kind, 2)?;
        match kind {
            0 => *self = AnchorWire::Free,
            1 => {
                let mut at = match self {
                    AnchorWire::Point(at) => at.map(F32::new),
                    _ => [F32::default(); 3],
                };
                c.floats(name, &mut at)?;
                *self = AnchorWire::Point(at.map(F32::get));
            }
            2 => {
                let (mut ghost, mut node, offset) = match self {
                    AnchorWire::Ghost {
                        ghost,
                        node,
                        offset,
                    } => (*ghost, node.clone().into_bytes(), *offset),
                    _ => (0, Vec::new(), [0.0; 3]),
                };
                node.truncate(MAX_NODE_BYTES);
                let bits = c.ghost_bits();
                c.int(name, &mut ghost, bits)?;
                c.string(name, &mut node)?;
                if node.len() > MAX_NODE_BYTES {
                    return Err(NetError::Invalid("an anchor's node is too long"));
                }
                let mut offset = offset.map(F32::new);
                let mut held = offset.iter().any(|v| v.get() != 0.0);
                c.flag(name, &mut held)?;
                if held {
                    c.floats(name, &mut offset)?;
                } else {
                    offset = [F32::default(); 3];
                }
                *self = AnchorWire::Ghost {
                    ghost,
                    node: text_of(&node, MAX_NODE_BYTES),
                    offset: offset.map(|v| crate::sim::coord(v.get())),
                };
            }
            _ => return Err(NetError::Invalid("an anchor's kind")),
        }
        Ok(())
    }

    /// The server rope's console fields `<name>Mode` (0 free, 1 point, 2 object), `<name>Point`, `<name>Object` (an object's id),
    /// `<name>Node` and `<name>Offset`.
    fn from_view(view: &dyn ObjectView, name: &str, _index: usize, _spec: FieldSpec) -> AnchorWire {
        let mode = torque_ghosts::int(view, &format!("{name}Mode"), 0);
        match mode {
            1 => AnchorWire::Point(std::array::from_fn(|k| {
                torque_ghosts::fld(view, &format!("{name}Point"), k)
            })),
            2 => match view.link(&format!("{name}Object")) {
                Link::Ghost(ghost) => {
                    let node = torque_ghosts::text(view, &format!("{name}Node"), 0);
                    AnchorWire::Ghost {
                        ghost,
                        node: text_of(&node, MAX_NODE_BYTES),
                        offset: std::array::from_fn(|k| {
                            crate::sim::coord(torque_ghosts::fld(view, &format!("{name}Offset"), k))
                        }),
                    }
                }
                Link::NotGhosted => AnchorWire::Waiting,
                Link::Null => AnchorWire::Free,
            },
            _ => AnchorWire::Free,
        }
    }

    fn pending(&self) -> bool {
        *self == AnchorWire::Waiting
    }

    fn values(&self, out: &mut Vec<Value>) {
        out.push(Value::Str(
            match self {
                AnchorWire::Free | AnchorWire::Waiting => "free".to_string(),
                AnchorWire::Point([x, y, z]) => format!("{x} {y} {z}"),
                AnchorWire::Ghost {
                    ghost,
                    node,
                    offset: [x, y, z],
                } if [x, y, z].iter().any(|v| **v != 0.0) => {
                    format!("ghost {ghost} {node} {x} {y} {z}")
                }
                AnchorWire::Ghost { ghost, node, .. } => format!("ghost {ghost} {node}"),
            }
            .into_bytes(),
        ));
    }
}

/// A reel: the length the rope is going to, and the seconds it has to get there (0: at once). 0 for the length is no say about it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LengthWire {
    /// The length it goes to; 0 for none.
    pub target: f32,
    /// The time left to reach it.
    pub seconds: f32,
}

impl NetField for LengthWire {
    const MIN_BITS: usize = 64;

    fn layout<C: Codec>(&mut self, c: &mut C, name: &'static str, _spec: FieldSpec) -> R {
        let mut raw = [self.target, self.seconds].map(F32::new);
        c.floats(name, &mut raw)?;
        *self = LengthWire {
            target: raw[0].get(),
            seconds: raw[1].get(),
        };
        Ok(())
    }

    /// The server rope's `length` (the target), `lengthReel` (the seconds the reel was given) and `lengthAt` (the server's time it
    /// was set, in ms): what is left of the reel now.
    fn from_view(view: &dyn ObjectView, name: &str, _index: usize, _spec: FieldSpec) -> LengthWire {
        let target = torque_ghosts::fld(view, name, 0);
        let reel = torque_ghosts::fld(view, &format!("{name}Reel"), 0);
        // The times are the server's clock in milliseconds, kept as the bits of an `i32` and compared by their difference, so a
        // server that has run past what an `i32` holds (24.8 days) still gets the time since right.
        let at = torque_ghosts::int(view, &format!("{name}At"), 0) as u32;
        let now = torque_ghosts::int(view, "@now", 0) as u32;
        let since = f64::from(now.wrapping_sub(at) as i32);
        let left = f64::from(reel) - since / 1000.0;
        LengthWire {
            target,
            seconds: if left > 0.0 { left as f32 } else { 0.0 },
        }
    }

    fn values(&self, out: &mut Vec<Value>) {
        out.push(Value::F32(self.target));
    }
}

/// A rope's own look on the wire ([`RopeLook`]): each value with a flag for whether the rope has its own.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LookWire(pub RopeLook);

impl NetField for LookWire {
    const MIN_BITS: usize = 4;
    const COMPOUND: bool = true;

    fn layout<C: Codec>(&mut self, c: &mut C, name: &'static str, _spec: FieldSpec) -> R {
        let look = &mut self.0;
        let mut has = look.color.is_some();
        c.flag(name, &mut has)?;
        if has {
            let mut color = look.color.unwrap_or_default();
            for byte in &mut color {
                let mut v = u32::from(*byte);
                c.int(name, &mut v, 8)?;
                *byte = (v & 0xff) as u8;
            }
            look.color = Some(color);
        } else {
            look.color = None;
        }
        let mut has = look.texture.is_some();
        c.flag(name, &mut has)?;
        if has {
            let mut text = look.texture.take().unwrap_or_default().into_bytes();
            text.truncate(MAX_TEXTURE_BYTES);
            c.string(name, &mut text)?;
            if text.len() > MAX_TEXTURE_BYTES {
                return Err(NetError::Invalid("a rope's texture file is too long"));
            }
            look.texture = Some(text_of(&text, MAX_TEXTURE_BYTES));
        } else {
            look.texture = None;
        }
        let mut has = look.scroll.is_some();
        c.flag(name, &mut has)?;
        if has {
            let mut bits = look.scroll.unwrap_or(0.0).to_bits();
            c.float32(name, &mut bits)?;
            look.scroll = Some(f32::from_bits(bits));
        } else {
            look.scroll = None;
        }
        let mut has = look.emissive.is_some();
        c.flag(name, &mut has)?;
        if has {
            let mut on = look.emissive.unwrap_or(false);
            c.flag(name, &mut on)?;
            look.emissive = Some(on);
        } else {
            look.emissive = None;
        }
        Ok(())
    }

    /// The server rope's `lookColor` ("r g b a", empty for none), `lookTexture` (empty for none), `lookScroll` (a number, empty
    /// for none) and `lookEmissive` (-1 for none, else 0 or 1).
    fn from_view(view: &dyn ObjectView, name: &str, _index: usize, _spec: FieldSpec) -> LookWire {
        let text = |field: &str| {
            String::from_utf8_lossy(&torque_ghosts::text(view, &format!("{name}{field}"), 0))
                .trim()
                .to_string()
        };
        let texture = text_of(text("Texture").as_bytes(), MAX_TEXTURE_BYTES);
        LookWire(RopeLook {
            color: parse_color(&text("Color")),
            texture: (!texture.is_empty()).then_some(texture),
            scroll: text("Scroll").parse::<f32>().ok().filter(|s| s.is_finite()),
            emissive: match torque_ghosts::int(view, &format!("{name}Emissive"), 0) {
                0 => Some(false),
                n if n > 0 => Some(true),
                _ => None,
            },
        })
    }

    fn values(&self, out: &mut Vec<Value>) {
        out.push(Value::Str(format!("{:?}", self.0).into_bytes()));
    }
}

/// The driven curve of a rope on the wire; none is a rope that is not given one.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CurveWire(pub Option<Curve>);

fn layout_tangent<C: Codec>(c: &mut C, name: &'static str, t: &mut Tangent) -> R {
    let mut kind = match t {
        Tangent::Auto => 0,
        Tangent::Mount => 1,
        Tangent::Fixed(_) => 2,
    };
    c.int(name, &mut kind, 2)?;
    match kind {
        0 => *t = Tangent::Auto,
        1 => *t = Tangent::Mount,
        2 => {
            let mut dir = match t {
                Tangent::Fixed(d) => d.map(F32::new),
                _ => [F32::default(); 3],
            };
            c.floats(name, &mut dir)?;
            *t = Tangent::Fixed(dir.map(F32::get));
        }
        _ => return Err(NetError::Invalid("a curve's tangent kind")),
    }
    Ok(())
}

impl NetField for CurveWire {
    const MIN_BITS: usize = 1;
    const COMPOUND: bool = true;

    fn layout<C: Codec>(&mut self, c: &mut C, name: &'static str, _spec: FieldSpec) -> R {
        let mut on = self.0.is_some();
        c.flag(name, &mut on)?;
        if !on {
            self.0 = None;
            return Ok(());
        }
        let mut curve = self.0.unwrap_or(Curve {
            start: Tangent::Auto,
            end: Tangent::Auto,
            bend: 0.0,
        });
        layout_tangent(c, name, &mut curve.start)?;
        layout_tangent(c, name, &mut curve.end)?;
        let mut bits = curve.bend.to_bits();
        c.float32(name, &mut bits)?;
        curve.bend = f32::from_bits(bits);
        self.0 = Some(curve);
        Ok(())
    }

    /// The server rope's `curveMode` (1 for a curve), `curveStart` and `curveEnd` ("" toward the other end, "mount", or "x y z")
    /// and `curveBend`.
    fn from_view(view: &dyn ObjectView, name: &str, _index: usize, _spec: FieldSpec) -> CurveWire {
        if torque_ghosts::int(view, &format!("{name}Mode"), 0) != 1 {
            return CurveWire(None);
        }
        let tangent = |field: &str| {
            let text = torque_ghosts::text(view, &format!("{name}{field}"), 0);
            parse_tangent(&String::from_utf8_lossy(&text))
        };
        CurveWire(Some(Curve {
            start: tangent("Start"),
            end: tangent("End"),
            bend: torque_ghosts::fld(view, &format!("{name}Bend"), 0),
        }))
    }

    fn values(&self, out: &mut Vec<Value>) {
        out.push(Value::F32(self.0.map_or(0.0, |c| c.bend)));
    }
}

/// `""` (toward the other end), `"mount"` or `"x y z"` as a tangent; anything else is toward the other end.
pub fn parse_tangent(text: &str) -> Tangent {
    let text = text.trim();
    if text.eq_ignore_ascii_case("mount") {
        return Tangent::Mount;
    }
    let words: Vec<f32> = text
        .split_ascii_whitespace()
        .filter_map(|w| w.parse().ok())
        .collect();
    match words[..] {
        [x, y, z] => Tangent::fixed([x, y, z]).unwrap_or(Tangent::Auto),
        _ => Tangent::Auto,
    }
}

/// How long a temporary rope has left when the ghost is sent, and the seconds at its end it fades over. 0 for `left` is a rope
/// that lasts.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LifeWire {
    /// The seconds left.
    pub left: f32,
    /// The fade at the end.
    pub fade: f32,
}

impl NetField for LifeWire {
    const MIN_BITS: usize = 64;
    const COMPOUND: bool = true;

    fn layout<C: Codec>(&mut self, c: &mut C, name: &'static str, _spec: FieldSpec) -> R {
        let mut raw = [self.left, self.fade].map(F32::new);
        c.floats(name, &mut raw)?;
        *self = LifeWire {
            left: raw[0].get(),
            fade: raw[1].get(),
        };
        Ok(())
    }

    /// The server rope's `lifeEnd` (the server's time it ends, in ms; 0 for none) and `lifeFade`.
    fn from_view(view: &dyn ObjectView, name: &str, _index: usize, _spec: FieldSpec) -> LifeWire {
        let end = torque_ghosts::int(view, &format!("{name}End"), 0);
        let fade = torque_ghosts::fld(view, &format!("{name}Fade"), 0);
        // 0 is no end; the clock's milliseconds are kept as the bits of an `i32` and compared by their difference (see
        // `LengthWire`).
        if end == 0 {
            return LifeWire::default();
        }
        let now = torque_ghosts::int(view, "@now", 0) as u32;
        let left_ms = f64::from((end as u32).wrapping_sub(now) as i32);
        // The rope is deleted at its end; a ghost still being sent after it has the least time left.
        LifeWire {
            left: (left_ms / 1000.0).max(0.001) as f32,
            fade,
        }
    }

    fn values(&self, out: &mut Vec<Value>) {
        out.push(Value::F32(self.left));
    }
}

/// The id a client gave its own predicted rope, as the owner's ghost carries it (0: none, and what everyone else is sent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PredictWire(pub u8);

impl NetField for PredictWire {
    const MIN_BITS: usize = 8;
    const COMPOUND: bool = true;

    fn layout<C: Codec>(&mut self, c: &mut C, name: &'static str, _spec: FieldSpec) -> R {
        let mut v = u32::from(self.0);
        c.int(name, &mut v, 8)?;
        self.0 = (v & 0xff) as u8;
        Ok(())
    }

    /// The server rope's `predict` and its `predictOwner` (a connection's object id): the id for the connection that owns the
    /// rope, 0 for any other.
    fn from_view(
        view: &dyn ObjectView,
        name: &str,
        _index: usize,
        _spec: FieldSpec,
    ) -> PredictWire {
        let id = torque_ghosts::int(view, name, 0);
        let owner = torque_ghosts::int(view, &format!("{name}Owner"), 0);
        let here = torque_ghosts::id(view, "@connection", 0);
        match (u8::try_from(id), here) {
            (Ok(id), Some(here)) if owner > 0 && i64::from(here) == owner => PredictWire(id),
            _ => PredictWire(0),
        }
    }

    fn values(&self, out: &mut Vec<Value>) {
        out.push(Value::Int(i64::from(self.0)));
    }
}

/// The driver a networked rope follows ([`crate::driver`]): its name, and the client's ghost of the object the server named (the key
/// the driver is asked with).
#[derive(Debug, Clone, PartialEq, Default)]
pub enum DriverWire {
    /// None: the rope follows what it was told.
    #[default]
    Off,
    /// The driver registered under `name`, asked about the ghost.
    On {
        /// The driver's name.
        name: String,
        /// The ghost's index.
        ghost: u32,
    },
    /// A ghost the server has not made for this client yet: sent as off, and sent again once it has.
    Waiting,
}

impl NetField for DriverWire {
    const MIN_BITS: usize = 1;
    const COMPOUND: bool = true;

    fn layout<C: Codec>(&mut self, c: &mut C, name: &'static str, _spec: FieldSpec) -> R {
        let mut on = matches!(self, DriverWire::On { .. });
        c.flag(name, &mut on)?;
        if !on {
            *self = DriverWire::Off;
            return Ok(());
        }
        let (mut ghost, mut driver) = match self {
            DriverWire::On { name, ghost } => (*ghost, name.clone().into_bytes()),
            _ => (0, Vec::new()),
        };
        driver.truncate(MAX_DRIVER_NAME_BYTES);
        let bits = c.ghost_bits();
        c.int(name, &mut ghost, bits)?;
        c.string(name, &mut driver)?;
        if driver.len() > MAX_DRIVER_NAME_BYTES {
            return Err(NetError::Invalid("a driver's name is too long"));
        }
        *self = DriverWire::On {
            name: text_of(&driver, MAX_DRIVER_NAME_BYTES),
            ghost,
        };
        Ok(())
    }

    /// The server rope's console fields `<name>Name` (empty for none) and `<name>Object` (an object's id).
    fn from_view(view: &dyn ObjectView, name: &str, _index: usize, _spec: FieldSpec) -> DriverWire {
        let driver = torque_ghosts::text(view, &format!("{name}Name"), 0);
        let driver = text_of(&driver, MAX_DRIVER_NAME_BYTES);
        if driver.is_empty() {
            return DriverWire::Off;
        }
        match view.link(&format!("{name}Object")) {
            Link::Ghost(ghost) => DriverWire::On {
                name: driver,
                ghost,
            },
            Link::NotGhosted => DriverWire::Waiting,
            Link::Null => DriverWire::Off,
        }
    }

    fn pending(&self) -> bool {
        *self == DriverWire::Waiting
    }

    fn values(&self, out: &mut Vec<Value>) {
        out.push(Value::Str(
            match self {
                DriverWire::Off | DriverWire::Waiting => "off".to_string(),
                DriverWire::On { name, ghost } => format!("{name} {ghost}"),
            }
            .into_bytes(),
        ));
    }
}

// ANCHOR: ghost
/// What a client knows of a networked rope. A group is `None` until the server sends it (an update sends only the groups whose
/// mask bit is set; a new ghost has them all).
#[derive(Debug, Clone, PartialEq, Default, torque_engine::GhostRecord)]
pub struct RopeGhost {
    /// The `RopeData` datablock.
    pub data_block: DatablockRef,
    /// The id the owner's client gave its predicted rope.
    #[net(mask = MASK_INIT)]
    pub predict: Option<PredictWire>,
    /// What holds the start.
    #[net(mask = MASK_START)]
    pub start: Option<AnchorWire>,
    /// What holds the far end.
    #[net(mask = MASK_END)]
    pub end: Option<AnchorWire>,
    /// The length and its reel.
    #[net(mask = MASK_LENGTH)]
    pub length: Option<LengthWire>,
    /// The rope's own color, texture, scroll and glow.
    #[net(mask = MASK_LOOK)]
    pub look: Option<LookWire>,
    /// The driven curve.
    #[net(mask = MASK_CURVE)]
    pub curve: Option<CurveWire>,
    /// The lifetime.
    #[net(mask = MASK_LIFE)]
    pub life: Option<LifeWire>,
    /// The driver the rope follows.
    #[net(mask = MASK_DRIVER)]
    pub driver: Option<DriverWire>,
}
// ANCHOR_END: ghost

#[cfg(test)]
#[path = "records_tests.rs"]
mod tests;
