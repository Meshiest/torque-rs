//! Test doubles for the host's side of the ghost layer, for this crate's tests and for the tests of crates that pack or
//! decode records (the `testing` feature; only dev-dependencies turn it on).

use std::collections::HashMap;

use crate::{Link, NetStr, ObjectView, Value};

/// An [`ObjectView`] backed by maps; anything not in it is `Null`, no link, no net string and no shape instance.
#[derive(Debug, Clone, Default)]
pub struct MapView {
    /// Field values by name and element index.
    pub vals: HashMap<(String, usize), Value>,
    /// Pointer fields as the connection resolves them.
    pub links: HashMap<String, Link>,
    /// Net string fields by name and element index.
    pub nets: HashMap<(String, usize), NetStr>,
    /// The shape instance's mesh object count.
    pub meshes: Option<usize>,
}

impl MapView {
    /// This view with `name[index]` set to `v`.
    #[must_use]
    pub fn with(mut self, name: &str, index: usize, v: Value) -> MapView {
        self.set(name, index, v);
        self
    }

    /// This view with `name[0..]` set to the floats `xs`.
    #[must_use]
    pub fn floats(mut self, name: &str, xs: &[f32]) -> MapView {
        for (i, x) in xs.iter().enumerate() {
            self.set(name, i, Value::F32(*x));
        }
        self
    }

    /// This view with the pointer field `name` resolved to `link`.
    #[must_use]
    pub fn linked(mut self, name: &str, link: Link) -> MapView {
        self.links.insert(name.to_string(), link);
        self
    }

    /// Sets `name[index]` to `v`.
    pub fn set(&mut self, name: &str, index: usize, v: Value) {
        self.vals.insert((name.to_string(), index), v);
    }
}

impl ObjectView for MapView {
    fn get(&self, field: &str, index: usize) -> Value {
        self.vals
            .get(&(field.to_string(), index))
            .cloned()
            .unwrap_or(Value::Null)
    }

    fn link(&self, field: &str) -> Link {
        self.links.get(field).copied().unwrap_or(Link::Null)
    }

    fn net_string(&self, field: &str, index: usize) -> NetStr {
        self.nets
            .get(&(field.to_string(), index))
            .cloned()
            .unwrap_or(NetStr::Null)
    }

    fn mesh_objects(&self) -> Option<usize> {
        self.meshes
    }
}
