//! Permissions on a server: the roles a game defines, the permissions plugins declare they check, and who holds what.
//!
//! The model (names, wildcard grants, flat roles, the caps) is [`torque_permissions`]. This module is the engine's part:
//!
//! - [`EngineBuilder::permissions`] is where a plugin declares the permissions it checks (with a line of description, so a tool
//!   can list them), defines roles in Rust, and may set the [`PermissionProvider`] (a slot: one plugin sets it).
//! - A [`PermissionProvider`] keeps what each player holds ([`Subject`]), keyed by [`SubjectKey`]: the auth identity of a signed-in
//!   player, else the connection. The default, [`MemoryPermissions`], keeps it in memory; a game with accounts backs it with its
//!   database. Connection-keyed grants are forgotten when the connection ends ([`Permissions::forget_connection`]);
//!   identity-keyed ones are the provider's business.
//! - [`Permissions`] is what a server runs: the roles and the registry (a script may add to both), the provider, and the checks.
//!
//! Everything is on the server. A client never sets its own permissions and none is sent to a client unless a game sends it.
//!
//! ```
//! use torque_engine::permissions::{Permissions, SubjectKey};
//! use torque_engine::{EngineBuilder, Plugin, PluginId};
//!
//! #[derive(PluginId)]
//! #[plugin(id = "example.props")]
//! struct Props;
//!
//! impl Plugin for Props {
//!     fn build(&self, engine: &mut EngineBuilder) {
//!         let mut perms = engine.permissions();
//!         perms.declare("props.spawn", "Spawn a prop");
//!         perms.role("builder", &["props.*"]);
//!     }
//! }
//!
//! let engine = EngineBuilder::new().add_plugins(Props).build()?;
//! let perms = Permissions::new(&engine)?;
//! let player = SubjectKey::Connection(7);
//! assert!(!perms.check(&player, "props.spawn"));
//! perms.add_role(&player, "builder")?;
//! assert!(perms.check(&player, "props.spawn"));
//! perms.forget_connection(7);
//! assert!(!perms.check(&player, "props.spawn"));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

pub use torque_permissions::PermissionError;
use torque_permissions::{Permission, Registry, Roles, Subject};

use crate::{BuildError, Engine, EngineBuilder, PluginId};

/// Who a [`Subject`] is kept for: a signed-in player's auth identity, or the connection when there is none.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum SubjectKey {
    /// The identity the auth provider verified.
    Identity(Vec<u8>),
    /// A connection (what a player with no identity is held by; forgotten when it ends).
    Connection(u32),
}

/// Where what each player holds is kept. A game backs it with its auth or database; the methods take `&self` so a provider
/// owns its own interior state (a connection pool, a cache).
pub trait PermissionProvider {
    /// What `key` holds now (nothing for a key the provider has not seen).
    fn load(&self, key: &SubjectKey) -> Subject;

    /// Keeps what `key` holds.
    fn store(&self, key: &SubjectKey, subject: Subject);

    /// A connection ended: forget what was kept for [`SubjectKey::Connection`]`(conn)`. A provider keeps identity-keyed data as
    /// long as it likes.
    fn forget_connection(&self, conn: u32);
}

/// The default provider: everything in memory, gone when the server stops, connection and identity keys alike.
#[derive(Default)]
pub struct MemoryPermissions {
    held: RefCell<HashMap<SubjectKey, Subject>>,
}

impl PermissionProvider for MemoryPermissions {
    fn load(&self, key: &SubjectKey) -> Subject {
        self.held.borrow().get(key).cloned().unwrap_or_default()
    }

    fn store(&self, key: &SubjectKey, subject: Subject) {
        self.held.borrow_mut().insert(key.clone(), subject);
    }

    fn forget_connection(&self, conn: u32) {
        self.held.borrow_mut().remove(&SubjectKey::Connection(conn));
    }
}

/// The provider slot's value.
struct PermissionsSlot(Rc<dyn PermissionProvider>);

/// A permission a plugin declared it checks.
#[derive(Clone, Debug)]
pub struct DeclaredPermission {
    /// The plugin that declared it.
    pub plugin: PluginId,
    /// The permission's name.
    pub name: String,
    /// One line on what it allows.
    pub description: String,
}

/// A role a plugin defined.
#[derive(Clone, Debug)]
pub struct RoleDef {
    /// The plugin that defined it.
    pub plugin: PluginId,
    /// The role's name.
    pub name: String,
    /// Its grants.
    pub grants: Vec<String>,
}

impl EngineBuilder {
    /// Declares permissions, defines roles and sets the provider ([module docs](crate::permissions)).
    pub fn permissions(&mut self) -> PermissionsRegistry<'_> {
        PermissionsRegistry { engine: self }
    }
}

/// Adds to what an engine being built says about permissions ([`EngineBuilder::permissions`]).
pub struct PermissionsRegistry<'a> {
    engine: &'a mut EngineBuilder,
}

impl PermissionsRegistry<'_> {
    fn reject(&mut self, error: PermissionError) {
        let plugin = self.engine.current_plugin();
        self.engine.fail(BuildError::Rejected {
            plugin,
            message: error.to_string(),
        });
    }

    /// Declares that the plugin checks `permission` (a dotted name), described in one line. A check of a permission nobody
    /// declared is a bug the server reports.
    pub fn declare(&mut self, permission: &str, description: &str) -> &mut Self {
        let mut probe = Registry::default();
        match probe.declare(permission, description) {
            Ok(()) => {
                let plugin = self.engine.current_plugin();
                self.engine.push(DeclaredPermission {
                    plugin,
                    name: permission.to_string(),
                    description: description.to_string(),
                });
            }
            Err(error) => self.reject(error),
        }
        self
    }

    /// Defines the role `name` as `grants` (permissions and trailing-wildcard grants). The engine defines no roles: a game
    /// does, here, in script (`definePermissionRole`) or from a file. A second definition of a name replaces the first.
    pub fn role(&mut self, name: &str, grants: &[&str]) -> &mut Self {
        let mut probe = Roles::default();
        match probe.define(name, grants) {
            Ok(()) => {
                let plugin = self.engine.current_plugin();
                self.engine.push(RoleDef {
                    plugin,
                    name: name.to_string(),
                    grants: grants.iter().map(|g| (*g).to_string()).collect(),
                });
            }
            Err(error) => self.reject(error),
        }
        self
    }

    /// Sets the provider that keeps what players hold. One plugin sets it; a second fails the build.
    pub fn provider<P: PermissionProvider + 'static>(&mut self, provider: P) -> &mut Self {
        self.engine
            .set_slot(Rc::new(PermissionsSlot(Rc::new(provider))));
        self
    }

    /// Whether `permission` was declared so far (by this or another plugin).
    pub fn is_declared(&self, permission: &str) -> bool {
        self.engine
            .list::<DeclaredPermission>()
            .iter()
            .any(|d| d.name == permission)
    }
}

impl Engine {
    /// The permissions plugins declared, in the order they did.
    pub fn declared_permissions(&self) -> &[DeclaredPermission] {
        self.list::<DeclaredPermission>()
    }

    /// The provider a plugin set, else a new in-memory one (each server has its own).
    pub fn permission_provider(&self) -> Rc<dyn PermissionProvider> {
        self.slot::<PermissionsSlot>().map_or_else(
            || Rc::new(MemoryPermissions::default()) as Rc<dyn PermissionProvider>,
            |slot| Rc::clone(&slot.0),
        )
    }
}

/// What a call may ask about the permissions of the player behind it.
pub trait PermissionView {
    /// Whether the player holds `permission` (by a direct grant or one of their roles' grants).
    fn has_permission(&self, permission: &str) -> bool;
}

/// A view of a player who holds nothing (a client's: clients have no permissions of their own).
pub struct NoPermissions;

impl PermissionView for NoPermissions {
    fn has_permission(&self, _permission: &str) -> bool {
        false
    }
}

/// The most undeclared permission names kept between two reports.
pub const MAX_UNDECLARED: usize = 256;

/// The undeclared permissions that were checked ([`Permissions::take_undeclared`]).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Undeclared {
    /// The first [`MAX_UNDECLARED`] names, sorted.
    pub names: Vec<String>,
    /// How many checks of names past those there were (a name past the cap is not remembered, so each check counts).
    pub more: usize,
}

impl Undeclared {
    /// Whether nothing undeclared was checked.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty() && self.more == 0
    }

    /// The warnings to print: one for each name, and one for the rest.
    pub fn lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .names
            .iter()
            .map(|name| format!("the permission {name} is checked but no plugin declared it"))
            .collect();
        if self.more > 0 {
            lines.push(format!(
                "and {} more checks of undeclared permissions",
                self.more
            ));
        }
        lines
    }
}

/// What a server runs: the roles and the registry (plugins' and the game's, a script may add to both), the provider, and the
/// checks ([module docs](crate::permissions)).
pub struct Permissions {
    roles: RefCell<Roles>,
    registry: RefCell<Registry>,
    provider: Rc<dyn PermissionProvider>,
    undeclared: RefCell<BTreeSet<String>>,
    undeclared_more: std::cell::Cell<usize>,
}

impl Permissions {
    /// The permissions of a server made by `engine`: what its plugins declared and defined, and its provider.
    ///
    /// # Errors
    /// A role or declaration the server-wide caps refuse (the plugins' own were checked when they made them).
    pub fn new(engine: &Engine) -> Result<Permissions, PermissionError> {
        let mut roles = Roles::default();
        for def in engine.list::<RoleDef>() {
            let grants: Vec<&str> = def.grants.iter().map(String::as_str).collect();
            roles.define(&def.name, &grants)?;
        }
        let mut registry = Registry::default();
        for d in engine.declared_permissions() {
            registry.declare(&d.name, &d.description)?;
        }
        Ok(Permissions {
            roles: RefCell::new(roles),
            registry: RefCell::new(registry),
            provider: engine.permission_provider(),
            undeclared: RefCell::new(BTreeSet::new()),
            undeclared_more: std::cell::Cell::new(0),
        })
    }

    /// Defines (or redefines) the role `name` from grants written as text separated by spaces or commas.
    ///
    /// # Errors
    /// A bad name or grant, or a cap: nothing changes.
    pub fn define_role(&self, name: &str, grants: &str) -> Result<(), PermissionError> {
        self.roles.borrow_mut().define_text(name, grants)
    }

    /// Declares a permission a script's code checks.
    ///
    /// # Errors
    /// A bad name or description, or the cap.
    pub fn declare(&self, permission: &str, description: &str) -> Result<(), PermissionError> {
        self.registry.borrow_mut().declare(permission, description)
    }

    /// The declared permissions with their descriptions, sorted by name (what a tool lists).
    pub fn declared(&self) -> Vec<(String, String)> {
        self.registry
            .borrow()
            .iter()
            .map(|(p, d)| (p.as_str().to_string(), d.to_string()))
            .collect()
    }

    /// The defined role names, sorted.
    pub fn role_names(&self) -> Vec<String> {
        self.roles.borrow().names().map(str::to_string).collect()
    }

    fn change<R>(
        &self,
        key: &SubjectKey,
        f: impl FnOnce(&Roles, &mut Subject) -> Result<R, PermissionError>,
    ) -> Result<R, PermissionError> {
        let mut subject = self.provider.load(key);
        let out = f(&self.roles.borrow(), &mut subject)?;
        self.provider.store(key, subject);
        Ok(out)
    }

    /// Gives `key` the role `name` (which must be defined). True when it did not hold it.
    ///
    /// # Errors
    /// An unknown role, or the cap on a player's roles.
    pub fn add_role(&self, key: &SubjectKey, name: &str) -> Result<bool, PermissionError> {
        self.change(key, |roles, s| s.add_role(roles, name))
    }

    /// Takes the role `name` from `key`. True when it was held.
    pub fn remove_role(&self, key: &SubjectKey, name: &str) -> bool {
        self.change(key, |_, s| Ok(s.remove_role(name)))
            .unwrap_or(false)
    }

    /// Whether `key` holds the role `name`.
    pub fn has_role(&self, key: &SubjectKey, name: &str) -> bool {
        self.provider.load(key).has_role(name)
    }

    /// The roles `key` holds, sorted.
    pub fn roles_of(&self, key: &SubjectKey) -> Vec<String> {
        self.provider
            .load(key)
            .roles()
            .map(str::to_string)
            .collect()
    }

    /// Gives `key` a direct grant. True when it did not hold it.
    ///
    /// # Errors
    /// A grant that does not parse, or the cap on a player's direct grants.
    pub fn grant(&self, key: &SubjectKey, grant: &str) -> Result<bool, PermissionError> {
        self.change(key, |_, s| s.grant(grant))
    }

    /// Takes a direct grant from `key`. True when it was held.
    pub fn revoke(&self, key: &SubjectKey, grant: &str) -> bool {
        self.change(key, |_, s| Ok(s.revoke(grant)))
            .unwrap_or(false)
    }

    /// The direct grants `key` holds, sorted.
    pub fn grants_of(&self, key: &SubjectKey) -> Vec<String> {
        self.provider
            .load(key)
            .grants()
            .map(|g| g.as_str().to_string())
            .collect()
    }

    /// Whether `key` holds `permission`. A permission nobody declared is a plugin's bug, reported as a warning in every build
    /// ([`take_undeclared`](Self::take_undeclared)); the check still answers. (It is never a panic: a name built from what a peer
    /// sent is as likely to be undeclared as a typo is.) Text that is not a permission is never held.
    pub fn check(&self, key: &SubjectKey, permission: &str) -> bool {
        let Ok(parsed) = Permission::parse(permission) else {
            return false;
        };
        if !self.registry.borrow().contains(&parsed) {
            self.note_undeclared(permission);
        }
        self.holds(key, permission)
    }

    /// Whether `key` holds `permission`, declared or not (what a script's `hasPermission` asks: a typo there is an answer, not a
    /// plugin's bug). Text that is not a permission is never held.
    pub fn holds(&self, key: &SubjectKey, permission: &str) -> bool {
        let Ok(parsed) = Permission::parse(permission) else {
            return false;
        };
        self.provider.load(key).check(&self.roles.borrow(), &parsed)
    }

    /// Remembers an undeclared permission that was checked: the first [`MAX_UNDECLARED`] names, then only how many checks more (a peer
    /// that makes names up cannot make the server hold them all).
    fn note_undeclared(&self, permission: &str) {
        let mut names = self.undeclared.borrow_mut();
        if names.contains(permission) {
            return;
        }
        if names.len() < MAX_UNDECLARED {
            names.insert(permission.to_string());
        } else {
            self.undeclared_more
                .set(self.undeclared_more.get().saturating_add(1));
        }
    }

    /// The undeclared permissions that were checked since the last call, for the host to warn about once.
    pub fn take_undeclared(&self) -> Undeclared {
        Undeclared {
            names: std::mem::take(&mut *self.undeclared.borrow_mut())
                .into_iter()
                .collect(),
            more: self.undeclared_more.replace(0),
        }
    }

    /// A connection ended: connection-keyed grants go ([`PermissionProvider::forget_connection`]).
    pub fn forget_connection(&self, conn: u32) {
        self.provider.forget_connection(conn);
    }

    /// A view of what `key` holds, for a call from that player.
    pub fn view(&self, key: SubjectKey) -> KeyView<'_> {
        KeyView { perms: self, key }
    }
}

/// What one player holds, as a call asks ([`Permissions::view`]).
pub struct KeyView<'a> {
    perms: &'a Permissions,
    key: SubjectKey,
}

impl PermissionView for KeyView<'_> {
    fn has_permission(&self, permission: &str) -> bool {
        self.perms.check(&self.key, permission)
    }
}

#[cfg(test)]
#[path = "permissions_tests.rs"]
mod tests;
