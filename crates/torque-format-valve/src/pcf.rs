//! PCF particle files: particle system definitions stored as a binary DMX document (encoding `binary 2`, the one these games
//! ship; later encodings are refused), and a small simulation of the commonest of their functions.
//!
//! **Layout** (binary 2): a text header ending in a NUL (`<!-- dmx encoding binary 2 format pcf 1 -->`), a string table (a
//! `u16` count and NUL-terminated strings), the elements (an `i32` count; each a type name from the table, its own name inline
//! and a 16-byte id), then each element's attributes (an `i32` count; each a name from the table, a type byte and its value:
//! an element index, `i32`, `f32`, a bool byte, an inline string, length-prefixed bytes, an id, an RGBA color, 2 to 4 floats, a
//! 4 x 4 matrix, or an `i32`-counted array of one of those). A system definition (`DmeParticleSystemDefinition`) names its
//! material, its particle count and default radius and color, and lists its emitters, initializers, operators, renderers
//! and children, each an element whose `functionName` says what it does and whose other attributes are its settings.
//!
//! [`Sim`] runs a system's emitters, initializers and operators that it knows (continuous and instant emission; random
//! lifetime, radius, color, alpha, rotation, sprite sequence, position in a sphere with its speed, velocity; basic movement
//! with gravity and drag, lifespan decay, alpha fades, radius scale, color fade, rotation) and leaves the others out: an
//! approximation of the effect, enough to see its sprites, colors and motion. Every count and index is checked.

use std::rc::Rc;

use crate::read::{Cursor, Error, Result, array, cap, f32_at, i32_at, invalid, slice};

/// The largest particle file read.
pub const MAX_FILE_BYTES: u64 = 32 << 20;
/// The most particles one simulated system keeps.
pub const MAX_PARTICLES: usize = 4096;
const MAX_STRING: usize = 4096;
/// The most functions of one kind (emitters, initializers, operators, renderers) a system keeps; a function listed twice
/// counts once.
pub const MAX_FUNCTIONS: usize = 256;
/// The highest sprite sequence a particle takes.
pub const MAX_SEQUENCE: i32 = 1024;
/// The most attributes one element has.
pub const MAX_ATTRS: usize = 1024;

/// An attribute's value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Element(Option<usize>),
    Int(i32),
    Float(f32),
    Bool(bool),
    Str(String),
    Bytes(Vec<u8>),
    Id([u8; 16]),
    Color([u8; 4]),
    Floats(Vec<f32>),
    Array(Vec<Value>),
}

/// An element: its type, its name and its attributes in order. Type and attribute names are the file's string table's,
/// shared (a file that names one long string many times holds it once).
#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    pub kind: Rc<str>,
    pub name: String,
    pub attrs: Vec<(Rc<str>, Value)>,
}

impl Element {
    /// An attribute's value.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.attrs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
    }

    pub fn float(&self, name: &str) -> Option<f32> {
        match self.get(name)? {
            Value::Float(f) => Some(*f),
            Value::Int(i) => Some(*i as f32),
            _ => None,
        }
    }

    pub fn int(&self, name: &str) -> Option<i32> {
        match self.get(name)? {
            Value::Int(i) => Some(*i),
            Value::Float(f) => Some(*f as i32),
            _ => None,
        }
    }

    pub fn bool(&self, name: &str) -> Option<bool> {
        match self.get(name)? {
            Value::Bool(b) => Some(*b),
            Value::Int(i) => Some(*i != 0),
            _ => None,
        }
    }

    pub fn str(&self, name: &str) -> Option<&str> {
        match self.get(name)? {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn color(&self, name: &str) -> Option<[u8; 4]> {
        match self.get(name)? {
            Value::Color(c) => Some(*c),
            _ => None,
        }
    }

    pub fn vec3(&self, name: &str) -> Option<[f32; 3]> {
        match self.get(name)? {
            Value::Floats(v) if v.len() >= 3 => Some([v[0], v[1], v[2]]),
            _ => None,
        }
    }

    /// The elements an element-array attribute lists.
    pub fn elements(&self, name: &str) -> Vec<usize> {
        match self.get(name) {
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|v| match v {
                    Value::Element(Some(i)) => Some(*i),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }
}

/// A particle file's elements.
#[derive(Debug, Clone, PartialEq)]
pub struct Pcf {
    /// Shared, so a simulation keeps its functions without copying them.
    pub elements: Vec<Rc<Element>>,
}

/// One function of a system (an emitter, initializer, operator or renderer): its name and its element.
#[derive(Debug, Clone, PartialEq)]
pub struct Function<'a> {
    pub name: &'a str,
    pub element: &'a Rc<Element>,
}

/// A system definition with its functions resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct System<'a> {
    pub name: &'a str,
    pub element: &'a Element,
    pub emitters: Vec<Function<'a>>,
    pub initializers: Vec<Function<'a>>,
    pub operators: Vec<Function<'a>>,
    pub renderers: Vec<Function<'a>>,
    /// The names of its child systems.
    pub children: Vec<&'a str>,
}

impl<'a> System<'a> {
    pub fn material(&self) -> Option<&'a str> {
        self.element.str("material")
    }
}

/// The fixed size of a value of `ty` (None: a string, bytes or an array).
fn value_size(ty: u8) -> Option<usize> {
    Some(match ty {
        1..=3 | 8 => 4,
        4 => 1,
        7 => 16,
        9 => 8,
        10 | 12 => 12,
        11 | 13 => 16,
        14 => 64,
        _ => return None,
    })
}

fn read_value(c: &mut Cursor<'_>, ty: u8, elements: usize, budget: &mut usize) -> Result<Value> {
    let take = |budget: &mut usize, n: usize| -> Result<()> {
        *budget = budget.checked_sub(n).ok_or(Error::TooLarge {
            what: "the particle file's values",
            asked: n as u64,
            cap: *budget as u64,
        })?;
        Ok(())
    };
    take(budget, 1)?;
    Ok(match ty {
        1 => {
            let i = c.u32("an element reference")? as i32;
            if i == -2 {
                // An element in another file: its id as a string follows.
                c.cstr(MAX_STRING, "an external element's id")?;
            }
            Value::Element(usize::try_from(i).ok().filter(|&i| i < elements))
        }
        2 => Value::Int(c.u32("an int")? as i32),
        3 => Value::Float(f32::from_le_bytes(array::<4>(
            c.bytes(4, "a float")?,
            0,
            "a float",
        )?)),
        4 => Value::Bool(c.bytes(1, "a bool")?[0] != 0),
        5 => {
            let s = c.cstr(MAX_STRING, "a string")?;
            take(budget, s.len())?;
            Value::Str(String::from_utf8_lossy(s).into_owned())
        }
        6 => {
            let n = c.u32("a byte value")? as usize;
            take(budget, n)?;
            Value::Bytes(c.bytes(n, "a byte value")?.to_vec())
        }
        7 => Value::Id(array::<16>(c.bytes(16, "an id")?, 0, "an id")?),
        8 => Value::Color(array::<4>(c.bytes(4, "a color")?, 0, "a color")?),
        9..=14 => {
            let n = value_size(ty).unwrap_or(0) / 4;
            take(budget, n)?;
            let b = c.bytes(n * 4, "floats")?;
            Value::Floats(
                (0..n)
                    .map(|k| f32_at(b, k * 4, "floats"))
                    .collect::<Result<_>>()?,
            )
        }
        15..=28 => {
            let n = c.u32("an array")? as usize;
            let item = ty - 14;
            // Every item takes at least a byte; refuse counts the bytes left cannot hold.
            let min = value_size(item).unwrap_or(1);
            if n.checked_mul(min)
                .is_none_or(|b| b > c.b.len().saturating_sub(c.pos))
            {
                return Err(Error::Truncated { what: "an array" });
            }
            let mut out = Vec::with_capacity(n);
            for _ in 0..n {
                out.push(read_value(c, item, elements, budget)?);
            }
            Value::Array(out)
        }
        t => return invalid(format!("attribute type {t} is not read")),
    })
}

impl Pcf {
    /// Reads a particle file.
    pub fn parse(b: &[u8]) -> Result<Pcf> {
        cap("the particle file's size", b.len() as u64, MAX_FILE_BYTES)?;
        let head_end = b
            .iter()
            .take(256)
            .position(|&c| c == 0)
            .ok_or(Error::Invalid("not a DMX file (no header)".into()))?;
        let head = String::from_utf8_lossy(&b[..head_end]);
        if !head.starts_with("<!-- dmx encoding binary ") {
            return invalid("not a binary DMX file");
        }
        if !head.starts_with("<!-- dmx encoding binary 2 ") {
            return invalid(format!("DMX encoding {} is not read", head.trim()));
        }
        let mut c = Cursor::new(b);
        c.pos = head_end + 1;
        let n = usize::from(c.u16("the string table")?);
        let mut strings = Vec::with_capacity(n);
        for _ in 0..n {
            strings.push(Rc::<str>::from(String::from_utf8_lossy(
                c.cstr(MAX_STRING, "a string")?,
            )));
        }
        let ne = c.u32("the elements")? as usize;
        // Each element takes at least 19 bytes (a type, an empty name, an id).
        if ne
            .checked_mul(19)
            .is_none_or(|need| need > c.b.len().saturating_sub(c.pos))
        {
            return Err(Error::Truncated {
                what: "the elements",
            });
        }
        let string = |i: u16| -> Result<Rc<str>> {
            strings
                .get(usize::from(i))
                .cloned()
                .ok_or(Error::Invalid(format!("string {i} past the table")))
        };
        let mut elements: Vec<Element> = Vec::with_capacity(ne);
        for _ in 0..ne {
            let kind = string(c.u16("an element")?)?;
            let name = String::from_utf8_lossy(c.cstr(MAX_STRING, "an element name")?).into_owned();
            c.bytes(16, "an element id")?;
            elements.push(Element {
                kind,
                name,
                attrs: Vec::new(),
            });
        }
        // What the values may make, all elements together: about the file's size.
        let mut budget = b.len() + 1024;
        for element in &mut elements {
            let na = c.u32("an element's attributes")? as usize;
            cap("an element's attributes", na as u64, MAX_ATTRS as u64)?;
            if na
                .checked_mul(4)
                .is_none_or(|need| need > c.b.len().saturating_sub(c.pos))
            {
                return Err(Error::Truncated {
                    what: "an element's attributes",
                });
            }
            let mut attrs = Vec::with_capacity(na);
            for _ in 0..na {
                // Each entry is paid for (its name is shared, not copied).
                budget = budget.checked_sub(1).ok_or(Error::TooLarge {
                    what: "the particle file's attributes",
                    asked: 1,
                    cap: 0,
                })?;
                let name = string(c.u16("an attribute")?)?;
                let ty = c.bytes(1, "an attribute type")?[0];
                attrs.push((name, read_value(&mut c, ty, ne, &mut budget)?));
            }
            element.attrs = attrs;
        }
        Ok(Pcf {
            elements: elements.into_iter().map(Rc::new).collect(),
        })
    }

    /// The system definitions, their functions resolved (references that are not elements dropped).
    pub fn systems(&self) -> Vec<System<'_>> {
        // Each function once (a list may name one many times), at most MAX_FUNCTIONS a list.
        let funcs = |e: &Element, list: &str| -> Vec<Function<'_>> {
            let mut seen = std::collections::HashSet::new();
            e.elements(list)
                .into_iter()
                .filter(|&i| seen.insert(i))
                .filter_map(|i| self.elements.get(i))
                .take(MAX_FUNCTIONS)
                .map(|f| Function {
                    name: f.str("functionName").unwrap_or(&f.name),
                    element: f,
                })
                .collect()
        };
        self.elements
            .iter()
            .filter(|e| &*e.kind == "DmeParticleSystemDefinition")
            .map(|e| System {
                name: &e.name,
                element: e,
                emitters: funcs(e, "emitters"),
                initializers: funcs(e, "initializers"),
                operators: funcs(e, "operators"),
                renderers: funcs(e, "renderers"),
                children: e
                    .elements("children")
                    .into_iter()
                    .filter_map(|i| self.elements.get(i))
                    .filter_map(|child| match child.get("child") {
                        Some(Value::Element(Some(k))) => self.elements.get(*k),
                        _ => None,
                    })
                    .map(|s| s.name.as_str())
                    .collect(),
            })
            .collect()
    }

    /// The system named `name` (any case).
    pub fn system(&self, name: &str) -> Option<System<'_>> {
        self.systems()
            .into_iter()
            .find(|s| s.name.eq_ignore_ascii_case(name))
    }
}

/// A simulated particle (in the system's units, around its origin).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Particle {
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub radius: f32,
    pub base_radius: f32,
    /// RGBA, 0..1.
    pub color: [f32; 4],
    pub base_color: [f32; 3],
    pub base_alpha: f32,
    pub rotation: f32,
    pub spin: f32,
    pub sequence: i32,
    pub age: f32,
    pub lifetime: f32,
}

/// An emitter the simulation runs, its settings read once.
#[derive(Debug, Clone)]
enum Emitter {
    Continuous { start: f32, end: f32, rate: f32 },
    Instant { start: f32, count: f32 },
}

/// An initializer the simulation runs, its settings read once.
#[derive(Debug, Clone)]
enum Init {
    Lifetime(f32, f32),
    Radius(f32, f32),
    Color([u8; 4], [u8; 4]),
    Alpha(f32, f32),
    Rotation {
        initial: f32,
        lo: f32,
        hi: f32,
    },
    Spin {
        constant: f32,
        lo: f32,
        hi: f32,
        flip: bool,
    },
    Sequence(i32, i32),
    Sphere {
        distance: (f32, f32),
        speed: (f32, f32),
        lo: [f32; 3],
        hi: [f32; 3],
    },
    Velocity {
        speed: (f32, f32),
        lo: [f32; 3],
        hi: [f32; 3],
    },
    Offset([f32; 3], [f32; 3]),
}

/// An operator the simulation runs, its settings read once.
#[derive(Debug, Clone)]
enum Op {
    Movement {
        gravity: [f32; 3],
        drag: f32,
    },
    Rotation,
    AlphaFade {
        from: f32,
        to: f32,
        fade_in: (f32, f32),
        fade_out: (f32, f32),
    },
    AlphaRandom {
        out: bool,
        time: f32,
    },
    RadiusScale {
        from: f32,
        to: f32,
        start: f32,
        end: f32,
    },
    ColorFade {
        color: [u8; 4],
        start: f32,
        end: f32,
    },
}

impl Emitter {
    fn read(name: &str, e: &Element) -> Option<Emitter> {
        let start = e.float("emission_start_time").unwrap_or(0.0);
        Some(match name {
            "emit_continuously" => {
                let dur = e.float("emission_duration").unwrap_or(0.0);
                Emitter::Continuous {
                    start,
                    end: if dur > 0.0 {
                        start + dur
                    } else {
                        f32::INFINITY
                    },
                    rate: e.float("emission_rate").unwrap_or(0.0).clamp(0.0, 10_000.0),
                }
            }
            "emit_instantaneously" => Emitter::Instant {
                start,
                count: e
                    .int("num_to_emit")
                    .unwrap_or(0)
                    .clamp(0, MAX_PARTICLES as i32) as f32,
            },
            _ => return None,
        })
    }
}

impl Init {
    fn read(name: &str, e: &Element) -> Option<Init> {
        let f = |k: &str, d: f32| e.float(k).unwrap_or(d);
        let v = |k: &str| e.vec3(k).unwrap_or([0.0; 3]);
        Some(match name {
            "lifetime random" => Init::Lifetime(f("lifetime_min", 1.0), f("lifetime_max", 1.0)),
            "radius random" => Init::Radius(f("radius_min", 1.0), f("radius_max", 1.0)),
            "color random" => Init::Color(
                e.color("color1").unwrap_or([255; 4]),
                e.color("color2").unwrap_or([255; 4]),
            ),
            "alpha random" => Init::Alpha(f("alpha_min", 255.0), f("alpha_max", 255.0)),
            "rotation random" => Init::Rotation {
                initial: f("rotation_initial", 0.0),
                lo: f("rotation_offset_min", 0.0),
                hi: f("rotation_offset_max", 360.0),
            },
            "rotation speed random" => Init::Spin {
                constant: f("rotation_speed_constant", 0.0),
                lo: f("rotation_speed_random_min", 0.0),
                hi: f("rotation_speed_random_max", 360.0),
                flip: e.bool("randomly_flip_direction").unwrap_or(false),
            },
            "sequence random" => {
                let lo = e.int("sequence_min").unwrap_or(0).clamp(0, MAX_SEQUENCE);
                Init::Sequence(
                    lo,
                    e.int("sequence_max").unwrap_or(0).clamp(lo, MAX_SEQUENCE),
                )
            }
            "position within sphere random" => Init::Sphere {
                distance: (f("distance_min", 0.0), f("distance_max", 0.0)),
                speed: (f("speed_min", 0.0), f("speed_max", 0.0)),
                lo: v("speed_in_local_coordinate_system_min"),
                hi: v("speed_in_local_coordinate_system_max"),
            },
            "velocity random" => Init::Velocity {
                speed: (f("random_speed_min", 0.0), f("random_speed_max", 0.0)),
                lo: v("speed_in_local_coordinate_system_min"),
                hi: v("speed_in_local_coordinate_system_max"),
            },
            "position modify offset random" => Init::Offset(v("offset min"), v("offset max")),
            _ => return None,
        })
    }
}

impl Op {
    fn read(name: &str, e: &Element) -> Option<Op> {
        let f = |k: &str, d: f32| e.float(k).unwrap_or(d);
        Some(match name {
            "movement basic" => Op::Movement {
                gravity: e.vec3("gravity").unwrap_or([0.0; 3]),
                drag: f("drag", 0.0).clamp(0.0, 1.0),
            },
            "rotation basic" => Op::Rotation,
            "alpha fade and decay" => Op::AlphaFade {
                from: f("start_alpha", 1.0),
                to: f("end_alpha", 0.0),
                fade_in: (f("start_fade_in_time", 0.0), f("end_fade_in_time", 0.5)),
                fade_out: (f("start_fade_out_time", 0.5), f("end_fade_out_time", 1.0)),
            },
            "alpha fade in random" | "alpha fade out random" => Op::AlphaRandom {
                out: name.ends_with("out random"),
                time: e
                    .float("proportional fade in/out time min")
                    .or(e.float("proportional_fade_in_time_min"))
                    .unwrap_or(0.25)
                    .clamp(0.01, 1.0),
            },
            "radius scale" => Op::RadiusScale {
                from: f("radius_start_scale", 1.0),
                to: f("radius_end_scale", 1.0),
                start: f("start_time", 0.0),
                end: f("end_time", 1.0),
            },
            "color fade" => Op::ColorFade {
                color: e.color("color_fade").unwrap_or([255; 4]),
                start: f("fade_start_time", 0.0),
                end: f("fade_end_time", 1.0),
            },
            _ => return None,
        })
    }
}

/// A system's simulation (module docs). Its functions' settings are read once, when it is made.
#[derive(Debug, Clone)]
pub struct Sim {
    pub particles: Vec<Particle>,
    pub time: f32,
    emitted: f32,
    rng: u64,
    max: usize,
    radius: f32,
    color: [u8; 4],
    emitters: Vec<Emitter>,
    initializers: Vec<Init>,
    operators: Vec<Op>,
}

impl Sim {
    /// The simulation of `system`, seeded.
    pub fn new(system: &System<'_>, seed: u64) -> Sim {
        fn own<T>(fs: &[Function<'_>], read: fn(&str, &Element) -> Option<T>) -> Vec<T> {
            fs.iter()
                .filter_map(|f| read(&f.name.to_ascii_lowercase(), f.element))
                .collect()
        }
        Sim {
            particles: Vec::new(),
            time: 0.0,
            emitted: 0.0,
            rng: seed | 1,
            max: usize::try_from(system.element.int("max_particles").unwrap_or(1000))
                .unwrap_or(0)
                .min(MAX_PARTICLES),
            radius: system.element.float("radius").unwrap_or(5.0),
            color: system.element.color("color").unwrap_or([255; 4]),
            emitters: own(&system.emitters, Emitter::read),
            initializers: own(&system.initializers, Init::read),
            operators: own(&system.operators, Op::read),
        }
    }

    fn random(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        (x >> 40) as f32 / (1u64 << 24) as f32
    }

    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.random()
    }

    fn spawn(&mut self) {
        if self.particles.len() >= self.max {
            return;
        }
        let c = self.color;
        let mut p = Particle {
            position: [0.0; 3],
            velocity: [0.0; 3],
            radius: self.radius,
            base_radius: self.radius,
            color: [
                f32::from(c[0]) / 255.0,
                f32::from(c[1]) / 255.0,
                f32::from(c[2]) / 255.0,
                f32::from(c[3]) / 255.0,
            ],
            base_color: [0.0; 3],
            base_alpha: 1.0,
            rotation: 0.0,
            spin: 0.0,
            sequence: 0,
            age: 0.0,
            lifetime: 1.0,
        };
        let inits = std::mem::take(&mut self.initializers);
        for init in &inits {
            match *init {
                Init::Lifetime(lo, hi) => p.lifetime = self.range(lo, hi),
                Init::Radius(lo, hi) => p.radius = self.range(lo, hi),
                Init::Color(a, b) => {
                    let t = self.random();
                    for k in 0..3 {
                        p.color[k] *=
                            (f32::from(a[k]) + (f32::from(b[k]) - f32::from(a[k])) * t) / 255.0;
                    }
                }
                Init::Alpha(lo, hi) => {
                    let a = self.range(lo, hi);
                    p.color[3] *= a / 255.0;
                }
                Init::Rotation { initial, lo, hi } => {
                    p.rotation = (initial + self.range(lo, hi)).to_radians();
                }
                Init::Spin {
                    constant,
                    lo,
                    hi,
                    flip,
                } => {
                    let s = constant + self.range(lo, hi);
                    p.spin = s.to_radians()
                        * if flip && self.random() < 0.5 {
                            -1.0
                        } else {
                            1.0
                        };
                }
                Init::Sequence(lo, hi) => {
                    let pick = (f64::from(hi - lo + 1) * f64::from(self.random())) as i32;
                    p.sequence = lo.saturating_add(pick).min(hi);
                }
                Init::Sphere {
                    distance,
                    speed,
                    lo,
                    hi,
                } => {
                    let d = self.range(distance.0, distance.1);
                    let dir = self.unit();
                    p.position = [dir[0] * d, dir[1] * d, dir[2] * d];
                    let s = self.range(speed.0, speed.1);
                    for k in 0..3 {
                        p.velocity[k] += dir[k] * s + self.range(lo[k], hi[k]);
                    }
                }
                Init::Velocity { speed, lo, hi } => {
                    let s = self.range(speed.0, speed.1);
                    let dir = self.unit();
                    for k in 0..3 {
                        p.velocity[k] += dir[k] * s + self.range(lo[k], hi[k]);
                    }
                }
                Init::Offset(lo, hi) => {
                    for k in 0..3 {
                        p.position[k] += self.range(lo[k], hi[k]);
                    }
                }
            }
        }
        self.initializers = inits;
        p.base_radius = p.radius;
        p.base_color = [p.color[0], p.color[1], p.color[2]];
        p.base_alpha = p.color[3];
        self.particles.push(p);
    }

    fn unit(&mut self) -> [f32; 3] {
        let z = self.range(-1.0, 1.0);
        let a = self.range(0.0, std::f32::consts::TAU);
        let r = (1.0 - z * z).max(0.0).sqrt();
        [r * a.cos(), r * a.sin(), z]
    }

    /// Advances the simulation by `dt` seconds.
    pub fn step(&mut self, dt: f32) {
        let dt = dt.clamp(0.0, 0.1);
        let (t0, t1) = (self.time, self.time + dt);
        for e in &self.emitters {
            match *e {
                Emitter::Continuous { start, end, rate } => {
                    self.emitted += (t1.min(end) - t0.max(start)).max(0.0) * rate;
                }
                Emitter::Instant { start, count } if t0 <= start && start < t1 => {
                    self.emitted += count;
                }
                Emitter::Instant { .. } => {}
            }
        }
        // At most the room left (one more for the fraction); what does not fit is dropped.
        let room = self.max.saturating_sub(self.particles.len());
        self.emitted = self.emitted.clamp(0.0, room as f32 + 1.0);
        while self.emitted >= 1.0 && self.particles.len() < self.max {
            self.emitted -= 1.0;
            self.spawn();
        }
        if self.particles.len() >= self.max {
            self.emitted = 0.0;
        }
        let ops = std::mem::take(&mut self.operators);
        for p in &mut self.particles {
            p.age += dt;
            let life = if p.lifetime > 0.0 {
                (p.age / p.lifetime).clamp(0.0, 1.0)
            } else {
                1.0
            };
            for op in &ops {
                match *op {
                    Op::Movement { gravity, drag } => {
                        let keep = (1.0 - drag).powf(dt * 30.0);
                        for (k, gk) in gravity.iter().enumerate() {
                            p.velocity[k] = (p.velocity[k] + gk * dt) * keep;
                            p.position[k] += p.velocity[k] * dt;
                        }
                    }
                    Op::Rotation => p.rotation += p.spin * dt,
                    Op::AlphaFade {
                        from,
                        to,
                        fade_in,
                        fade_out,
                    } => {
                        let a = if life < fade_in.1 {
                            from + (1.0 - from)
                                * ((life - fade_in.0) / (fade_in.1 - fade_in.0).max(1e-4))
                                    .clamp(0.0, 1.0)
                        } else if life > fade_out.0 {
                            1.0 + (to - 1.0)
                                * ((life - fade_out.0) / (fade_out.1 - fade_out.0).max(1e-4))
                                    .clamp(0.0, 1.0)
                        } else {
                            1.0
                        };
                        p.color[3] = p.base_alpha * a;
                    }
                    Op::AlphaRandom { out, time } => {
                        let a = if out {
                            ((1.0 - life) / time).clamp(0.0, 1.0)
                        } else {
                            (life / time).clamp(0.0, 1.0)
                        };
                        p.color[3] = p.base_alpha * a;
                    }
                    Op::RadiusScale {
                        from,
                        to,
                        start,
                        end,
                    } => {
                        let t = ((life - start) / (end - start).max(1e-4)).clamp(0.0, 1.0);
                        p.radius = p.base_radius * (from + (to - from) * t);
                    }
                    Op::ColorFade { color, start, end } => {
                        let t = ((life - start) / (end - start).max(1e-4)).clamp(0.0, 1.0);
                        for (k, ck) in color.iter().take(3).enumerate() {
                            p.color[k] =
                                p.base_color[k] + (f32::from(*ck) / 255.0 - p.base_color[k]) * t;
                        }
                    }
                }
            }
        }
        self.operators = ops;
        // Without a lifespan operator particles still end at their lifetime (as nearly every system decays them).
        self.particles.retain(|p| p.age < p.lifetime);
        self.time = t1;
    }
}

/// A sheet frame: its duration and its UV rectangle (`u0 v0 u1 v1`).
pub type SheetFrame = (f32, [f32; 4]);

/// A sprite sheet (a texture's `0x10` resource): each sequence's frames, each a duration and a UV rectangle (`u0 v0 u1 v1`).
#[derive(Debug, Clone, PartialEq)]
pub struct Sheet {
    pub sequences: Vec<(i32, Vec<SheetFrame>)>,
}

impl Sheet {
    /// Reads a sheet resource's bytes (after its length word).
    pub fn parse(b: &[u8]) -> Result<Sheet> {
        let version = i32_at(b, 0, "the sheet")?;
        let n = crate::read::count(
            i32_at(b, 4, "the sheet")?,
            16,
            b.len(),
            "the sheet's sequences",
        )?;
        let images = match version {
            0 => 1,
            1 => 4,
            v => return invalid(format!("sheet version {v} is not read")),
        };
        let mut at = 8;
        let mut sequences = Vec::with_capacity(n);
        for _ in 0..n {
            let seq = i32_at(b, at, "a sheet sequence")?;
            let frames = crate::read::count(
                i32_at(b, at + 8, "a sheet sequence")?,
                4 + 16 * images,
                b.len().saturating_sub(at + 16),
                "a sheet sequence's frames",
            )?;
            at += 16;
            let mut list = Vec::with_capacity(frames);
            for _ in 0..frames {
                let t = f32_at(b, at, "a sheet frame")?;
                let uv =
                    [0, 1, 2, 3].map(|k| f32_at(b, at + 4 + k * 4, "a sheet frame").unwrap_or(0.0));
                slice(b, at, 4 + 16 * images, "a sheet frame")?;
                list.push((t, uv));
                at += 4 + 16 * images;
            }
            sequences.push((seq, list));
        }
        Ok(Sheet { sequences })
    }

    /// The UV rectangle of sequence `seq`'s first frame (the whole texture when there is none).
    pub fn rect(&self, seq: i32) -> [f32; 4] {
        self.sequences
            .iter()
            .find(|s| s.0 == seq)
            .or(self.sequences.first())
            .and_then(|s| s.1.first())
            .map_or([0.0, 0.0, 1.0, 1.0], |f| f.1)
    }
}

/// A particle file (tests and fuzz seeds): one system emitting continuously, with a lifetime, a radius, a color, movement with
/// gravity, a fade and a renderer.
pub fn write_test_file() -> Vec<u8> {
    let strings = [
        "DmElement",
        "DmeParticleSystemDefinition",
        "DmeParticleOperator",
        "particleSystemDefinitions",
        "functionName",
        "emitters",
        "initializers",
        "operators",
        "renderers",
        "material",
        "max_particles",
        "emission_rate",
        "lifetime_min",
        "lifetime_max",
        "color1",
        "color2",
        "gravity",
        "drag",
    ];
    let mut b = b"<!-- dmx encoding binary 2 format pcf 1 -->\n\0".to_vec();
    b.extend_from_slice(&(strings.len() as u16).to_le_bytes());
    for s in strings {
        b.extend_from_slice(s.as_bytes());
        b.push(0);
    }
    let elems: [(u16, &str); 7] = [
        (0, "untitled"),
        (1, "smoke"),
        (2, "emit_continuously"),
        (2, "Lifetime Random"),
        (2, "Color Random"),
        (2, "Movement Basic"),
        (2, "render_animated_sprites"),
    ];
    b.extend_from_slice(&(elems.len() as i32).to_le_bytes());
    for (k, name) in elems {
        b.extend_from_slice(&k.to_le_bytes());
        b.extend_from_slice(name.as_bytes());
        b.push(0);
        b.extend_from_slice(&[7u8; 16]);
    }
    let attr = |b: &mut Vec<u8>, name: u16, ty: u8, value: &[u8]| {
        b.extend_from_slice(&name.to_le_bytes());
        b.push(ty);
        b.extend_from_slice(value);
    };
    let arr = |items: &[i32]| -> Vec<u8> {
        let mut v = (items.len() as i32).to_le_bytes().to_vec();
        for i in items {
            v.extend_from_slice(&i.to_le_bytes());
        }
        v
    };
    let cstr = |s: &str| -> Vec<u8> {
        let mut v = s.as_bytes().to_vec();
        v.push(0);
        v
    };
    // The root: the list of definitions.
    b.extend_from_slice(&1i32.to_le_bytes());
    attr(&mut b, 3, 15, &arr(&[1]));
    // The system.
    b.extend_from_slice(&6i32.to_le_bytes());
    attr(&mut b, 5, 15, &arr(&[2]));
    attr(&mut b, 6, 15, &arr(&[3, 4]));
    attr(&mut b, 7, 15, &arr(&[5]));
    attr(&mut b, 8, 15, &arr(&[6, 99]));
    attr(&mut b, 9, 5, &cstr("particle/smoke1/smoke1.vmt"));
    attr(&mut b, 10, 2, &64i32.to_le_bytes());
    // Its functions.
    b.extend_from_slice(&2i32.to_le_bytes());
    attr(&mut b, 4, 5, &cstr("emit_continuously"));
    attr(&mut b, 11, 3, &20f32.to_le_bytes());
    b.extend_from_slice(&3i32.to_le_bytes());
    attr(&mut b, 4, 5, &cstr("Lifetime Random"));
    attr(&mut b, 12, 3, &1f32.to_le_bytes());
    attr(&mut b, 13, 3, &2f32.to_le_bytes());
    b.extend_from_slice(&3i32.to_le_bytes());
    attr(&mut b, 4, 5, &cstr("Color Random"));
    attr(&mut b, 14, 8, &[255, 128, 0, 255]);
    attr(&mut b, 15, 8, &[255, 64, 0, 255]);
    b.extend_from_slice(&3i32.to_le_bytes());
    attr(&mut b, 4, 5, &cstr("Movement Basic"));
    let mut g = Vec::new();
    for c in [0f32, 0.0, 50.0] {
        g.extend_from_slice(&c.to_le_bytes());
    }
    attr(&mut b, 16, 10, &g);
    attr(&mut b, 17, 3, &0.05f32.to_le_bytes());
    b.extend_from_slice(&1i32.to_le_bytes());
    attr(&mut b, 4, 5, &cstr("render_animated_sprites"));
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_runs_the_test_system() {
        let p = Pcf::parse(&write_test_file()).unwrap();
        assert_eq!(p.elements.len(), 7);
        let s = p.system("SMOKE").unwrap();
        assert_eq!(s.material(), Some("particle/smoke1/smoke1.vmt"));
        assert_eq!(s.emitters.len(), 1);
        assert_eq!(s.initializers.len(), 2);
        assert_eq!(
            s.renderers.len(),
            1,
            "the reference past the elements is dropped"
        );
        let mut sim = Sim::new(&s, 7);
        for _ in 0..30 {
            sim.step(1.0 / 30.0);
        }
        assert!(
            (15..=21).contains(&sim.particles.len()),
            "{}",
            sim.particles.len()
        );
        assert!(
            sim.particles
                .iter()
                .all(|q| q.color[0] > 0.99 && q.lifetime >= 1.0)
        );
        assert!(sim.particles.iter().any(|q| q.position[2] > 0.0));
    }

    #[test]
    fn refuses_hostile_files() {
        assert!(Pcf::parse(b"<!-- dmx encoding binary 5 format pcf 2 -->\n\0").is_err());
        assert!(Pcf::parse(b"no header").is_err());
        let mut b = write_test_file();
        let at = b.len() - 30;
        b.truncate(at);
        assert!(Pcf::parse(&b).is_err());
        // An array that claims more items than the bytes left.
        let mut b = b"<!-- dmx encoding binary 2 format pcf 1 -->\n\0".to_vec();
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(b"a\0");
        b.extend_from_slice(&1i32.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(b"x\0");
        b.extend_from_slice(&[0; 16]);
        b.extend_from_slice(&1i32.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.push(17);
        b.extend_from_slice(&i32::MAX.to_le_bytes());
        assert!(Pcf::parse(&b).is_err());
    }

    /// A system of `emitters` references to one instant emitter of `n` particles, and one sequence initializer.
    fn system_file(emitters: usize, n: i32, seq: (i32, i32)) -> Pcf {
        let attr = |k: &str, v: Value| (Rc::<str>::from(k), v);
        let el = |kind: &str, name: &str, attrs: Vec<(Rc<str>, Value)>| {
            Rc::new(Element {
                kind: kind.into(),
                name: name.into(),
                attrs,
            })
        };
        Pcf {
            elements: vec![
                el(
                    "DmeParticleSystemDefinition",
                    "burst",
                    vec![
                        attr(
                            "emitters",
                            Value::Array(vec![Value::Element(Some(1)); emitters]),
                        ),
                        attr("initializers", Value::Array(vec![Value::Element(Some(2))])),
                        attr("max_particles", Value::Int(i32::MAX)),
                    ],
                ),
                el(
                    "DmeParticleOperator",
                    "emit_instantaneously",
                    vec![
                        attr("num_to_emit", Value::Int(n)),
                        attr("emission_start_time", Value::Float(0.0)),
                    ],
                ),
                el(
                    "DmeParticleOperator",
                    "Sequence Random",
                    vec![
                        attr("functionName", Value::Str("Sequence Random".into())),
                        attr("sequence_min", Value::Int(seq.0)),
                        attr("sequence_max", Value::Int(seq.1)),
                    ],
                ),
            ],
        }
    }

    #[test]
    fn emission_stops_at_the_particle_cap() {
        // 100,000 references to one emitter of i32::MAX particles: emitted once, capped, the leftover dropped.
        let p = system_file(100_000, i32::MAX, (0, 3));
        let s = p.system("burst").unwrap();
        assert_eq!(
            s.emitters.len(),
            1,
            "a function listed many times counts once"
        );
        let mut sim = Sim::new(&s, 1);
        let t = std::time::Instant::now();
        sim.step(0.1);
        sim.step(0.1);
        assert_eq!(sim.particles.len(), MAX_PARTICLES);
        assert!(t.elapsed().as_secs() < 5);
    }

    #[test]
    fn many_distinct_functions_are_capped() {
        let mut p = system_file(1, 1, (0, 0));
        let extra: Vec<Rc<Element>> = (0..1000)
            .map(|k| {
                Rc::new(Element {
                    kind: "DmeParticleOperator".into(),
                    name: format!("op{k}"),
                    attrs: Vec::new(),
                })
            })
            .collect();
        let first = p.elements.len();
        p.elements.extend(extra);
        let list = Value::Array(
            (first..first + 1000)
                .map(|i| Value::Element(Some(i)))
                .collect(),
        );
        let mut sys = (*p.elements[0]).clone();
        sys.attrs.push((Rc::from("operators"), list));
        p.elements[0] = Rc::new(sys);
        assert_eq!(p.system("burst").unwrap().operators.len(), MAX_FUNCTIONS);
    }

    #[test]
    fn extreme_sequences_stay_in_range() {
        for seq in [
            (i32::MIN, i32::MAX),
            (i32::MAX, i32::MIN),
            (-5, 5),
            (2000, 3000),
        ] {
            let p = system_file(1, 50, seq);
            let mut sim = Sim::new(&p.system("burst").unwrap(), 9);
            sim.step(0.1);
            assert!(
                sim.particles
                    .iter()
                    .all(|q| (0..=MAX_SEQUENCE).contains(&q.sequence)),
                "{seq:?}"
            );
        }
    }

    #[test]
    fn attribute_names_are_shared_not_copied() {
        // Many attributes naming one long string: the parsed names share the table's one copy.
        let long = "n".repeat(4000);
        let mut b = b"<!-- dmx encoding binary 2 format pcf 1 -->\n\0".to_vec();
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(b"DmElement\0");
        b.extend_from_slice(long.as_bytes());
        b.push(0);
        b.extend_from_slice(&1i32.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(b"x\0");
        b.extend_from_slice(&[0; 16]);
        let file = |n: usize| {
            let mut b = b.clone();
            b.extend_from_slice(&(n as i32).to_le_bytes());
            for _ in 0..n {
                b.extend_from_slice(&1u16.to_le_bytes());
                b.push(4);
                b.push(1);
            }
            b
        };
        let n = MAX_ATTRS;
        let p = Pcf::parse(&file(n)).unwrap();
        let attrs = &p.elements[0].attrs;
        assert_eq!(attrs.len(), n);
        assert!(Rc::ptr_eq(&attrs[0].0, &attrs[n - 1].0));
        // One more is refused.
        let e = Pcf::parse(&file(n + 1)).unwrap_err();
        assert!(e.to_string().contains("attributes"), "{e}");
    }

    #[test]
    fn settings_are_read_once_and_unknown_functions_dropped() {
        // The most operators a system keeps, each padded to the most attributes, its settings last: a step costs the same
        // as with bare operators, since nothing is looked up per particle.
        let mut p = system_file(1, MAX_PARTICLES as i32, (0, 0));
        let pad: Vec<(Rc<str>, Value)> = (0..MAX_ATTRS - 2)
            .map(|_| (Rc::from("padding"), Value::Int(0)))
            .collect();
        let first = p.elements.len();
        for k in 0..MAX_FUNCTIONS {
            let mut attrs = pad.clone();
            let name = if k % 2 == 0 {
                "Movement Basic"
            } else {
                "unknown"
            };
            attrs.push((Rc::from("functionName"), Value::Str(name.into())));
            attrs.push((Rc::from("drag"), Value::Float(0.5)));
            p.elements.push(Rc::new(Element {
                kind: "DmeParticleOperator".into(),
                name: format!("op{k}"),
                attrs,
            }));
        }
        let list = Value::Array(
            (first..first + MAX_FUNCTIONS)
                .map(|i| Value::Element(Some(i)))
                .collect(),
        );
        let mut sys = (*p.elements[0]).clone();
        sys.attrs.push((Rc::from("operators"), list));
        p.elements[0] = Rc::new(sys);
        let mut sim = Sim::new(&p.system("burst").unwrap(), 1);
        assert_eq!(sim.operators.len(), MAX_FUNCTIONS / 2);
        assert!(matches!(sim.operators[0], Op::Movement { drag, .. } if drag == 0.5));
        let t = std::time::Instant::now();
        for _ in 0..4 {
            sim.step(0.05);
        }
        assert_eq!(sim.particles.len(), MAX_PARTICLES);
        assert!(t.elapsed().as_secs() < 5);
    }

    #[test]
    fn reads_a_sheet() {
        let mut b = Vec::new();
        for v in [1i32, 1, 3, 1, 1] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&1f32.to_le_bytes());
        b.extend_from_slice(&1f32.to_le_bytes());
        for v in [0.25f32, 0.0, 0.5, 0.5] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&[0u8; 48]);
        let s = Sheet::parse(&b).unwrap();
        assert_eq!(s.rect(3), [0.25, 0.0, 0.5, 0.5]);
        assert!(Sheet::parse(&b[..20]).is_err());
    }
}
