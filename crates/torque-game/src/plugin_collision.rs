//! The collision sources of the plugins a script engine has ([`torque_collide::source`]): the server's container reads them for
//! every collision phase ([`crate::scene::container_world::ContainerWorld`]), and the client's prediction world reads the
//! client's.
//!
//! A host installs them once its script engine exists, from the entries on the engine's list ([`install`]); an engine with none
//! has nothing installed and every query is as it was. Script sees each source's type bit as
//! `$TypeMasks::<Name>ObjectType`.

use std::rc::Rc;

use collide::source::{CollisionSources, PluginCollision, Side};
use ts_vm::vm::Vm;

struct Installed(Rc<CollisionSources>);

/// Gives `vm` the sources of `side` among `entries` and their type bits as globals. Which sources there are depends on the list
/// alone ([`CollisionSources::from_entries`]), so the server and the client keep the same ones whatever their script engines
/// hold. A global (`$TypeMasks::<Name>ObjectType`) that is already set is left as it is, with a warning. The warnings are given
/// back; nothing is installed when there is no source.
pub fn install(vm: &mut Vm, entries: &[PluginCollision], side: Side) -> Vec<String> {
    let (sources, mut warnings) = CollisionSources::from_entries(entries, side);
    if sources.is_empty() {
        return warnings;
    }
    for (name, bit) in sources.bits() {
        let global = format!("$TypeMasks::{name}ObjectType");
        if vm.global(&global).is_some() {
            warnings.push(format!("`{global}` is already set: left as it is"));
            continue;
        }
        vm.set_global_uint(global.as_bytes(), bit);
    }
    vm.set_extension(Rc::new(Installed(Rc::new(sources))));
    warnings
}

/// The sources `vm` was given, if any.
pub fn sources_of(vm: &Vm) -> Option<Rc<CollisionSources>> {
    vm.extension::<Installed>().map(|i| Rc::clone(&i.0))
}

#[cfg(test)]
mod tests {
    use collide::source::{CollisionShape, CollisionSource, RESERVED_NAMES};

    use super::*;

    #[derive(Debug)]
    struct NoShapes;

    impl CollisionSource for NoShapes {
        fn shapes(&self, _out: &mut Vec<CollisionShape>) {}
    }

    fn entries(names: &[&str]) -> Vec<PluginCollision> {
        names
            .iter()
            .map(|n| PluginCollision::server(*n, Rc::new(NoShapes)))
            .collect()
    }

    fn kept(vm: &Vm) -> Vec<(String, u32)> {
        sources_of(vm)
            .map(|s| s.bits().map(|(n, b)| (n.to_owned(), b)).collect())
            .unwrap_or_default()
    }

    #[test]
    fn a_source_cannot_take_a_built_in_types_name() {
        let mut vm = Vm::new();
        let mut names: Vec<String> = RESERVED_NAMES.iter().map(|n| n.to_lowercase()).collect();
        names.extend(["Wall", "a-b", "", "X;Y"].map(String::from));
        let list: Vec<&str> = names.iter().map(String::as_str).collect();
        let warnings = install(&mut vm, &entries(&list), Side::Server);
        // Only `Wall` is left; every other name is refused with a warning.
        assert_eq!(
            kept(&vm),
            [("Wall".to_owned(), collide::source::FREE_TYPE_BITS[0])]
        );
        assert_eq!(warnings.len(), names.len() - 1, "{warnings:?}");
        assert_eq!(vm.global("$TypeMasks::StaticShapeObjectType"), None);
        assert_eq!(vm.global("$TypeMasks::PlayerObjectType"), None);
        assert!(vm.global("$TypeMasks::WallObjectType").is_some());
    }

    #[test]
    fn both_sides_keep_the_same_sources_whatever_their_script_holds() {
        let list = [
            PluginCollision::server("Props", Rc::new(NoShapes)),
            PluginCollision::client("Props", Rc::new(NoShapes)),
            PluginCollision::server("Wall", Rc::new(NoShapes)),
            PluginCollision::client("wall", Rc::new(NoShapes)),
        ];
        let (mut server, mut client) = (Vm::new(), Vm::new());
        // The server's script engine already has one of the globals; the client's does not.
        server.set_global_uint(b"$TypeMasks::PropsObjectType", 5);
        install(&mut server, &list, Side::Server);
        install(&mut client, &list, Side::Client);
        assert_eq!(kept(&server), kept(&client));
        assert_eq!(kept(&server).len(), 2);
        // The global that was there stays; the other side's was set.
        assert_eq!(
            server.global("$TypeMasks::PropsObjectType"),
            Some(b"5".to_vec())
        );
        assert!(client.global("$TypeMasks::PropsObjectType").is_some());
    }
}
