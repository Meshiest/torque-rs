//! A permissions model with no engine behind it.
//!
//! - A **permission** is a dotted name (`props.spawn`, `rpc.example.chat`): 1 to [`MAX_SEGMENTS`] segments of 1 to
//!   [`MAX_SEGMENT_BYTES`] bytes from `[a-z0-9_-]`.
//! - A **grant** is a permission, or a permission with a trailing wildcard: `props.*` matches every permission under `props`
//!   (one segment or more, so not `props` itself) and `*` matches all of them.
//! - A **role** is a named set of grants that a game defines ([`Roles`]); the model defines none and roles are flat (a role
//!   does not include another: a game that wants "admin implies moderator" grants both).
//! - A **subject** ([`Subject`]) is what one player holds: roles and direct grants. [`Subject::check`] is true when any grant
//!   of the subject, or of one of its roles, matches the permission.
//! - A [`Registry`] lists the permissions plugins declare they check, each with a one-line description, so a tool can show them.
//!
//! Every list is capped ([`MAX_ROLES`], [`MAX_GRANTS_PER_ROLE`], [`MAX_SUBJECT_ROLES`], [`MAX_SUBJECT_GRANTS`],
//! [`MAX_DECLARED`]) and a name or grant that does not parse is an error and never changes anything, so text from a script, a
//! config file or a peer is safe to pass in.
//!
//! ```
//! use torque_permissions::{Permission, Roles, Subject};
//!
//! let mut roles = Roles::default();
//! roles.define("builder", &["props.*", "tools.paint"])?;
//! let mut player = Subject::default();
//! player.add_role(&roles, "builder")?;
//! player.grant("chat.mute")?;
//! assert!(player.check(&roles, &Permission::parse("props.spawn")?));
//! assert!(player.check(&roles, &Permission::parse("chat.mute")?));
//! assert!(!player.check(&roles, &Permission::parse("tools.erase")?));
//! # Ok::<(), torque_permissions::PermissionError>(())
//! ```
//!
//! # Maintaining
//!
//! This crate is the model only: names, grants, roles, subjects, the registry and their caps, all in `lib.rs` (tests in
//! `tests.rs`). The engine's part (the provider slot, `Permissions`, the checks a server runs) is `torque-engine`'s
//! `permissions` module, and the script natives are `torque-server`'s `net/permissions.rs`.
//!
//! Rules a change must keep, each guarded by a test:
//! - A name or grant that does not parse changes nothing and is an error, and no text is ever trusted: wildcards only as a whole
//!   last segment of a grant, never in a permission (`names_and_grants_are_checked`).
//! - `*` and `a.*` cover one segment or more, never the bare prefix (`exact_and_wildcard_grants_match_what_they_say`).
//! - Every cap refuses with an error and leaves things as they were, and re-granting what is held is not a new grant
//!   (`every_cap_refuses_with_an_error_and_changes_nothing`).
//! - Roles are flat and never removed: a role is emptied by defining it with no grants, so a redefined name cannot hand its old
//!   holders what they never had (`roles_are_flat_named_sets_the_game_defines`).
//! - The parsers never panic and what parses parses again as itself (`the_parsers_never_panic_and_agree_with_themselves`).
//!
//! Checks: `cargo test -p torque-permissions`, with the fuzz at a million rounds as
//! `TORQUE_FUZZ_PERMISSIONS=1000000 cargo test -p torque-permissions`; then the engine's `cargo test -p torque-engine permissions`
//! and the script side's `cargo test -p torque-example-minimal-engine --test permissions`.

#![warn(missing_docs)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// The most segments in a permission or a grant (a wildcard counts as one).
pub const MAX_SEGMENTS: usize = 8;
/// The most bytes in a segment (and in a role's name).
pub const MAX_SEGMENT_BYTES: usize = 32;
/// The most roles a server defines.
pub const MAX_ROLES: usize = 256;
/// The most grants one role holds.
pub const MAX_GRANTS_PER_ROLE: usize = 64;
/// The most roles one subject holds.
pub const MAX_SUBJECT_ROLES: usize = 32;
/// The most direct grants one subject holds.
pub const MAX_SUBJECT_GRANTS: usize = 64;
/// The most permissions a registry lists.
pub const MAX_DECLARED: usize = 1_024;
/// The longest description of a declared permission.
pub const MAX_DESCRIPTION_BYTES: usize = 200;

/// Why a name, a grant or a change was refused.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum PermissionError {
    /// A name or grant with nothing in it.
    Empty,
    /// A segment that is empty, longer than [`MAX_SEGMENT_BYTES`], or has a character outside `[a-z0-9_-]`.
    Segment(String),
    /// More than [`MAX_SEGMENTS`] segments.
    TooManySegments,
    /// A `*` that is not the last whole segment, or is in a permission (only a grant may have one).
    Wildcard,
    /// A role name that is not 1 to [`MAX_SEGMENT_BYTES`] bytes of `[a-z0-9_-]`.
    RoleName(String),
    /// A role the server did not define.
    UnknownRole(String),
    /// More than [`MAX_ROLES`] roles defined.
    TooManyRoles,
    /// More than [`MAX_GRANTS_PER_ROLE`] grants in a role.
    TooManyRoleGrants,
    /// More than [`MAX_SUBJECT_ROLES`] roles on a subject.
    TooManySubjectRoles,
    /// More than [`MAX_SUBJECT_GRANTS`] direct grants on a subject.
    TooManySubjectGrants,
    /// More than [`MAX_DECLARED`] declared permissions.
    TooManyDeclared,
    /// A description over [`MAX_DESCRIPTION_BYTES`] bytes, or with a control character.
    Description,
}

impl fmt::Display for PermissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PermissionError::Empty => f.write_str("a permission name is not empty"),
            PermissionError::Segment(s) => write!(
                f,
                "`{s}` is not a segment of a permission (1 to {MAX_SEGMENT_BYTES} bytes of a-z, 0-9, `_` and `-`)"
            ),
            PermissionError::TooManySegments => {
                write!(f, "a permission has at most {MAX_SEGMENTS} segments")
            }
            PermissionError::Wildcard => {
                f.write_str("`*` is a whole last segment of a grant, and only a grant has one")
            }
            PermissionError::RoleName(n) => write!(
                f,
                "`{n}` is not a role name (1 to {MAX_SEGMENT_BYTES} bytes of a-z, 0-9, `_` and `-`)"
            ),
            PermissionError::UnknownRole(n) => write!(f, "there is no role `{n}`"),
            PermissionError::TooManyRoles => write!(f, "at most {MAX_ROLES} roles"),
            PermissionError::TooManyRoleGrants => {
                write!(f, "at most {MAX_GRANTS_PER_ROLE} grants in a role")
            }
            PermissionError::TooManySubjectRoles => {
                write!(f, "at most {MAX_SUBJECT_ROLES} roles on one player")
            }
            PermissionError::TooManySubjectGrants => {
                write!(
                    f,
                    "at most {MAX_SUBJECT_GRANTS} direct grants on one player"
                )
            }
            PermissionError::TooManyDeclared => {
                write!(f, "at most {MAX_DECLARED} declared permissions")
            }
            PermissionError::Description => write!(
                f,
                "a description is one line of at most {MAX_DESCRIPTION_BYTES} bytes"
            ),
        }
    }
}

impl std::error::Error for PermissionError {}

fn valid_segment(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_SEGMENT_BYTES
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

fn segments(text: &str) -> Result<Vec<&str>, PermissionError> {
    if text.is_empty() {
        return Err(PermissionError::Empty);
    }
    let parts: Vec<&str> = text.split('.').collect();
    if parts.len() > MAX_SEGMENTS {
        return Err(PermissionError::TooManySegments);
    }
    Ok(parts)
}

/// A permission: a validated dotted name.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Permission(String);

impl Permission {
    /// Parses `text` as a permission.
    ///
    /// # Errors
    /// A [`PermissionError`] for an empty name, a bad or missing segment, too many segments, or a wildcard.
    pub fn parse(text: &str) -> Result<Permission, PermissionError> {
        for part in segments(text)? {
            if part.contains('*') {
                return Err(PermissionError::Wildcard);
            }
            if !valid_segment(part) {
                return Err(PermissionError::Segment(part.to_string()));
            }
        }
        Ok(Permission(text.to_string()))
    }

    /// The name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A grant: a permission, or a permission prefix with a trailing wildcard (`props.*`, `*`).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Grant {
    text: String,
    /// The segments before the wildcard (all of them for an exact grant).
    prefix: Vec<String>,
    wildcard: bool,
}

impl Grant {
    /// Parses `text` as a grant.
    ///
    /// # Errors
    /// A [`PermissionError`] for an empty grant, a bad segment, too many segments, or a `*` anywhere but as the last whole
    /// segment.
    pub fn parse(text: &str) -> Result<Grant, PermissionError> {
        let parts = segments(text)?;
        let last = parts.len() - 1;
        let mut prefix = Vec::with_capacity(parts.len());
        let mut wildcard = false;
        for (i, part) in parts.iter().enumerate() {
            if *part == "*" {
                if i != last {
                    return Err(PermissionError::Wildcard);
                }
                wildcard = true;
            } else if part.contains('*') {
                return Err(PermissionError::Wildcard);
            } else if valid_segment(part) {
                prefix.push((*part).to_string());
            } else {
                return Err(PermissionError::Segment((*part).to_string()));
            }
        }
        Ok(Grant {
            text: text.to_string(),
            prefix,
            wildcard,
        })
    }

    /// The grant as written.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Whether the grant covers `permission`: an exact grant only itself; `a.b.*` every permission with more segments after
    /// `a.b`; `*` every permission.
    pub fn matches(&self, permission: &Permission) -> bool {
        let mut have = permission.0.split('.');
        for want in &self.prefix {
            if have.next() != Some(want.as_str()) {
                return false;
            }
        }
        if self.wildcard {
            have.next().is_some()
        } else {
            have.next().is_none()
        }
    }
}

impl fmt::Display for Grant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// Whether `name` is a role's name.
pub fn valid_role_name(name: &str) -> bool {
    valid_segment(name)
}

/// The roles a server defines: names and their grants. The model defines none.
#[derive(Clone, Default, Debug)]
pub struct Roles {
    defined: BTreeMap<String, Vec<Grant>>,
}

impl Roles {
    /// Defines the role `name` as `grants`, replacing a role of that name: the way to take everything from a role is to define it
    /// with no grants. A role is never removed, so one that is defined again cannot hand its old holders what they did not have
    /// before.
    ///
    /// # Errors
    /// A bad name or grant, more than [`MAX_GRANTS_PER_ROLE`] grants, or a 257th role: nothing changes.
    pub fn define(&mut self, name: &str, grants: &[&str]) -> Result<(), PermissionError> {
        if !valid_role_name(name) {
            return Err(PermissionError::RoleName(name.to_string()));
        }
        if grants.len() > MAX_GRANTS_PER_ROLE {
            return Err(PermissionError::TooManyRoleGrants);
        }
        let parsed = grants
            .iter()
            .map(|g| Grant::parse(g))
            .collect::<Result<BTreeSet<Grant>, _>>()?;
        if !self.defined.contains_key(name) && self.defined.len() >= MAX_ROLES {
            return Err(PermissionError::TooManyRoles);
        }
        self.defined
            .insert(name.to_string(), parsed.into_iter().collect());
        Ok(())
    }

    /// Defines the role `name` from `grants` written as text separated by spaces, commas or tabs (a script's or a file's form).
    ///
    /// # Errors
    /// As [`Roles::define`].
    pub fn define_text(&mut self, name: &str, grants: &str) -> Result<(), PermissionError> {
        let list: Vec<&str> = grants
            .split(|c: char| c.is_ascii_whitespace() || c == ',')
            .filter(|g| !g.is_empty())
            .collect();
        self.define(name, &list)
    }

    /// The grants of the role `name`.
    pub fn grants(&self, name: &str) -> Option<&[Grant]> {
        self.defined.get(name).map(Vec::as_slice)
    }

    /// The defined role names, sorted.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.defined.keys().map(String::as_str)
    }

    /// How many roles are defined.
    pub fn len(&self) -> usize {
        self.defined.len()
    }

    /// Whether none is.
    pub fn is_empty(&self) -> bool {
        self.defined.is_empty()
    }
}

/// What one player holds: roles (by name) and direct grants.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Subject {
    roles: BTreeSet<String>,
    grants: BTreeSet<Grant>,
}

impl Subject {
    /// Gives the subject the role `name`, which `roles` must define. True when it did not hold it already.
    ///
    /// # Errors
    /// An unknown role, or a 33rd: nothing changes.
    pub fn add_role(&mut self, roles: &Roles, name: &str) -> Result<bool, PermissionError> {
        if !valid_role_name(name) {
            return Err(PermissionError::RoleName(name.to_string()));
        }
        if roles.grants(name).is_none() {
            return Err(PermissionError::UnknownRole(name.to_string()));
        }
        if self.roles.contains(name) {
            return Ok(false);
        }
        if self.roles.len() >= MAX_SUBJECT_ROLES {
            return Err(PermissionError::TooManySubjectRoles);
        }
        Ok(self.roles.insert(name.to_string()))
    }

    /// Takes the role `name` away. True when the subject held it.
    pub fn remove_role(&mut self, name: &str) -> bool {
        self.roles.remove(name)
    }

    /// Whether the subject holds the role `name`.
    pub fn has_role(&self, name: &str) -> bool {
        self.roles.contains(name)
    }

    /// The roles held, sorted.
    pub fn roles(&self) -> impl Iterator<Item = &str> {
        self.roles.iter().map(String::as_str)
    }

    /// Gives the subject a direct grant. True when it did not hold it already.
    ///
    /// # Errors
    /// A grant that does not parse, or a 65th: nothing changes.
    pub fn grant(&mut self, grant: &str) -> Result<bool, PermissionError> {
        let grant = Grant::parse(grant)?;
        if self.grants.contains(&grant) {
            return Ok(false);
        }
        if self.grants.len() >= MAX_SUBJECT_GRANTS {
            return Err(PermissionError::TooManySubjectGrants);
        }
        Ok(self.grants.insert(grant))
    }

    /// Takes a direct grant away (a text that does not parse was never held). True when the subject held it.
    pub fn revoke(&mut self, grant: &str) -> bool {
        Grant::parse(grant).is_ok_and(|g| self.grants.remove(&g))
    }

    /// The direct grants held, sorted.
    pub fn grants(&self) -> impl Iterator<Item = &Grant> {
        self.grants.iter()
    }

    /// Whether any direct grant, or any grant of a role the subject holds (as `roles` defines it now), covers `permission`.
    pub fn check(&self, roles: &Roles, permission: &Permission) -> bool {
        self.grants.iter().any(|g| g.matches(permission))
            || self.roles.iter().any(|r| {
                roles
                    .grants(r)
                    .is_some_and(|gs| gs.iter().any(|g| g.matches(permission)))
            })
    }

    /// [`Subject::check`] of a permission given as text; text that is not a permission is not held.
    pub fn check_text(&self, roles: &Roles, permission: &str) -> bool {
        Permission::parse(permission).is_ok_and(|p| self.check(roles, &p))
    }
}

/// The permissions plugins declared they check, each with a one-line description.
#[derive(Clone, Default, Debug)]
pub struct Registry {
    declared: BTreeMap<Permission, String>,
}

impl Registry {
    /// Declares `permission` with `description` (declaring it again replaces the description).
    ///
    /// # Errors
    /// A name that is not a permission, a description that is not one short line, or a 1,025th permission.
    pub fn declare(&mut self, permission: &str, description: &str) -> Result<(), PermissionError> {
        let permission = Permission::parse(permission)?;
        if description.len() > MAX_DESCRIPTION_BYTES || description.chars().any(char::is_control) {
            return Err(PermissionError::Description);
        }
        if !self.declared.contains_key(&permission) && self.declared.len() >= MAX_DECLARED {
            return Err(PermissionError::TooManyDeclared);
        }
        self.declared.insert(permission, description.to_string());
        Ok(())
    }

    /// Whether `permission` was declared.
    pub fn contains(&self, permission: &Permission) -> bool {
        self.declared.contains_key(permission)
    }

    /// The description of `permission`, if declared.
    pub fn description(&self, permission: &Permission) -> Option<&str> {
        self.declared.get(permission).map(String::as_str)
    }

    /// Every declared permission with its description, sorted by name.
    pub fn iter(&self) -> impl Iterator<Item = (&Permission, &str)> {
        self.declared.iter().map(|(p, d)| (p, d.as_str()))
    }

    /// How many are declared.
    pub fn len(&self) -> usize {
        self.declared.len()
    }

    /// Whether none is.
    pub fn is_empty(&self) -> bool {
        self.declared.is_empty()
    }
}

#[cfg(test)]
mod tests;
