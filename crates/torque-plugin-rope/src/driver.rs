//! Rope drivers: another plugin steers a rope's ends, length and curve from Rust, without going through script.
//!
//! A rope normally follows what a script told it: the points it was pinned to, the object it hangs from, the length it reels to. A
//! *driver* is a plugin's own answer to "where are this rope's ends and how long is it now": a joint's two anchors and its current
//! length, a grab beam's muzzle and the point held on a prop. The rope stays a visual: a driver only says where the rope is drawn,
//! and nothing a rope does ever pushes back.
//!
//! There are two ways in, and both end in [`RopeWorld::drive`]:
//!
//! - **By hand.** A plugin that keeps the [`RopeShared`](crate::RopeShared) world calls [`RopeWorld::drive`] with a [`Drive`] each
//!   frame (from a render pass) or each tick, for any rope it made. An end given as [`DriveEnd::Point`] is exactly there in the
//!   frame that is drawn next; an end given as [`DriveEnd::Ghost`] follows where the frame draws that ghost, at an offset in the
//!   ghost's own frame, every frame, with no more calls.
//! - **By a registered driver.** A [`RopeDriver`] registered under a name ([`RopePlugin::with_driver`](crate::RopePlugin::with_driver),
//!   or [`RopeDrivers::register`] from another plugin's build) is asked once each client tick for every rope bound to it
//!   ([`RopeDrivers::bind`]) and its answer is applied with [`RopeWorld::drive`]. A server makes a networked rope follow a driver
//!   with `ropeNetDrive(rope, name, object)` ([`net::server::set_driver`](crate::net::server::set_driver) from Rust): every client
//!   binds its copy of the rope to the driver of that name, keyed by the client's ghost of `object` (a joint, say), so the length
//!   and the ends the driver reports never cross the network a second time.
//!
//! A driver owns what its [`Drive`] says: the two ends, whether the rope is at its limit, and its curve; the length too when the
//! drive carries one. Whatever a script or a ghost record says about those is replaced on the next tick.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use torque_ts_vm::vm::Vm;

use crate::sim::{Curve, RopeId, RopeWorld};

/// The most driver names a client warns about before it starts the list again (a server that sends endless names is not given
/// endless memory).
const MAX_WARNED_DRIVERS: usize = 256;

/// The longest name a driver is registered under (it is on the wire for a networked rope).
pub const MAX_DRIVER_NAME_BYTES: usize = 24;

/// What holds one end of a driven rope.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum DriveEnd {
    /// Nothing: the end falls with the rest.
    #[default]
    Free,
    /// A point in the world, where the end is drawn exactly.
    Point([f32; 3]),
    /// A place in a ghost's own frame: `node` on its shape (empty: its origin) moved by `offset` in the frame of the ghost's
    /// drawn transform, so the end rotates with the ghost and is where the frame draws that point, every frame.
    Ghost {
        /// The ghost's number.
        ghost: u32,
        /// The place on its shape; empty for its origin.
        node: String,
        /// The offset in the ghost's frame.
        offset: [f32; 3],
    },
}

/// What a driver says about a rope for now: where its ends are, how long it is, whether it is at its limit and what curve it has.
#[derive(Clone, Debug, PartialEq)]
pub struct Drive {
    /// What holds the first end.
    pub start: DriveEnd,
    /// What holds the far end.
    pub end: DriveEnd,
    /// The length the rope is to have (None: the rope keeps its own, and a script's `ropeSetLength` still works). It is held
    /// inside the rope's range ([`MAX_LENGTH`](crate::MAX_LENGTH)); a line's length is the distance between its ends and is not
    /// set.
    pub length: Option<f32>,
    /// The seconds the rope takes to reach a changed `length` (0: at once). A driver that gives a new length every tick gives
    /// the tick's seconds here, so the rope runs on smoothly between two ticks. The first length a rope is given is set at once.
    pub settle: f32,
    /// Whether the thing the rope shows is at the end of its reach: the rope is then drawn straight and evenly spaced between its
    /// ends even when they are a little nearer than its length (a taut joint), where otherwise it hangs.
    pub at_limit: bool,
    /// The driven curve (None: the rope is simulated, or straight when its datablock is not simulated).
    pub curve: Option<Curve>,
}

impl Drive {
    /// A drive with the two ends, no length of its own, not at its limit and no curve.
    pub fn new(start: DriveEnd, end: DriveEnd) -> Drive {
        Drive {
            start,
            end,
            length: None,
            settle: 0.0,
            at_limit: false,
            curve: None,
        }
    }
}

/// A plugin's answer to where a rope is, asked once each client tick for every rope bound to it ([`RopeDrivers::bind`]).
pub trait RopeDriver: 'static {
    /// What the rope bound with `key` is now, in the client's script engine `vm`. For a networked rope `key` is the client's
    /// ghost number of the object the server named. None: the thing the rope showed is gone; the rope is let go of the driver
    /// and stays as it was (the server's rope is deleted by whoever made it).
    fn drive(&self, vm: &Vm, key: u32) -> Option<Drive>;
}

/// The drivers by name and the ropes bound to them: an engine resource of the client.
#[derive(Default)]
struct Book {
    drivers: BTreeMap<String, Rc<dyn RopeDriver>>,
    bound: BTreeMap<RopeId, (String, u32)>,
    /// Names a rope was bound to that no driver has, warned about once.
    missing: BTreeSet<String>,
}

/// The registry of rope drivers ([module docs](self)). A clone is the same registry.
#[derive(Clone, Default)]
pub struct RopeDrivers(Rc<RefCell<Book>>);

impl RopeDrivers {
    /// An empty registry.
    pub fn new() -> RopeDrivers {
        RopeDrivers::default()
    }

    /// Registers `driver` under `name` (1 to [`MAX_DRIVER_NAME_BYTES`] bytes, no whitespace). False when the name is not valid or
    /// is taken: a name is one driver's.
    pub fn register(&self, name: &str, driver: impl RopeDriver) -> bool {
        self.register_shared(name, Rc::new(driver))
    }

    /// [`RopeDrivers::register`] for a driver that is shared already.
    pub(crate) fn register_shared(&self, name: &str, driver: Rc<dyn RopeDriver>) -> bool {
        if !valid_name(name) {
            return false;
        }
        let mut book = self.0.borrow_mut();
        if book.drivers.contains_key(name) {
            return false;
        }
        book.drivers.insert(name.to_owned(), driver);
        true
    }

    /// Whether a driver is registered under `name`.
    pub fn is_registered(&self, name: &str) -> bool {
        self.0.borrow().drivers.contains_key(name)
    }

    /// Binds `rope` to the driver `name`, to be asked with `key` each tick; a rope has one driver, so an earlier binding is
    /// replaced. The name need not be registered yet (a rope that arrives before its driver's plugin is added waits).
    pub fn bind(&self, rope: RopeId, name: &str, key: u32) {
        self.0
            .borrow_mut()
            .bound
            .insert(rope, (name.to_owned(), key));
    }

    /// Lets `rope` go of its driver. False if it had none. The rope is no longer at its limit ([`RopeWorld::release_drive`]); its
    /// ends stay as the driver last put them (a point end where it last was, a ghost end still following its ghost), and so do its
    /// length and curve, until a script or another driver changes them.
    pub fn unbind(&self, world: &mut RopeWorld, rope: RopeId) -> bool {
        let had = self.forget(rope);
        if had {
            world.release_drive(rope);
        }
        had
    }

    /// Takes the binding of a rope that is gone away. False if it had none.
    pub(crate) fn forget(&self, rope: RopeId) -> bool {
        self.0.borrow_mut().bound.remove(&rope).is_some()
    }

    /// The driver name and the key `rope` is bound with.
    pub fn bound(&self, rope: RopeId) -> Option<(String, u32)> {
        self.0.borrow().bound.get(&rope).cloned()
    }

    /// How many ropes are bound to a driver.
    pub fn bound_count(&self) -> usize {
        self.0.borrow().bound.len()
    }

    /// Asks each driver about each rope bound to it and applies the answers. A rope that is gone is forgotten; a rope whose driver
    /// has nothing to say is let go; a rope bound to a name no driver has waits (the console says so once).
    pub(crate) fn run(&self, vm: &mut Vm, world: &mut RopeWorld) {
        let work: Vec<_> = {
            let mut book = self.0.borrow_mut();
            book.bound.retain(|&rope, _| world.rope(rope).is_some());
            book.bound
                .iter()
                .map(|(&rope, (name, key))| {
                    (rope, name.clone(), *key, book.drivers.get(name).cloned())
                })
                .collect()
        };
        for (rope, name, key, driver) in work {
            let Some(driver) = driver else {
                let first = {
                    let mut book = self.0.borrow_mut();
                    if book.missing.len() >= MAX_WARNED_DRIVERS {
                        book.missing.clear();
                    }
                    book.missing.insert(name.clone())
                };
                if first {
                    vm.warn(format!(
                        "a rope is bound to the driver {name:?}, which no plugin has registered"
                    ));
                }
                continue;
            };
            match driver.drive(vm, key) {
                Some(drive) => {
                    let _ = world.drive(rope, &drive);
                }
                None => {
                    self.unbind(world, rope);
                }
            }
        }
    }
}

/// A name a driver may have: 1 to [`MAX_DRIVER_NAME_BYTES`] bytes with no whitespace.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_DRIVER_NAME_BYTES
        && !name.bytes().any(|b| b.is_ascii_whitespace())
}
