//! A subclass of a vehicle, Torque's way of adding to a class: the fields it adds to each object ([`VehicleFields`],
//! held in [`VehicleState::ext`](crate::state::VehicleState::ext)) and, for the WheeledVehicle, the overrides of the step its
//! class makes ([`crate::wheeled::WheeledClass`], which the host hands the step through
//! [`VehicleHost::wheeled_class`](crate::host::VehicleHost::wheeled_class)).
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/vehicles/wheeledVehicle.cpp`
//! (Copyright (c) 2012 GarageGames, LLC); the code here is this crate's own.

use std::any::Any;
use std::fmt::Debug;

use crate::state::VehicleState;

/// The fields a subclass adds to each vehicle. They travel with the state: cloned, compared and printed with it. Any
/// `Clone + PartialEq + Debug` type is one; the subclass reads its own back with [`VehicleState::fields`] and
/// [`VehicleState::fields_mut`].
pub trait VehicleFields: Any + Debug {
    fn clone_box(&self) -> Box<dyn VehicleFields>;
    fn eq_fields(&self, other: &dyn VehicleFields) -> bool;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: Any + Debug + Clone + PartialEq> VehicleFields for T {
    fn clone_box(&self) -> Box<dyn VehicleFields> {
        Box::new(self.clone())
    }
    fn eq_fields(&self, other: &dyn VehicleFields) -> bool {
        other.as_any().downcast_ref::<T>() == Some(self)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl Clone for Box<dyn VehicleFields> {
    fn clone(&self) -> Self {
        (**self).clone_box()
    }
}

impl PartialEq for Box<dyn VehicleFields> {
    fn eq(&self, other: &Self) -> bool {
        (**self).eq_fields(&**other)
    }
}

impl VehicleState {
    /// The subclass's fields of type `T`, if this vehicle has them.
    pub fn fields<T: Any>(&self) -> Option<&T> {
        self.ext.as_deref()?.as_any().downcast_ref()
    }

    /// The subclass's fields of type `T`, mutably.
    pub fn fields_mut<T: Any>(&mut self) -> Option<&mut T> {
        self.ext.as_deref_mut()?.as_any_mut().downcast_mut()
    }
}
