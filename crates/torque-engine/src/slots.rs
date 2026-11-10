//! The typed stores behind [`EngineBuilder`](crate::EngineBuilder) and [`Engine`](crate::Engine): resources, exclusive slots and
//! additive lists, each keyed by its type.
//!
//! The maps are only ever looked up by key, never iterated (but to move the resources into their cells, which is
//! order-free), so their order cannot leak into the engine's behavior.

use std::any::{Any, TypeId, type_name};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use crate::PluginId;
use crate::param::{Res, ResMut, ResourceError};

/// A resource while the engine is built: the value itself, moved into a [`RefCell`] when the engine is built.
trait Resource {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    /// The value in its cell: a `RefCell<T>`.
    fn into_cell(self: Box<Self>) -> Box<dyn Any>;
}

impl<T: Any> Resource for T {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn into_cell(self: Box<Self>) -> Box<dyn Any> {
        Box::new(RefCell::new(*self))
    }
}

/// The built engine's resources, each in a `RefCell<T>`: borrowed by [`Res`] and [`ResMut`], checked at run time.
#[derive(Default)]
pub(crate) struct Resources(BTreeMap<TypeId, Box<dyn Any>>);

impl Resources {
    fn cell<T: Any>(&self) -> Result<&RefCell<T>, ResourceError> {
        self.0
            .get(&TypeId::of::<T>())
            .and_then(|cell| cell.downcast_ref())
            .ok_or(ResourceError::Missing {
                resource: type_name::<T>(),
            })
    }

    pub(crate) fn get<T: Any>(&self) -> Result<Res<'_, T>, ResourceError> {
        let cell = self.cell::<T>()?;
        cell.try_borrow()
            .map(Res::new)
            .map_err(|_| ResourceError::BorrowedMut {
                resource: type_name::<T>(),
            })
    }

    pub(crate) fn get_mut<T: Any>(&self) -> Result<ResMut<'_, T>, ResourceError> {
        let cell = self.cell::<T>()?;
        cell.try_borrow_mut()
            .map(ResMut::new)
            .map_err(|_| ResourceError::Borrowed {
                resource: type_name::<T>(),
            })
    }
}

/// An exclusive slot's value, with the plugin that set it.
struct SlotEntry {
    owner: PluginId,
    /// An `Rc<T>`, for the slot's type `T`.
    value: Box<dyn Any>,
}

/// A slot set by a second plugin: the slot's type and the plugin that set it first.
pub(crate) struct Taken {
    pub(crate) slot: &'static str,
    pub(crate) first: PluginId,
}

#[derive(Default)]
pub(crate) struct Store {
    resources: BTreeMap<TypeId, Box<dyn Resource>>,
    slots: BTreeMap<TypeId, SlotEntry>,
    /// A `Vec<T>` per item type `T`.
    lists: BTreeMap<TypeId, Box<dyn Any>>,
}

impl Store {
    pub(crate) fn insert_resource<T: Any>(&mut self, value: T) {
        self.resources.insert(TypeId::of::<T>(), Box::new(value));
    }

    pub(crate) fn init_resource<T: Any + Default>(&mut self) {
        self.resources
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(T::default()));
    }

    pub(crate) fn resource<T: Any>(&self) -> Option<&T> {
        // Through the box: `Box<dyn Resource>` is a `Resource` itself.
        let value: &dyn Resource = &**self.resources.get(&TypeId::of::<T>())?;
        value.as_any().downcast_ref()
    }

    pub(crate) fn resource_mut<T: Any>(&mut self) -> Option<&mut T> {
        let value: &mut dyn Resource = &mut **self.resources.get_mut(&TypeId::of::<T>())?;
        value.as_any_mut().downcast_mut()
    }

    /// Moves the resources out into their cells, for the built engine.
    pub(crate) fn take_resources(&mut self) -> Resources {
        let resources = std::mem::take(&mut self.resources);
        Resources(
            resources
                .into_iter()
                .map(|(key, value)| (key, value.into_cell()))
                .collect(),
        )
    }

    /// Sets the slot of type `T`, unless a plugin already did.
    pub(crate) fn set_slot<T: ?Sized + 'static>(
        &mut self,
        owner: PluginId,
        value: Rc<T>,
    ) -> Result<(), Taken> {
        let key = TypeId::of::<T>();
        if let Some(entry) = self.slots.get(&key) {
            return Err(Taken {
                slot: type_name::<T>(),
                first: entry.owner,
            });
        }
        self.slots.insert(
            key,
            SlotEntry {
                owner,
                value: Box::new(value),
            },
        );
        Ok(())
    }

    pub(crate) fn slot<T: ?Sized + 'static>(&self) -> Option<Rc<T>> {
        let entry = self.slots.get(&TypeId::of::<T>())?;
        entry.value.downcast_ref::<Rc<T>>().cloned()
    }

    pub(crate) fn push<T: Any>(&mut self, item: T) {
        self.lists
            .entry(TypeId::of::<Vec<T>>())
            .or_insert_with(|| Box::new(Vec::<T>::new()))
            .downcast_mut::<Vec<T>>()
            .expect("a list is stored under its own type")
            .push(item);
    }

    pub(crate) fn list<T: Any>(&self) -> &[T] {
        self.lists
            .get(&TypeId::of::<Vec<T>>())
            .and_then(|list| list.downcast_ref::<Vec<T>>())
            .map_or(&[], Vec::as_slice)
    }
}
