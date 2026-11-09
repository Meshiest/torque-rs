//! What a native does beyond its result: the effects it declares when it is registered ([`Vm::add_native_effects`]), which
//! a host's call policy reads to decide what code it does not trust may call (the extensions' script languages, say).
//!
//! The effects of a name are kept per script engine and only ever grow: a native registered again under the same name
//! (a host replacing it for its own objects) keeps the effects declared before, and a name's effects are the union of every
//! namespace's native of that name ([`Vm::effects_of_name`]), as a policy that checks by name sees them.

use std::collections::HashMap;
use std::fmt;
use std::ops::{BitOr, BitOrAssign};

use crate::text::fold;
use crate::vm::Vm;

/// A set of effects a native has. [`Effects::NONE`] for one that only computes, reads or changes objects and variables.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Effects(u8);

impl Effects {
    /// No effect beyond its result and the objects and variables it changes.
    pub const NONE: Effects = Effects(0);
    /// It opens, writes, deletes or exports files.
    pub const FILES: Effects = Effects(1);
    /// It runs code from text, or a function or method it names where a caller's policy does not see which.
    pub const CODE: Effects = Effects(1 << 1);
    /// It changes what the engine runs as a whole: switches or loads mods, deletes every datablock or variable, quits.
    pub const ENGINE: Effects = Effects(1 << 2);
    /// It connects to, or sends to, an address the caller names.
    pub const NETWORK: Effects = Effects(1 << 3);
    /// It changes who may do what: grants, revokes or defines permissions and roles.
    pub const PERMISSIONS: Effects = Effects(1 << 4);
    /// It writes a variable whose name one of its arguments gives (`nextToken`): a policy checks that write as it checks a
    /// write of the variable itself, taking any of the arguments for the name.
    pub const VARIABLES: Effects = Effects(1 << 5);

    /// Every effect with its name, in the order [`Display`](fmt::Display) writes them.
    pub const NAMED: [(Effects, &'static str); 6] = [
        (Effects::FILES, "files"),
        (Effects::CODE, "code"),
        (Effects::ENGINE, "engine"),
        (Effects::NETWORK, "network"),
        (Effects::PERMISSIONS, "permissions"),
        (Effects::VARIABLES, "variables"),
    ];

    /// Both sets.
    #[must_use]
    pub const fn union(self, other: Effects) -> Effects {
        Effects(self.0 | other.0)
    }

    /// The effects in both sets.
    #[must_use]
    pub const fn intersection(self, other: Effects) -> Effects {
        Effects(self.0 & other.0)
    }

    /// Whether every effect of `other` is in this set.
    #[must_use]
    pub const fn contains(self, other: Effects) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether the two sets share an effect.
    #[must_use]
    pub const fn intersects(self, other: Effects) -> bool {
        self.0 & other.0 != 0
    }

    /// Whether it has no effect.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The set the names joined with `+` say (`files+code`), or `-` for none. `None` for a name that is not an effect's.
    pub fn parse(text: &str) -> Option<Effects> {
        if text == "-" {
            return Some(Effects::NONE);
        }
        text.split('+').try_fold(Effects::NONE, |set, name| {
            let (effect, _) = Effects::NAMED.iter().find(|(_, n)| *n == name)?;
            Some(set | *effect)
        })
    }
}

impl BitOr for Effects {
    type Output = Effects;

    fn bitor(self, other: Effects) -> Effects {
        self.union(other)
    }
}

impl BitOrAssign for Effects {
    fn bitor_assign(&mut self, other: Effects) {
        *self = self.union(other);
    }
}

/// The names joined with `+`, or `-` for none: what [`Effects::parse`] reads.
impl fmt::Display for Effects {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return f.write_str("-");
        }
        let mut first = true;
        for (effect, name) in Effects::NAMED {
            if self.contains(effect) {
                if !first {
                    f.write_str("+")?;
                }
                f.write_str(name)?;
                first = false;
            }
        }
        Ok(())
    }
}

/// The effects declared in one script engine: by folded `namespace::name`, and by folded name across namespaces.
#[derive(Default)]
pub(crate) struct EffectTable {
    natives: HashMap<Vec<u8>, Effects>,
    names: HashMap<Vec<u8>, Effects>,
    /// Globals whose writes have effects.
    globals: Vec<(GlobalPattern, Effects)>,
}

/// A pattern of global names, folded once when it is made: a whole name (`$Pref::Dev`), or a start of names when it ends
/// in `*` (`$Pref::Lua::*`). Compared without case; a name is taken up to its first NUL byte, as the script engine keeps
/// it ([`crate::vars::Dictionary`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalPattern {
    folded: Vec<u8>,
    start: bool,
}

impl GlobalPattern {
    /// The pattern `text` (with its `$`).
    pub fn new(text: &str) -> GlobalPattern {
        let (body, start) = match text.strip_suffix('*') {
            Some(body) => (body, true),
            None => (text, false),
        };
        GlobalPattern {
            folded: fold(body.as_bytes()),
            start,
        }
    }

    /// Whether it names the global `folded` (a name already folded and cut at its NUL, [`fold_global`]).
    pub fn matches_folded(&self, folded: &[u8]) -> bool {
        if self.start {
            folded.starts_with(&self.folded)
        } else {
            folded == self.folded.as_slice()
        }
    }
}

/// The global `name` as patterns compare it: cut at its first NUL byte and folded.
pub fn fold_global(name: &[u8]) -> Vec<u8> {
    fold(crate::text::until_nul(name))
}

/// The effects of writing the global `name` that `patterns` give: every pattern that names it.
pub fn global_effects_in(patterns: &[(GlobalPattern, Effects)], name: &[u8]) -> Effects {
    let folded = fold_global(name);
    patterns
        .iter()
        .filter(|(pattern, _)| pattern.matches_folded(&folded))
        .fold(Effects::NONE, |set, &(_, e)| set | e)
}

fn key(namespace: &[u8], name: &[u8]) -> Vec<u8> {
    let mut key = fold(namespace);
    key.extend_from_slice(b"::");
    key.extend_from_slice(&fold(name));
    key
}

impl Vm {
    /// Declares that the native `namespace::name` (`namespace` empty for a global function, a class for a method) has
    /// `effects`, on top of what was declared for it before. Called right after it is registered; a name declared before
    /// its native is registered keeps the effects all the same.
    pub fn add_native_effects(&mut self, namespace: &str, name: &str, effects: Effects) {
        if effects.is_empty() {
            return;
        }
        let table = &mut self.policies_mut().effects;
        *table
            .natives
            .entry(key(namespace.as_bytes(), name.as_bytes()))
            .or_default() |= effects;
        *table.names.entry(fold(name.as_bytes())).or_default() |= effects;
    }

    /// The effects declared for the native `namespace::name`, without case.
    pub fn native_effects(&self, namespace: &str, name: &str) -> Effects {
        self.policies()
            .effects
            .natives
            .get(&key(namespace.as_bytes(), name.as_bytes()))
            .copied()
            .unwrap_or_default()
    }

    /// Declares that writing the global `pattern` (with its `$`; a start of names when it ends in `*`, `$Pref::Lua::*`)
    /// has `effects`: a write that changes what the engine allows (a limit, a folder, which mods load) or what it runs.
    /// Only ever adds, as [`add_native_effects`](Self::add_native_effects) does.
    pub fn add_global_effects(&mut self, pattern: &str, effects: Effects) {
        if effects.is_empty() {
            return;
        }
        let pattern = GlobalPattern::new(pattern);
        let globals = &mut self.policies_mut().effects.globals;
        match globals.iter_mut().find(|(p, _)| *p == pattern) {
            Some((_, held)) => *held |= effects,
            None => globals.push((pattern, effects)),
        }
    }

    /// The effects of writing the global `name` (with its `$`), without case: every declared pattern that names it.
    pub fn global_effects(&self, name: &[u8]) -> Effects {
        global_effects_in(&self.policies().effects.globals, name)
    }

    /// The effects declared for any native named `name` in any namespace, without case: what a policy that checks a call
    /// by its name (whatever namespace or object it goes through) must assume.
    pub fn effects_of_name(&self, name: &[u8]) -> Effects {
        self.policies()
            .effects
            .names
            .get(&fold(name))
            .copied()
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effects_read_back_as_written() {
        for text in [
            "-",
            "files",
            "code",
            "files+code+engine+network",
            "permissions+variables",
            "engine+network",
        ] {
            let effects = Effects::parse(text).expect(text);
            assert_eq!(effects.to_string(), text);
        }
        assert_eq!(
            Effects::parse("code+files"),
            Some(Effects::FILES | Effects::CODE)
        );
        assert_eq!(Effects::parse("disk"), None);
        assert_eq!(Effects::parse(""), None);
    }

    #[test]
    fn a_globals_effects_come_from_every_pattern_that_names_it() {
        let mut vm = Vm::new();
        vm.add_global_effects("$Pref::Lua::*", Effects::ENGINE);
        vm.add_global_effects("$pref::lua::MemoryLimit", Effects::FILES);
        assert_eq!(
            vm.global_effects(b"$PREF::Lua::MemoryLimit"),
            Effects::ENGINE | Effects::FILES
        );
        assert_eq!(vm.global_effects(b"$Pref::Lua::Other"), Effects::ENGINE);
        assert_eq!(vm.global_effects(b"$Pref::Luaish"), Effects::NONE);
        assert_eq!(vm.global_effects(b"$instantGroup"), Effects::ENGINE);
        // A name is taken up to its NUL, as the script engine keys it.
        assert_eq!(vm.global_effects(b"$Pref::Lua::Other\0x"), Effects::ENGINE);
        vm.add_global_effects("$Pref::Dev", Effects::ENGINE);
        assert_eq!(vm.global_effects(b"$pref::dev\0x"), Effects::ENGINE);
    }

    #[test]
    fn a_names_effects_only_grow_and_cover_every_namespace() {
        let mut vm = Vm::new();
        vm.add_native_effects("ActionMap", "save", Effects::FILES);
        vm.register_in("Other", "save", 2, 2, "", |_, _| crate::vm::Native::Void);
        vm.add_native_effects("Other", "SAVE", Effects::NONE);
        assert_eq!(vm.native_effects("actionmap", "Save"), Effects::FILES);
        assert_eq!(vm.native_effects("Other", "save"), Effects::NONE);
        assert_eq!(vm.effects_of_name(b"SaVe"), Effects::FILES);
        vm.add_native_effects("ActionMap", "save", Effects::CODE);
        assert_eq!(
            vm.native_effects("ActionMap", "save"),
            Effects::FILES | Effects::CODE
        );
    }
}
