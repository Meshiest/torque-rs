//! The rigid-body solver seam: what a vehicle's sub-step hands to whatever resolves its body against the world and moves it.
//!
//! A sub-step is the class's forces (tires, springs, wheel rays, jets, the container's drag; they stay in [`crate::wheeled`] and
//! [`crate::flying`], and fold into the rigid's momenta as before), then the solver's part ([`BodySolver::substep`]: the body
//! against the working set, the contact responses, the rest test, the integration), then the impact test, the scan and the
//! liquid callbacks (also the class's). The solver is picked per datablock through the host ([`VehicleHost::body_solver`]); with
//! none picked the vehicle's own [`Body`] runs, which is TGE's code, unchanged.
//!
//! A solver owns no truth: the [`VehicleState`]'s `Rigid` is the state, the ghost records and control words carry it, and a client
//! restores a vehicle from the server's words, so a solver keeps nothing that has to survive a tick. It is made when a tick
//! begins (by the host) and dropped when it ends.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/vehicles/vehicle.cpp` (Copyright (c)
//! 2012 GarageGames, LLC); the code here is this crate's own.

use torque_collide::Exhausted;

use crate::data::VehicleData;
use crate::host::VehicleHost;
use crate::state::{self, Body, VehicleState};

/// The call a solver makes when its body touched something, between the collision response and the contact forces: the class's
/// extension point ([`VehicleHost::wheeled_body_contact`] for a WheeledVehicle). A solver with no such point of its own calls it
/// once per sub-step in which the body touched, after the collision and before the rest test.
pub type ContactHook<'a> = dyn FnMut(&mut VehicleState, &mut dyn VehicleHost) + 'a;

/// What resolves a vehicle's body against the world and integrates it.
///
/// Every method may report [`Exhausted`] when the work budget ([`torque_collide::World::charge`]) is spent; the tick stops where it
/// was, and the state is as far as it got.
pub trait BodySolver: std::fmt::Debug {
    /// The start of a tick, before its first sub-step: the working set round the body (`updateWorkingCollisionSet`).
    fn begin_tick(&mut self, s: &VehicleState, host: &mut dyn VehicleHost)
    -> Result<(), Exhausted>;

    /// One sub-step of `dt` after the class's forces: the body's collision and contact response, the rest test and the
    /// integration, writing the result into `s.rigid` (and `s.rest_count`). A body already at rest is neither resolved nor
    /// moved. `on_contact` is the class's hook (none for a class without one). True when the body touched something
    /// (`resolveCollision` hit).
    fn substep(
        &mut self,
        s: &mut VehicleState,
        db: &VehicleData,
        dt: f32,
        host: &mut dyn VehicleHost,
        on_contact: Option<&mut ContactHook<'_>>,
    ) -> Result<bool, Exhausted>;

    /// The end of a tick, after its last sub-step (also after one cut short by the budget): release what `begin_tick` took.
    /// Nothing by default.
    fn end_tick(
        &mut self,
        s: &mut VehicleState,
        host: &mut dyn VehicleHost,
    ) -> Result<(), Exhausted> {
        let _ = (s, host);
        Ok(())
    }

    /// A copy of this solver, for a vehicle that is cloned mid-tick.
    fn clone_box(&self) -> Box<dyn BodySolver>;
}

impl Clone for Box<dyn BodySolver> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

impl Body {
    /// The body's collision and contact response of one sub-step: the body collision, `resolveCollision`, the class's hook when
    /// the body touched, `resolveContacts`, then [`Body::hold_out`]. True when the body touched.
    pub fn contacts(
        &mut self,
        s: &mut VehicleState,
        db: &VehicleData,
        dt: f32,
        host: &mut dyn VehicleHost,
        on_contact: Option<&mut ContactHook<'_>>,
    ) -> bool {
        self.collide(s, db, host);
        let touched = state::resolve_collision(s, &self.list, db, host);
        if touched && let Some(hook) = on_contact {
            hook(s, host);
        }
        state::resolve_contacts(s, &self.list, db, dt);
        self.hold_out(s, db, host);
        touched
    }
}

/// TGE's solver: the vehicle's own [`Body`] (its working set, GJK states and collision list), and the rules of `updatePos`.
impl BodySolver for Body {
    fn begin_tick(
        &mut self,
        s: &VehicleState,
        host: &mut dyn VehicleHost,
    ) -> Result<(), Exhausted> {
        self.update_working_set(s, host)
    }

    #[inline]
    fn substep(
        &mut self,
        s: &mut VehicleState,
        db: &VehicleData,
        dt: f32,
        host: &mut dyn VehicleHost,
        on_contact: Option<&mut ContactHook<'_>>,
    ) -> Result<bool, Exhausted> {
        let mut touched = false;
        if !s.rigid.at_rest {
            touched = self.contacts(s, db, dt, host, on_contact);
            state::rest_test(s, self.list.count == 0, dt);
        }
        if !s.rigid.at_rest {
            s.rigid.integrate(dt);
        }
        Ok(touched)
    }

    fn clone_box(&self) -> Box<dyn BodySolver> {
        Box::new(self.clone())
    }
}

/// The solver a vehicle's sub-step runs: the host's, or the vehicle's own [`Body`] (a direct call, not a trait-object one).
#[inline]
pub(crate) fn substep(
    custom: &mut Option<Box<dyn BodySolver>>,
    body: &mut Body,
    s: &mut VehicleState,
    db: &VehicleData,
    dt: f32,
    host: &mut dyn VehicleHost,
    on_contact: Option<&mut ContactHook<'_>>,
) -> Result<bool, Exhausted> {
    match custom {
        Some(solver) => solver.substep(s, db, dt, host, on_contact),
        None => body.substep(s, db, dt, host, on_contact),
    }
}
