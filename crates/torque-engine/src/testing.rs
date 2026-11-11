//! Helpers for the tests of crates that build engines. Behind the `testing` feature, which only dev-dependencies enable.

use crate::{EngineBuilder, Plugin, PluginId, PluginType};

/// A plugin that runs a closure as its `build`, under an id of its own (the type's [`PluginType::ID`] only names it for the
/// engine's typed calls, so an engine takes it by value and by the instance's id).
pub struct ClosurePlugin {
    id: &'static str,
    build: Box<dyn Fn(&mut EngineBuilder)>,
}

impl ClosurePlugin {
    /// A plugin with `id` whose `build` calls `build`.
    #[must_use]
    pub fn new(id: &'static str, build: impl Fn(&mut EngineBuilder) + 'static) -> Self {
        Self {
            id,
            build: Box::new(build),
        }
    }
}

impl Plugin for ClosurePlugin {
    fn id(&self) -> Option<&str> {
        Some(self.id)
    }

    fn build(&self, engine: &mut EngineBuilder) {
        (self.build)(engine);
    }
}

impl PluginType for ClosurePlugin {
    const ID: PluginId = PluginId::new("test.closure");
}
