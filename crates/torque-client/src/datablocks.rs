//! The client's datablocks as script objects: TGE's `SimDataBlockEvent::process` on the client. After the script's
//! `onDataBlockObjectReceived(index, total)` the datablock the event unpacked (a new object of the event's class, `unpackData` into
//! its members) is registered under the server's id and joins DataBlockGroup, so scripts that walk `getDataBlockGroupSize()` /
//! `getDataBlock(i)` (a dialog's lists of player types and items, an inventory's icons) read its fields as on the server. A
//! datablock with that id already there (a second mission's) takes the new data and is preloaded again (the event's other branch:
//! `packData` of the new one, `unpackData` into the old one, `preload(false)`), when the client made it from an earlier event; an
//! object the client did not make (the server's own, on the script engine a local client shares with its server) is preloaded
//! and left as it is. Client datablocks have no names. Each object's generation is kept in [`DatablockObjects`], for the caches
//! a plugin builds from an object's fields.
//!
//! **The fields' text** is the game's unpack ([`Unpacked`], from [`crate::game::ClientGame::unpack_datablock`]: what the
//! client's getters print from the members `unpackData` wrote; TGE's in [`crate::conn::tge`]); a text field
//! goes into the client's string table, a datablock pointer prints the name of the object its id reaches ([`datablock_pointer_name`])
//! or "" (the client's datablocks are unnamed).
//!
//! **The preload** runs at once (TGE queues the datablock for `GameConnection::preloadNextDataBlock`, which preloads them in order
//! over the next ticks): no script can tell, the datablocks are preloaded before the mission's ghosts need them either way.
//!
//! **Engine-only** (TGE clobbers): an object at the id that is not a datablock of the event's class is left alone and the event
//! refused ([`Received::Refused`]); TGE warns, deletes it and registers the new datablock.
//!
//! Credit: follows TGE's `SimDataBlockEvent` as Torque3D carries it (MIT: `T3D/gameBase/gameConnectionEvents.cpp` and
//! `T3D/gameBase/gameConnection.cpp`, each Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use ts_vm::fields::FieldType;
use ts_vm::sim::ObjId;
use ts_vm::vm::{Vm, VmError};

use crate::game::{ClientGame, client_game};

/// The client's datablock objects by id, as the events made and changed them: a generation per id, moved by every event that
/// wrote the object (or, for an object the client did not make, that carried a record for it), and whether the client made it
/// ([`receive`]). A script engine's extension ([`DatablockObjects::of`]); a plugin that copies a datablock's fields out of its
/// object (a prop's numbers) keys the copy on `(id, generation)` and reads again when the generation moved. The world's record
/// store has a generation of its own (`ghost_world::ClientWorld::datablock_generation`), moved when the record arrives; this one
/// moves when the object is written, which can be a tick or more later (the events of a tick are capped), so a copy made from
/// the object is keyed on this one.
///
/// Ids are the ones the script engine accepted as datablock ids (its datablock range), so the table is bounded by that range.
#[derive(Debug, Default)]
pub struct DatablockObjects(RefCell<ObjectGenerations>);

#[derive(Debug, Default)]
struct ObjectGenerations {
    /// Moves at every change of any datablock object.
    epoch: u64,
    /// By id: the epoch at its last change, and whether the client made the object from an event.
    objects: BTreeMap<u32, (u64, bool)>,
}

impl DatablockObjects {
    /// The client's table, once an event made or changed a datablock object on `vm`.
    pub fn of(vm: &Vm) -> Option<Rc<DatablockObjects>> {
        vm.extension::<DatablockObjects>()
    }

    /// The epoch: a value other than the one last seen means some datablock object changed.
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.0.borrow().epoch
    }

    /// The generation of datablock object `id`: 0 for one no event has made or changed, else the epoch of its last change.
    #[must_use]
    pub fn generation(&self, id: u32) -> u64 {
        self.0.borrow().objects.get(&id).map_or(0, |o| o.0)
    }

    /// Whether the object at `id` is one the client made from a datablock event and still there (an object made by a script,
    /// or the server's own in a shared engine, is not the client's to write). The client's own datablocks are unnamed: a named
    /// object at the id is a script's, made after the client's went.
    #[must_use]
    pub fn made(&self, vm: &Vm, id: u32) -> bool {
        self.0.borrow().objects.get(&id).is_some_and(|o| o.1)
            && vm
                .objects()
                .get(id)
                .is_some_and(|o| o.name().is_none_or(|n| n.is_empty()))
    }

    /// Notes the event for `id`: its object made (or, on an update, written) by the client when `made`.
    fn changed(&self, id: u32, made: bool) {
        let mut g = self.0.borrow_mut();
        g.epoch += 1;
        let epoch = g.epoch;
        g.objects.insert(id, (epoch, made));
    }
}

/// Notes that the event for datablock `id` made (`made`) or changed its object on `vm`.
fn note_changed(vm: &mut Vm, id: u32, made: bool) {
    let objects = match DatablockObjects::of(vm) {
        Some(o) => o,
        None => {
            let o = Rc::new(DatablockObjects::default());
            vm.set_extension(Rc::clone(&o));
            o
        }
    };
    objects.changed(id, made);
}

/// A field the unpack writes: its name, its array index ("" for a scalar) and its text.
pub type FieldText = (Vec<u8>, Vec<u8>, Vec<u8>);

/// A field the unpack writes ([`FieldText`]) and, for a datablock pointer the event carried an id for, that id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnpackedField {
    pub name: Vec<u8>,
    pub index: Vec<u8>,
    pub text: Vec<u8>,
    pub pointer: Option<u32>,
}

/// A datablock as the game's unpack made it of its event (`unpackData` into a new object of its class).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unpacked {
    /// The server's id.
    pub id: u32,
    pub class: String,
    pub fields: Vec<UnpackedField>,
    /// The error the unpack prints, if any.
    pub error: Option<String>,
    /// A particle emitter's particle ids (the client keeps them in the datablock values' side table: its emitter has no
    /// `particles` text); `None` for another class.
    pub emitter_ids: Option<Vec<i32>>,
}

/// Whether `name` is a text field of `class` (`string`, `caseString` or `filename`).
fn string_field(game: &dyn ClientGame, class: &str, name: &[u8]) -> bool {
    game.class_fields(class).iter().any(|(n, ty, _)| {
        n.as_bytes() == name
            && matches!(
                ty,
                FieldType::String | FieldType::CaseString | FieldType::Filename
            )
    })
}

/// What [`receive`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Received {
    Added(ObjId),
    Updated(ObjId),
    /// Not made (an id outside the range, a class the client does not know, a refused `onAdd`, a failed preload).
    Refused,
}

/// The name of the object a datablock pointer id reaches, as TGE's `preload` resolves one (`AudioProfile::preload`: the id looked
/// up in the id dictionary, then a dynamic cast to the class the pointer holds, `AudioDescription`; a null result prints "could not
/// resolve description id" and fails the preload): an id in the datablock range ([`FIRST_DATABLOCK_ID`] up to the last id of the
/// range, `Objects::datablock_id_limit`), an object there that is a `SimDataBlock` and, when the class table types the field
/// (`FieldType::DataBlock`), of that class. `None` otherwise (the engine writes the field empty where TGE's client fails the
/// datablock's preload), and for an unnamed object (a field's text cannot hold it).
///
/// [`FIRST_DATABLOCK_ID`]: ts_vm::sim::FIRST_DATABLOCK_ID
pub fn datablock_pointer_name(vm: &Vm, expected: Option<&str>, id: u32) -> Option<Vec<u8>> {
    if !(ts_vm::sim::FIRST_DATABLOCK_ID..vm.objects().datablock_id_limit()).contains(&id) {
        return None;
    }
    let objects = vm.objects();
    let o = objects.get(id)?;
    if !objects.classes.is_a(o.class(), ts_vm::sim::SIM_DATA_BLOCK) {
        return None;
    }
    if let Some(class) = expected {
        let wanted = objects.classes.find(class.as_bytes())?;
        if !objects.classes.is_a(o.class(), wanted) {
            return None;
        }
    }
    let named = o.name()?.to_vec();
    (!named.is_empty()).then_some(named)
}

/// A datablock pointer's text as the field's getter prints it: the name of the object pointer `id` of field `name` of `class`
/// reaches, when it has one. A remote client's datablocks are unnamed (the getter prints ""); a local game's client shares the
/// server's own datablocks (one VM, as TGE's local client: `unpackData` sets the pointer to the server's object), so its
/// pointer fields keep naming them (a weapon image's `projectile` keeps naming the server's projectile, which its `onFire` makes;
/// the "" a remote client prints would wipe it).
///
/// The id is the server's (untrusted): it names an object only as TGE's `preload` resolves a pointer id
/// ([`datablock_pointer_name`]); any other id gives `None` and the field its plain client text ("").
fn pointer_name(vm: &Vm, class: &str, name: &[u8], id: u32) -> Option<Vec<u8>> {
    let name = std::str::from_utf8(name).ok()?;
    let expected = client_game(vm)
        .class_fields(class)
        .into_iter()
        .find(|(n, _, _)| n.eq_ignore_ascii_case(name))
        .and_then(|(_, ty, _)| match ty {
            FieldType::DataBlock(c) => Some(c),
            _ => None,
        });
    datablock_pointer_name(vm, expected, id)
}

/// The datablock of one event made in the client (see the module docs), after its `onDataBlockObjectReceived`, from the game's
/// unpack `d`. `values` are the side tables: a new `ParticleEmitterData` keeps the particle ids its record carries there before its
/// add (a client emitter has no `particles` string, only the unpacked id list).
pub fn receive(
    vm: &mut Vm,
    d: &Unpacked,
    values: Option<&torque_game::dbvalues::DbValues>,
) -> Result<Received, VmError> {
    if let Some(error) = &d.error {
        vm.error(error.clone());
    }
    // `readSTString()`: every string the unpack reads goes into the client's string table ignoring case, so it reads in the first
    // spelling the client has (the server's `pickupName = "rifle"` reads "Rifle" when a GUI's text had that spelling first).
    let game = client_game(vm);
    let fields: Vec<FieldText> = d
        .fields
        .iter()
        .map(|f| {
            let text = if string_field(&*game, &d.class, &f.name) {
                vm.intern_string(&f.text).to_vec()
            } else if let Some(named) = f
                .pointer
                .and_then(|id| pointer_name(vm, &d.class, &f.name, id))
            {
                named
            } else {
                f.text.clone()
            };
            (f.name.clone(), f.index.clone(), text)
        })
        .collect();
    let id = d.id;
    let class = d.class.as_bytes();
    // An object already at the id: TGE updates it in place when it is a datablock of the event's own class. Any other (another
    // class's datablock, an object the client made itself) is left alone and the event refused: the engine's own safety rule
    // (TGE clobbers it: `Clobbering it`, the object deleted and the new one registered).
    if let Some(o) = vm.objects().get(id) {
        let classes = &vm.objects().classes;
        let same = classes.is_a(o.class(), ts_vm::sim::SIM_DATA_BLOCK)
            && classes.find(class) == Some(o.class());
        if !same {
            return Ok(Received::Refused);
        }
        // The other branch (`packData` of the new one, `unpackData` into the one there, `preload(false)`) resolves a pointer by its
        // id. The fields are written only into an object the client made from an earlier event (a second mission's datablock):
        // in a local game the object at the id is the server's own datablock, written by the server's scripts, which the
        // client's reading of the wire must not write back (its texts are the wire's roundings of the server's values, and a
        // server that refreshes its objects on every write would take its own event for an edit). The pointer fields' texts
        // are the names [`pointer_name`] resolved (a local game's `play3D` reads `description` back by name), or empty for an
        // id the preload would not resolve.
        let made = DatablockObjects::of(vm).is_some_and(|o| o.made(vm, id));
        // A resend of what the object holds (a second mission's unchanged datablock) writes and changes nothing; an object the
        // client did not make counts as changed (the server sent the record for a reason).
        let mut changed = !made;
        if made {
            for (name, index, text) in &fields {
                let target = ts_vm::vm::FieldTarget::Live(id);
                if vm.read_field_of(target, name, index) != *text {
                    changed = true;
                    vm.set_object_member(id, name, index, text);
                }
            }
            if let (Some(ids), Some(values)) = (&d.emitter_ids, values) {
                values.set_emitter_ids(id, ids.clone());
            }
        }
        return Ok(match vm.preload_engine_datablock(id)? {
            true => {
                if changed {
                    note_changed(vm, id, made);
                }
                Received::Updated(id)
            }
            false => Received::Refused,
        });
    }
    let emitter = d.emitter_ids.is_some();
    if let (Some(ids), Some(values)) = (&d.emitter_ids, values) {
        values.set_emitter_ids(id, ids.clone());
    }
    let added = vm.add_engine_datablock(class, id, &fields)?;
    if let (true, Some(values), None) = (emitter, values, added) {
        values.forget(id);
    }
    Ok(match added {
        Some(id) => {
            note_changed(vm, id, true);
            Received::Added(id)
        }
        None => Received::Refused,
    })
}

#[cfg(test)]
#[path = "datablocks_tests.rs"]
mod tests;
