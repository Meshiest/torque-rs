//! Removing a datablock from a running server: the checked `removeDataBlock`, the user queries, and the reference counting
//! behind a datablock's plain `delete()`.
//!
//! **Plain `delete()`.** A datablock's `delete()` with no users deletes it at once, as any object's. One with users ([`refresh`]
//! counts them) is *retired* instead ([`Objects::retire_datablock`]): it stays registered and in DataBlockGroup, the objects
//! built from it keep working and a client that joins still receives it, but its name and id resolve for no new user (a new
//! object's `dataBlock`, `setDataBlock`, a datablock field of another datablock, `datablock Class(name)`, which makes a new
//! datablock of the name). The tick's drain frees it once its count is zero ([`refresh::drain`]). The id is spent either way:
//! the datablock id counter only grows inside a mission (`deleteDataBlocks()` resets it), so a client that still holds the
//! datablock never sees its id mean another.
//!
//! **`removeDataBlock(%db [, %mode])`** is the checked removal. It refuses, with the users and the referrers named, by default;
//! with a fallback datablock of the datablock's class it moves every user to it (`setDataBlock`: a ghost sees a normal datablock
//! change) and rewrites every datablock field that holds the id (the stored id, not a text), through the normal field write so
//! the referrer's key moves and its own users refresh; with `"deleteUsers"` it deletes every user first. A *referrer* is another
//! datablock with a [`FieldType::DataBlock`] field holding the id; a datablock a referrer points at is refused in every mode but
//! the fallback's, since the referrer's clients would read a dead id.
//!
//! **Queries.** `%db.getUserCount()`, `%db.getUsers([%max])` and `getDataBlockUsers(%db [, %max])`: the count is the datablock's
//! own, the lists are one scan of the process list, at most [`MAX_LISTED_USERS`] ids (a script's `%max` is held under it).
//!
//! **The wire.** Nothing here posts anything to a connection. A host that tells its clients about a removal sets a
//! [`RemovalHook`]: it runs once per datablock, with the id, at the moment the id stops resolving (the retire or the delete),
//! while the object is still registered.
//!
//! The natives are the server's: a client's script engine never gets them.
//!
//! [`refresh`]: super::refresh
//! [`Objects::retire_datablock`]: ts_vm::sim::Objects::retire_datablock

use std::cell::RefCell;
use std::rc::Rc;

use ts_vm::fields::{FieldType, Value};
use ts_vm::sim::{ObjId, SIM_DATA_BLOCK};
use ts_vm::vm::{FieldTarget, Native, Vm, VmError};

use super::refresh;

/// The most user ids a query lists (`getUsers`, `getDataBlockUsers`): a script's own cap is held under it.
pub const MAX_LISTED_USERS: usize = 4096;

/// How many users a query lists when the script names no cap.
pub const DEFAULT_LISTED_USERS: usize = 256;

/// What a removal does with the datablock's users.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removal {
    /// Refuse while the datablock has a user or a referrer.
    Refuse,
    /// Move every user to this datablock (of the removed one's class) and rewrite the referrers' fields to it.
    Repoint(ObjId),
    /// Delete every user first.
    DeleteUsers,
}

/// A host's hook for a datablock whose id stops resolving ([`set_removal_hook`]).
pub type RemovalHook = Rc<dyn Fn(&mut Vm, ObjId)>;

struct HookSlot(RefCell<Option<RemovalHook>>);

/// Gives the server `hook`, run once with each datablock's id when the datablock is retired or deleted through the natives here
/// (a plain `delete()`, `removeDataBlock`), before the deletion, while the object is still registered. None removes it.
pub fn set_removal_hook(vm: &mut Vm, hook: Option<RemovalHook>) {
    vm.set_extension(Rc::new(HookSlot(RefCell::new(hook))));
}

/// The hook, once per datablock: a datablock that is already retiring stopped resolving when it retired.
fn removed(vm: &mut Vm, db: ObjId) {
    if vm.objects().is_retiring(db) {
        return;
    }
    let hook = vm
        .extension::<HookSlot>()
        .and_then(|slot| slot.0.borrow().clone());
    if let Some(hook) = hook {
        hook(vm, db);
    }
}

fn is_datablock(vm: &Vm, id: ObjId) -> bool {
    let objects = vm.objects();
    objects
        .get(id)
        .is_some_and(|o| objects.classes.is_a(o.class(), SIM_DATA_BLOCK))
}

/// A datablock named or numbered by a script, for a new use: none for a retiring one.
fn find_live(vm: &Vm, text: &[u8]) -> Option<ObjId> {
    let id = vm.objects().find(text)?;
    (is_datablock(vm, id) && !vm.objects().is_retiring(id)).then_some(id)
}

fn shown(vm: &Vm, id: ObjId) -> String {
    let objects = vm.objects();
    let Some(object) = objects.get(id) else {
        return id.to_string();
    };
    let class = String::from_utf8_lossy(objects.classes.name(object.class()));
    match object.name() {
        Some(name) => format!("{class} {} ({id})", String::from_utf8_lossy(name)),
        None => format!("{class} {id}"),
    }
}

/// A script's `%db.delete()`: gone at once without users, retired with them ([`Objects::retire_datablock`]). The hook runs once
/// either way.
///
/// # Errors
/// The deletion's (`onRemove`) ran past the script engine's limits.
///
/// [`Objects::retire_datablock`]: ts_vm::sim::Objects::retire_datablock
pub fn delete_datablock(vm: &mut Vm, db: ObjId) -> Result<(), VmError> {
    if !is_datablock(vm, db) {
        return vm.delete_object(db);
    }
    if refresh::users(vm, db) == 0 {
        removed(vm, db);
        return vm.delete_object(db);
    }
    removed(vm, db);
    vm.objects_mut().retire_datablock(db);
    Ok(())
}

/// A datablock field of a datablock that holds `db`: the datablock, the field and the element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Referrer {
    pub datablock: ObjId,
    pub field: Vec<u8>,
    pub index: usize,
}

/// The datablocks whose [`FieldType::DataBlock`] fields hold `db` (the stored id), at most `max`: one pass over DataBlockGroup,
/// each datablock charged as one unit of work.
pub fn referrers(vm: &Vm, db: ObjId, max: usize) -> Vec<Referrer> {
    let objects = vm.objects();
    let Some(members) = objects.members(objects.datablock_group()) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for &id in members.as_slice() {
        if found.len() >= max {
            break;
        }
        vm.charge_work(1);
        let Some(object) = objects.get(id) else {
            continue;
        };
        let Some(class) = objects.classes.get(object.class()) else {
            continue;
        };
        for (field, slots) in &class.layout.fields {
            if !matches!(field.ty, FieldType::DataBlock(_)) {
                continue;
            }
            for (index, &slot) in slots.iter().enumerate() {
                if object.slot(slot) == Some(&Value::Object(Some(db))) {
                    found.push(Referrer {
                        datablock: id,
                        field: field.name.clone(),
                        index,
                    });
                }
            }
        }
    }
    found.truncate(max);
    found
}

/// Why a removal was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// Not a datablock.
    NotADatablock,
    /// The fallback is no live datablock of the datablock's class.
    BadFallback,
    /// This user's class does not take the fallback.
    FallbackNotTaken(ObjId),
    /// Objects use the datablock: how many, and the first by id.
    Users { count: u32, first: Option<ObjId> },
    /// Datablocks point at it.
    Referrers(Vec<Referrer>),
}

/// What `removeDataBlock` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The datablock is gone.
    Removed,
    /// Nothing was changed, for this reason.
    Kept(Refused),
}

/// `removeDataBlock`: the checks of `mode` (every one before anything changes), the users moved or deleted, the referrers
/// rewritten, then the datablock deleted.
///
/// # Errors
/// A user's `setDataBlock` work, a deletion or a callback ran past the script engine's limits (what ran so far stands).
pub fn remove_datablock(vm: &mut Vm, db: ObjId, mode: Removal) -> Result<Outcome, VmError> {
    // A retiring datablock may be removed too (by its id: its name finds nothing): a script's way to force a retire to its end.
    if !is_datablock(vm, db) {
        return Ok(Outcome::Kept(Refused::NotADatablock));
    }
    let fallback = match mode {
        Removal::Repoint(fallback) => {
            if !takes_place_of(vm, fallback, db) {
                return Ok(Outcome::Kept(Refused::BadFallback));
            }
            Some(fallback)
        }
        Removal::Refuse | Removal::DeleteUsers => None,
    };
    // Every referrer, not a listing's worth: a fallback must leave no field holding the dead id.
    let pointed_at = referrers(vm, db, usize::MAX);
    if fallback.is_none() && !pointed_at.is_empty() {
        return Ok(Outcome::Kept(Refused::Referrers(pointed_at)));
    }
    let count = refresh::users(vm, db);
    let users = refresh::users_of(vm, db, usize::MAX);
    if count > 0 && mode == Removal::Refuse {
        return Ok(Outcome::Kept(Refused::Users {
            count,
            first: users.first().copied(),
        }));
    }
    if let Some(fallback) = fallback
        && let Some(&user) = users
            .iter()
            .find(|&&user| !super::object::takes_datablock(vm, user, fallback))
    {
        return Ok(Outcome::Kept(Refused::FallbackNotTaken(user)));
    }
    match fallback {
        None => {
            for user in users {
                vm.delete_object(user)?;
            }
        }
        Some(fallback) => {
            for user in users {
                super::object::set_data_block(vm, user, fallback)?;
            }
            let text = fallback.to_string().into_bytes();
            // The field write scripts make: observed (the referrer's key moves, its users are marked) and validated.
            for r in &pointed_at {
                vm.write_field_of(
                    FieldTarget::Live(r.datablock),
                    &r.field,
                    r.index.to_string().as_bytes(),
                    &text,
                );
            }
        }
    }
    // A user a callback gave the datablock meanwhile leaves it in place.
    if refresh::users(vm, db) > 0 {
        return Ok(Outcome::Kept(Refused::Users {
            count: refresh::users(vm, db),
            first: refresh::users_of(vm, db, 1).first().copied(),
        }));
    }
    removed(vm, db);
    vm.delete_object(db)?;
    Ok(Outcome::Removed)
}

/// Whether `fallback` is a live, non-retiring datablock of `db`'s class (or a class below it), and not `db` itself.
fn takes_place_of(vm: &Vm, fallback: ObjId, db: ObjId) -> bool {
    let objects = vm.objects();
    let (Some(f), Some(d)) = (objects.get(fallback), objects.get(db)) else {
        return false;
    };
    fallback != db && !objects.is_retiring(fallback) && objects.classes.is_a(f.class(), d.class())
}

impl Refused {
    /// The console line of a refusal.
    pub fn line(&self, vm: &Vm, db: ObjId) -> String {
        let name = shown(vm, db);
        match self {
            Refused::NotADatablock => format!("removeDataBlock: {name} is not a datablock"),
            Refused::BadFallback => {
                format!("removeDataBlock: the fallback is not a datablock of {name}'s class")
            }
            Refused::FallbackNotTaken(user) => format!(
                "removeDataBlock: {} does not take the fallback datablock",
                shown(vm, *user)
            ),
            Refused::Users { count, first } => {
                let first = first.map_or_else(|| "none".to_string(), |id| shown(vm, id));
                format!("datablock {name} is used by {count} objects (first: {first})")
            }
            Refused::Referrers(referrers) => {
                let first = referrers.first().map_or_else(
                    || "none".to_string(),
                    |r| {
                        format!(
                            "{}.{}",
                            shown(vm, r.datablock),
                            String::from_utf8_lossy(&r.field)
                        )
                    },
                );
                format!(
                    "datablock {name} is referenced by {} datablock field(s) (first: {first})",
                    referrers.len()
                )
            }
        }
    }
}

/// A script's `%max`: the default without one, else held under [`MAX_LISTED_USERS`].
fn listed(arg: Option<&[u8]>) -> usize {
    match arg {
        None => DEFAULT_LISTED_USERS,
        Some(text) => {
            let asked = usize::try_from(ts_vm::text::atoi(text)).unwrap_or(0);
            asked.min(MAX_LISTED_USERS)
        }
    }
}

fn id_list(ids: &[ObjId]) -> Vec<u8> {
    ids.iter()
        .map(ObjId::to_string)
        .collect::<Vec<_>>()
        .join(" ")
        .into_bytes()
}

/// Registers the server's natives: `SimDataBlock::delete` (the counted one), `SimDataBlock::getUserCount`,
/// `SimDataBlock::getUsers`, `removeDataBlock` and `getDataBlockUsers`.
pub fn register(vm: &mut Vm) {
    vm.register_in(
        "SimDataBlock",
        "delete",
        2,
        2,
        "datablock.delete() - deletes the datablock, or retires it until its last user is gone",
        |vm, c| {
            let Some(id) = c.this_object else {
                return Native::Void;
            };
            match delete_datablock(vm, id) {
                Ok(()) => Native::Void,
                Err(e) => Native::Fail(e),
            }
        },
    );
    vm.register_in(
        "SimDataBlock",
        "getUserCount",
        2,
        2,
        "datablock.getUserCount() - how many objects have the datablock",
        |vm, c| {
            let count = c.this_object.map_or(0, |id| refresh::users(vm, id));
            Native::Int(i32::try_from(count).unwrap_or(i32::MAX))
        },
    );
    vm.register_in(
        "SimDataBlock",
        "getUsers",
        2,
        3,
        "datablock.getUsers([max]) - the ids of the objects that have the datablock, at most max (256 by default, 4096 at most)",
        |vm, c| {
            let Some(id) = c.this_object else {
                return Native::Str(Vec::new());
            };
            Native::Str(id_list(&refresh::users_of(
                vm,
                id,
                listed(c.argv.get(2).map(Vec::as_slice)),
            )))
        },
    );
    vm.register(
        "getDataBlockUsers",
        2,
        3,
        "getDataBlockUsers(datablock [, max]) - the ids of the objects that have the datablock, at most max (256 by default, 4096 at most)",
        |vm, c| {
            let Some(db) = c.argv.get(1).and_then(|text| vm.objects().find(text)) else {
                return Native::Str(Vec::new());
            };
            Native::Str(id_list(&refresh::users_of(
                vm,
                db,
                listed(c.argv.get(2).map(Vec::as_slice)),
            )))
        },
    );
    vm.register(
        "removeDataBlock",
        2,
        3,
        "removeDataBlock(datablock [, fallback | \"deleteUsers\"]) - removes a datablock no object or datablock uses; with a fallback datablock of its class moves its users and references to that first; with \"deleteUsers\" deletes its users first. False, with the reason printed, when it stays",
        |vm, c| {
            let text = c.argv.get(1).map_or(&b""[..], Vec::as_slice);
            let Some(db) = vm.objects().find(text) else {
                vm.error(format!(
                    "removeDataBlock: \"{}\" is not a datablock",
                    String::from_utf8_lossy(text)
                ));
                return Native::Bool(false);
            };
            let mode = match c.argv.get(2).map(Vec::as_slice) {
                None | Some(b"") => Removal::Refuse,
                Some(mode) if mode.eq_ignore_ascii_case(b"deleteUsers") => Removal::DeleteUsers,
                Some(name) => match find_live(vm, name) {
                    Some(fallback) => Removal::Repoint(fallback),
                    None => {
                        vm.error(format!(
                            "removeDataBlock: \"{}\" is not a datablock",
                            String::from_utf8_lossy(name)
                        ));
                        return Native::Bool(false);
                    }
                },
            };
            match remove_datablock(vm, db, mode) {
                Ok(Outcome::Removed) => Native::Bool(true),
                Ok(Outcome::Kept(refused)) => {
                    let line = refused.line(vm, db);
                    vm.error(line);
                    Native::Bool(false)
                }
                Err(e) => Native::Fail(e),
            }
        },
    );
}
