//! What a game may change about ghosting: the [`GhostPolicy`] a [`super::GhostManager`] is built with, and its questions about
//! objects, an [`ObjectPolicy`] over a game's own world. Every method has TGE 1.4's behaviour as its default ([`TorqueGhosts`],
//! `ghostWritePacket`): the full scope query runs with every packet and the ghosts with updates pending it did not find are
//! detached, every object a scope walk meets is scoped, every pending ghost is sorted by priority, nothing is dropped while its
//! object lives, and a NewGhost record names its class by id.
//!
//! **When the full query runs** is the policy's ([`GhostPolicy::full_query_due`]); a query a spent work budget cut runs on with the
//! next packet whatever the policy says. On a packet without it, the scan tries the objects the policy names
//! ([`ObjectPolicy::between_queries`]): each one within the scope radius is scoped, the walk resuming where a spent budget left it.
//!
//! **Admission and drops.** The policy is told of every ghost made ([`ObjectPolicy::ghost_made`]), every object a ghost lets go of
//! ([`GhostPolicy::ghost_unlinked`]) and every ghost freed ([`GhostPolicy::ghost_freed`]), so it can count the table its own way;
//! it decides whether a scope walk (the full query, the policy's list, the feed, the dependents of an object they scoped) may make a
//! new ghost ([`ObjectPolicy::admit`]); a script's scoping (`scopeToClient`) is never refused. At the end of a full query the
//! policy may ask for a drop pass ([`GhostPolicy::query_ended`]): each ghost the policy names ([`GhostPolicy::drop_candidate`],
//! [`ObjectPolicy::drop_now`]) gets a kill record (the object stays and may come back in scope), a ghost always in scope or
//! scoped by a script excepted. The pass is resumable under the work budget.
//!
//! **Order.** The pending ghosts are sent in priority order unless the policy names another ([`GhostPolicy::pending_order`]).
//!
//! **Implied class.** A policy whose NewGhost records carry a flag ahead of the class ([`GhostPolicy::implied_class_flag`]) writes
//! `flag(1)` alone for an object of its implied class and `flag(0)` + the class id for any other; the client must be told the same
//! ([`super::GhostClient::set_implied_class_flag`]).
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `sim/netGhost.cpp` (Copyright (c) 2012
//! GarageGames, LLC) and `sim/netObject.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the
//! code here is this crate's own.

use super::source::{ObjId, ObjectSource};

/// What the manager tells a policy about its table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TableCounts {
    /// The connection's ghost limit.
    pub max_ghosts: u32,
    /// Ghosts in use in all.
    pub ghosts: usize,
}

/// The scope radius around the scope object: the Sky's visible distance within `min..=max`, `no_sky` without one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScopeRadius {
    pub no_sky: f32,
    pub min: f32,
    pub max: f32,
}

impl Default for ScopeRadius {
    /// Torque's: the Sky's visible distance as it is, 1,000 without a Sky.
    fn default() -> ScopeRadius {
        ScopeRadius {
            no_sky: 1_000.0,
            min: f32::NEG_INFINITY,
            max: f32::INFINITY,
        }
    }
}

impl ScopeRadius {
    /// The radius for a Sky of visible distance `sky` (a NaN distance gives `max`).
    pub fn of(&self, sky: Option<f32>) -> f32 {
        let Some(v) = sky else {
            return self.no_sky;
        };
        let r = if v <= self.max { v } else { self.max };
        if r <= self.min { self.min } else { r }
    }
}

/// The order the pending ghosts go out in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingOrder {
    /// Sorted by priority (`getUpdatePriority`), highest first: Torque's.
    Priority,
    /// The array order as it is, without priorities; with `scope_first` the scope object's ghost goes first.
    Array { scope_first: bool },
}

/// A game's ghosting rules, the part that needs no world. See the module docs; every default is TGE's. The questions about objects
/// are [`ObjectPolicy`]'s.
pub trait GhostPolicy {
    /// Whether a Ready the client sends again for the same ghosting sequence is ignored: `onGhostAlwaysObjectsReceived` runs
    /// once a sequence. Torque runs it for every Ready (`false`), and a game's scripts that spawn there would spawn again.
    fn ready_once(&self) -> bool {
        false
    }

    /// The scope radius around the scope object (read with every packet).
    fn radius(&self) -> ScopeRadius {
        ScopeRadius::default()
    }

    /// Whether this packet's scan runs the full scope query, at sim time `now`. `resuming`: a query a spent budget cut runs on,
    /// whatever the answer.
    fn full_query_due(&mut self, now: u32, resuming: bool) -> bool {
        let _ = (now, resuming);
        true
    }

    /// Whether a full query has no effect of the policy's own: it scopes every object its walk meets, whatever it is, makes every
    /// ghost the table has room for ([`ObjectPolicy::admit`], [`ObjectPolicy::query_visit`] always true), asks for no drop pass and
    /// keeps no state of the walk. A query of such a policy over a source with a [`ObjectSource::scope_stamp`] that has run to
    /// its end with every ghost made and the stamp unchanged since only has to count its objects the next time, not scope them
    /// again. Torque's does; a policy that overrides any of those questions leaves this false (the default).
    fn plain_query(&self) -> bool {
        false
    }

    /// Whether a complete full query detaches the ghosts with updates pending that it did not find (`detachObject`).
    fn detach_out_of_scope(&self) -> bool {
        true
    }

    /// The order this packet's `pending` ghosts go out in; `full_query`: the scan ran the full query. Asked before the dead
    /// entries are freed.
    fn pending_order(&self, pending: usize, full_query: bool) -> PendingOrder {
        let _ = (pending, full_query);
        PendingOrder::Priority
    }

    /// Whether NewGhost records carry the implied-class flag ahead of the class id.
    fn implied_class_flag(&self) -> bool {
        false
    }

    /// A full query ran to its end at `now`, over a scope radius of `radius`: whether to start a drop pass over the ghosts still
    /// linked to their objects that [`GhostPolicy::drop_candidate`] names.
    fn query_ended(&mut self, now: u32, radius: f32, table: &TableCounts) -> bool {
        let _ = (now, radius, table);
        false
    }

    /// Whether the drop pass looks at the ghost `id` (asked when the pass starts and again when it reaches the ghost).
    fn drop_candidate(&self, id: u32) -> bool {
        let _ = id;
        false
    }

    /// A drop pass is done at `now`; `resuming`: a full query cut by the budget is still under way.
    fn drop_pass_done(&mut self, now: u32, resuming: bool) {
        let _ = (now, resuming);
    }

    /// The ghost `id` let go of its object (`detachObject`; the ghost lives on until its kill record is acknowledged).
    fn ghost_unlinked(&mut self, id: u32) {
        let _ = id;
    }

    /// The ghost `id` is freed (its id may be given to another object).
    fn ghost_freed(&mut self, id: u32) {
        let _ = id;
    }

    /// `resetGhosting`: the table is gone.
    fn reset(&mut self) {}
}

/// A game's ghosting rules about the objects of its world `S` (a game implements it for its own source, so it reads its own
/// objects). Every default is TGE's.
pub trait ObjectPolicy<S: ObjectSource>: GhostPolicy {
    /// Whether `obj` is of the implied class (asked only when [`GhostPolicy::implied_class_flag`] is true).
    fn implied_class(&self, src: &S, obj: ObjId) -> bool {
        let _ = (src, obj);
        false
    }

    /// Whether a scope walk (the full query, the policy's list, the feed, the dependents of an object they scoped) may make a new
    /// ghost of `obj`. `scope` is the scope object.
    fn admit(&mut self, src: &S, scope: Option<ObjId>, obj: ObjId, table: &TableCounts) -> bool {
        let _ = (src, scope, obj, table);
        true
    }

    /// A ghost `id` was made for `obj` (by a walk or a native).
    fn ghost_made(&mut self, src: &S, obj: ObjId, id: u32) {
        let _ = (src, obj, id);
    }

    /// Every packet's scan starts (before any walk), around `scope`.
    fn scan_started(&mut self, src: &S, scope: ObjId) {
        let _ = (src, scope);
    }

    /// A full query starts (not when one cut by the budget resumes).
    fn query_started(&mut self, src: &S, scope: ObjId, table: &TableCounts) {
        let _ = (src, scope, table);
    }

    /// Whether the full query takes `obj`, an object its walk met: false leaves it out of scope this time (its ghost, if it has
    /// one, is not marked in scope).
    fn query_visit(&mut self, src: &S, scope: ObjId, obj: ObjId) -> bool {
        let _ = (src, scope, obj);
        true
    }

    /// The objects the scan tries on a packet without the full query, in order (each one ghostable, not always in scope and within
    /// the scope radius is scoped); `None` walks nothing and keeps the walk's place for a later list. Torque walks nothing.
    fn between_queries<'a>(&mut self, src: &'a S) -> Option<&'a [ObjId]> {
        let _ = src;
        None
    }

    /// The scan is done (before the priorities); `pending` ghosts have updates to send.
    fn scanned(&mut self, src: &S, pending: usize) {
        let _ = (src, pending);
    }

    /// Whether the drop pass drops the ghost of `obj`.
    fn drop_now(&mut self, src: &S, scope: ObjId, obj: ObjId) -> bool {
        let _ = (src, scope, obj);
        false
    }
}

/// Plain Torque ghosting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TorqueGhosts;

impl GhostPolicy for TorqueGhosts {
    fn plain_query(&self) -> bool {
        true
    }
}

impl<S: ObjectSource> ObjectPolicy<S> for TorqueGhosts {}
