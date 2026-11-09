//! What the host implements for the ghost manager: the objects of the world as the network sees them (`ObjectSource`) and the
//! script call the manager makes (`GhostScripts`).
//!
//! The manager never holds a reference to the host's world between calls: every call takes the source it needs.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `sim/netGhost.cpp` (Copyright (c) 2012
//! GarageGames, LLC) and `sim/netObject.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the
//! code here is this crate's own.

use super::policy::ScopeRadius;
use crate::bits::BitWriter;

/// A SimObject id (the id scripts see), not a ghost index.
pub type ObjId = u32;

/// `GhostInfo.flags`.
pub mod flags {
    /// Seen by the last scope pass.
    pub const IN_SCOPE: u16 = 0x02;
    /// `NetFlags::ScopeAlways` (the object is always in scope).
    pub const SCOPE_ALWAYS: u16 = 0x04;
    /// No NewGhost record has been acked: the next record carries the class form.
    pub const NOT_YET_GHOSTED: u16 = 0x08;
    /// A NewGhost record is in flight: no second record until it is acked.
    pub const GHOSTING: u16 = 0x10;
    /// The object is gone; a kill record is due.
    pub const KILL_GHOST: u16 = 0x20;
    /// The kill record is in flight.
    pub const KILLING_GHOST: u16 = 0x40;
    /// The ghost was created by `activateGhosting` (its `GhostAlwaysObjectEvent` carries the class form).
    pub const SCOPED_EVENT: u16 = 0x80;
    /// Set by `scopeToClient`: stays in scope through a full query.
    pub const SCOPE_LOCAL_ALWAYS: u16 = 0x100;
}

/// `NetObject::mNetFlags` (the bits the manager reads).
pub mod net_flags {
    /// Ghost-always scoped at once (the ghost starts as `0x0e` instead of `0x0a`).
    pub const SCOPE_ALWAYS: u32 = 0x40;
    /// Only a loopback connection scopes it (never set by a standard server).
    pub const SCOPE_LOCAL: u32 = 0x80;
    /// The object may be ghosted at all.
    pub const GHOSTABLE: u32 = 0x100;
}

/// The scope object's view of the world (the 11-word `camInfo`, filled on every packet by the scope object's `getCameraInfo`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CamInfo {
    pub object: ObjId,
    pub eye: [f32; 3],
    pub forward: [f32; 3],
    pub fov: f32,
    pub fov_sin: f32,
    pub fov_cos: f32,
    /// The scope radius ([`ScopeRadius::of`] the Sky's visible distance); set by the manager, the host may leave it 0.
    pub visible_distance: f32,
}

/// A scope radius the ghost managers take in place of their schedule's (its `f32` bits; 0: none). Only for checks and
/// benchmarks that stand for a server sending every object; a game never sets it.
pub static SCOPE_OVERRIDE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The scope radius a manager uses: [`SCOPE_OVERRIDE`] when set, else `radius` of the Sky's distance.
pub fn scope_radius_used(sky: Option<f32>, radius: &ScopeRadius) -> f32 {
    match SCOPE_OVERRIDE.load(std::sync::atomic::Ordering::Relaxed) {
        0 => radius.of(sky),
        bits => f32::from_bits(bits),
    }
}

/// The comparison `dQsort` gets for the priority sort: `d = a - b`; `d < 0` is -1, otherwise 1 if `d > 0`, else 0. A NaN gives 0.
///
/// The order equal priorities are left in decides the array order, so which ghost is sent first: [`ObjectSource::sort_pending`]
/// keeps them in array order unless the host sorts otherwise.
pub fn priority_compare(a: f32, b: f32) -> i32 {
    let d = a - b;
    if d < 0.0 { -1 } else { i32::from(d > 0.0) }
}

/// The world as the ghost manager reads it. Everything but [`ObjectSource::pack_update`] takes `&self`, so a scope walk can hold the
/// source while it hands objects to the manager.
pub trait ObjectSource {
    /// Whether the object exists now.
    fn exists(&self, obj: ObjId) -> bool;

    /// The object's net class id (the `NetClassBitSize` group's id, 6 bits), or `None` for an object that has none.
    fn class_id(&self, obj: ObjId) -> Option<u32>;

    /// `NetObject::mNetFlags`, see [`net_flags`].
    fn net_flags(&self, obj: ObjId) -> u32;

    /// `packUpdate(conn, mask, stream)`: writes the object's state for the dirty bits `mask` and returns the bits it could not send
    /// (`retMask`; 0 for all sent). **It must be repeatable**: the manager rolls a record back, and measures nothing twice,
    /// when it overflows the 12,000-bit stream, so a call may be repeated with the same arguments. Strings go through the
    /// writer's string buffer.
    ///
    /// `ghost_of` is `getGhostIndex` on this connection: the wire id of an object whose NewGhost record was acknowledged,
    /// `None` otherwise. Packers that link to other objects (a mounted object, a light's owner) call it for the objects they link to.
    fn pack_update(
        &mut self,
        obj: ObjId,
        mask: u32,
        w: &mut BitWriter,
        ghost_of: &dyn Fn(ObjId) -> Option<u32>,
    ) -> u32;

    /// `getUpdatePriority(&camInfo, mask, skipCount)` (a virtual method). Called when the pending ghosts are sorted by priority
    /// (the policy's [`super::policy::GhostPolicy::pending_order`]).
    fn update_priority(&self, obj: ObjId, cam: &CamInfo, mask: u32, skip: u8) -> f32;

    /// The share of each packet the object's updates belong to: `Some((group, part))` holds the updates (and first records) of
    /// all the objects of `group` in one packet to `part` (0 to 1) of the packet's bits; an update that would take its group
    /// past that is passed over and stays dirty for a later packet, so a flood of one kind of object cannot crowd the others
    /// out. `None` (the default, and every TGE object) is no share: the update goes as TGE's would.
    fn packet_share(&self, obj: ObjId) -> Option<(u32, f32)> {
        let _ = obj;
        None
    }

    /// The scope object's camera data (eye, forward, field of view), `None` if it has none. The manager fills
    /// `visible_distance` itself.
    fn camera_info(&self, scope: ObjId) -> Option<CamInfo>;

    /// The Sky's `visibleDistance`, `None` when there is no Sky (or it has none): the scope radius is then the policy's
    /// ([`super::policy::ScopeRadius::no_sky`]) and the full query still runs (the server's scene graph exists from start-up).
    fn sky_visible_distance(&self) -> Option<f32>;

    /// The object the scope object is mounted on (`ShapeBase::mMount`), scoped with it.
    fn mount_of(&self, scope: ObjId) -> Option<ObjId>;

    /// `ShapeBase::onCameraScopeQuery`'s container walk (`Container::findObjects`): calls `visit` for every object in the box of
    /// half-size `radius` around the camera, in the container's order (the bins y-outer x-inner, then the overflow list; a host
    /// with containers of its own walks them in its own fixed order), with the object-type mask `0xffffffff`. The callback test
    /// (`netFlags & 0x140 == 0x100` and the sphere against the box) belongs to the host. `visit` returns `false` when the work
    /// budget is spent: stop. When the scope object is not in a scene graph the host walks the whole container without a distance
    /// test.
    fn scope_query(
        &self,
        scope: ObjId,
        cam: &CamInfo,
        radius: f32,
        visit: &mut dyn FnMut(ObjId) -> bool,
    );

    /// [`ObjectSource::scope_query`] resumed after a budget cut: the walk's first `skip` places are passed over
    /// without a visit, and the places walked are returned (up to and with the one whose `visit` stopped it, or all of them). A
    /// place is the host's own count; this default counts the visits (the objects in front are walked and tested again), a host
    /// may count the walk's entries so that a resumed walk passes over them without testing them (later packets
    /// tested every object again, which is slow with many objects).
    fn scope_query_from(
        &self,
        scope: ObjId,
        cam: &CamInfo,
        radius: f32,
        skip: usize,
        visit: &mut dyn FnMut(ObjId) -> bool,
    ) -> usize {
        let mut walked = 0usize;
        self.scope_query(scope, cam, radius, &mut |o| {
            walked += 1;
            walked <= skip || visit(o)
        });
        walked
    }

    /// The ghostable objects added since `from` (the feed of new objects: see
    /// [`crate::ghost::GhostManager::set_fresh`]): every object added gets the next number of one count, and this answers the number
    /// of the slice's first object with the slice of those numbered `from` and up that the host still holds (the host may have let go
    /// of the oldest: the first number is then above `from`). The default holds none.
    fn fresh_objects(&self, from: u64) -> (u64, &[ObjId]) {
        (from, &[])
    }

    /// The number the next object added will get (where a feed that starts now begins).
    fn fresh_end(&self) -> u64 {
        0
    }

    /// Distance from the object to the scope object, `None` when the object is not a SceneObject. Used only by the walks
    /// besides the full query (the policy's list between queries, the feed), with the netFlags test `& 0x140 == 0x100`.
    fn scope_distance(&self, obj: ObjId, scope: ObjId) -> Option<f32>;

    /// Whether the full scope query from `eye` would take `obj`: the walks besides the full query then take an object whose
    /// centre is past the radius but whose box reaches into it now, not at the next full query (objects along the
    /// scope's edge would otherwise reach the client late). Only objects the full query would take: the set in scope is the same,
    /// it is reached sooner. A source without the query answers false.
    fn reaches_scope(&self, _obj: ObjId, _eye: [f32; 3], _radius: f32) -> bool {
        false
    }

    /// The objects that must go into scope with `obj` when it first does, in the host's order, appended to `out`. Torque scopes
    /// each object alone: the default appends none.
    fn dependents(&self, obj: ObjId, out: &mut Vec<ObjId>) {
        let _ = (obj, out);
    }

    /// `GhostAlwaysSet` in set order (the order objects were added: it fixes the ghost ids).
    fn ghost_always(&self) -> &[ObjId];

    /// Sorts the pending ghosts' `(priority, ghost id)` pairs ascending by priority, as [`priority_compare`] orders them. The
    /// default is a stable sort (ties stay in array order; a NaN priority goes where `f32::total_cmp` puts it); a host may use
    /// another sort, whose tie order then decides the send order.
    fn sort_pending(&self, items: &mut [(f32, u32)]) {
        // -0 and +0 are one priority, as `priority_compare` has them.
        let key = |p: f32| if p == 0.0 { 0.0 } else { p };
        items.sort_by(|a, b| key(a.0).total_cmp(&key(b.0)));
    }
}

/// The script call the ghost layer makes.
pub trait GhostScripts {
    /// `onGhostAlwaysObjectsReceived`: the client's `ConnectionMessage` 1 with the current ghosting sequence arrived.
    fn on_ghost_always_objects_received(&mut self);
}

/// A [`GhostScripts`] that does nothing (tests and the reference client).
#[derive(Debug, Default, Clone, Copy)]
pub struct NoScripts;

impl GhostScripts for NoScripts {
    fn on_ghost_always_objects_received(&mut self) {}
}
