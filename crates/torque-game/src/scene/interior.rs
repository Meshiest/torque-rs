//! `InteriorInstance` on the server: `onAdd` reads the `.dif` its `interiorFile` names through the file host (each file read
//! once and kept), takes the interior's box as its object box, makes the interior's hulls its map collision
//! ([`super::ext::MapObject`]: the scene's rays and convexes answer from it) at its transform and scale, and enters the
//! container with the class's type bits (`InteriorObjectType`, `StaticObjectType`, `StaticRenderedObjectType`). An interior
//! whose file does not read fails its `onAdd`, as TGE's does. The file's CRC (Torque's: CRC-32 without the final inversion) is
//! kept for the ghost's first update ([`crc_of`]).
//!
//! The hulls are the first detail level's, read by [`torque_maps::legacy::IMPORTER`]; a hull's cost is charged to the run
//! before it is made ([`collide::map::interior::build_cost`]).
//!
//! Credit: follows TGE 1.4's `interior/interiorInstance.cc` (`InteriorInstance::onAdd`, the constructor's type mask) as
//! Torque3D 2.0, its MIT-licensed release, keeps it in `interior/interiorInstance.cpp` (Copyright (c) 2012 GarageGames, LLC);
//! the code here is this crate's own.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use collide::convex::ObjectView;
use collide::map::{InteriorGeom, InteriorShape, MapShape};
use collide::math::{Box3, affine_inverse};
use torque_resource::files::MAX_READ_BYTES;
use ts_vm::classes::EngineClass;
use ts_vm::host::{FileRead, FileStat};
use ts_vm::sim::ObjId;
use ts_vm::vm::{Vm, VmError};

use super::access::{matrix_of, scale_of};
use super::container::Container;
use super::ext::{self, MapObject};
use crate::charge::{charged_host, pay};

/// An interior's type bits.
pub const INTERIOR_TYPE_MASK: u32 =
    collide::types::INTERIOR | collide::types::STATIC | collide::types::STATIC_RENDERED;

/// A `.dif` read: its hulls, its box and its CRC.
#[derive(Debug, Clone)]
pub struct KeptInterior {
    pub geom: Arc<InteriorGeom>,
    pub bounds: Box3,
    pub crc: u32,
}

/// The files read (by path, lower case; `None` for one that did not read) and each interior object's CRC.
#[derive(Default)]
struct Interiors {
    files: RefCell<HashMap<Vec<u8>, Option<KeptInterior>>>,
    crcs: RefCell<HashMap<ObjId, u32>>,
}

/// Torque's CRC of a file: CRC-32 from all ones, not inverted at the end (`calculateCRC`).
pub fn torque_crc(bytes: &[u8]) -> u32 {
    !torque_maps::png::crc32(&[bytes])
}

/// The hulls and box of a `.dif`'s bytes; `None` for a file the importer refuses or one with no hull. `pay` is asked for the
/// hulls' cost first. A file whose hulls were made lately in this process ([`made`]: a server and its client in one process
/// read the same files) is not made again; its cost is asked for all the same.
pub fn read_interior(bytes: &[u8], pay: &mut dyn FnMut(u64) -> bool) -> Option<KeptInterior> {
    let crc = torque_crc(bytes);
    if let Some((cost, kept)) = made::find(bytes, crc) {
        return pay(cost).then_some(kept);
    }
    // One reading of the file: the importer's hulls are its first detail level's (`IMPORTER.convexes`), as is the box.
    let dif = torque_maps::dif::parse(bytes).ok()?;
    let first = dif.details.first()?;
    let set = torque_maps::legacy::convexes(first);
    let cost = collide::map::interior::build_cost(&set);
    if !pay(cost) {
        return None;
    }
    let geom = InteriorGeom::new(&set);
    let b = first.bounds;
    let kept = KeptInterior {
        bounds: Box3::new(b.min, b.max),
        geom: Arc::new(geom),
        crc,
    };
    made::keep(bytes, crc, cost, kept.clone());
    Some(kept)
}

/// The interiors made in this process, by their files' bytes (the same bytes, the same hulls), each known by its length, its
/// CRC and a 64-bit hash of its own ([`made::fingerprint`]): the [`made::MAX_FILES`] last read or found, the one longest
/// unused let go for a new one. A file that does not read is not kept (it is refused again, as cheaply as it was).
pub mod made {
    use std::collections::VecDeque;
    use std::sync::{Mutex, PoisonError};

    use super::KeptInterior;

    /// The most files kept.
    pub const MAX_FILES: usize = 256;

    /// A file's length, CRC and hash.
    type Key = (usize, u32, u64);

    /// What a file made: its hulls' cost and the interior.
    type Made = (u64, KeptInterior);

    /// The files kept, the one longest unused first.
    static MADE: Mutex<VecDeque<(Key, Made)>> = Mutex::new(VecDeque::new());

    /// A hash of `bytes`, eight at a time (a multiply and a rotate a word, the length and the last bytes mixed in): with the
    /// length and the CRC, a key no two files of a game share by chance.
    pub fn fingerprint(bytes: &[u8]) -> u64 {
        const K: u64 = 0x9e37_79b9_7f4a_7c15;
        let (words, rest) = bytes.as_chunks::<8>();
        let mut h = (bytes.len() as u64).wrapping_mul(K);
        for w in words {
            h = (h ^ u64::from_le_bytes(*w)).wrapping_mul(K).rotate_left(29);
        }
        let mut last = [0u8; 8];
        last[..rest.len()].copy_from_slice(rest);
        h = (h ^ u64::from_le_bytes(last)).wrapping_mul(K);
        h ^ (h >> 32)
    }

    fn key(bytes: &[u8], crc: u32) -> Key {
        (bytes.len(), crc, fingerprint(bytes))
    }

    /// What `bytes` (whose CRC is `crc`) made, if they are kept (now the last used).
    pub(super) fn find(bytes: &[u8], crc: u32) -> Option<Made> {
        let key = key(bytes, crc);
        let mut made = MADE.lock().unwrap_or_else(PoisonError::into_inner);
        let at = made.iter().position(|(k, _)| *k == key)?;
        let entry = made.remove(at)?;
        let found = entry.1.clone();
        made.push_back(entry);
        Some(found)
    }

    /// Keeps what `bytes` made (cost `cost`), the one longest unused let go when [`MAX_FILES`] are kept.
    pub(super) fn keep(bytes: &[u8], crc: u32, cost: u64, kept: KeptInterior) {
        let key = key(bytes, crc);
        let mut made = MADE.lock().unwrap_or_else(PoisonError::into_inner);
        if made.iter().any(|(k, _)| *k == key) {
            return;
        }
        while made.len() >= MAX_FILES {
            made.pop_front();
        }
        made.push_back((key, (cost, kept)));
    }

    /// How many files are kept.
    #[cfg(test)]
    pub(super) fn len() -> usize {
        MADE.lock().unwrap_or_else(PoisonError::into_inner).len()
    }
}

/// The CRC of interior object `id`'s file.
pub fn crc_of(vm: &Vm, id: ObjId) -> Option<u32> {
    vm.extension::<Interiors>()?.crcs.borrow().get(&id).copied()
}

struct InteriorClass {
    container: Rc<Container>,
    kept: Rc<Interiors>,
}

impl InteriorClass {
    fn load(&self, vm: &mut Vm, path: &[u8]) -> Result<Option<KeptInterior>, VmError> {
        let key = path.to_ascii_lowercase();
        if let Some(k) = self.kept.files.borrow().get(&key) {
            return Ok(k.clone());
        }
        let len = match charged_host(vm, |host| host.stat(path))? {
            FileStat::Found { len } => len,
            FileStat::Missing => 0,
            FileStat::Refused(refusal) => {
                vm.report_refusal(path, refusal)?;
                0
            }
        };
        let mut out = None;
        if len > 0 && len <= MAX_READ_BYTES {
            pay(vm, len)?;
            if let FileRead::Found(bytes) = charged_host(vm, |host| host.read(path))? {
                let vm_ref: &Vm = vm;
                out = read_interior(&bytes, &mut |u| vm_ref.charge_work(u));
            }
        }
        self.kept.files.borrow_mut().insert(key, out.clone());
        Ok(out)
    }
}

impl EngineClass for InteriorClass {
    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        let path = vm.script_field(id, b"interiorFile", b"");
        let Some(interior) = self.load(vm, &path)? else {
            vm.error(format!(
                "InteriorInstance::onAdd: unable to load interior {}",
                String::from_utf8_lossy(&path)
            ));
            return Ok(false);
        };
        super::object::set_object_box(vm, id, interior.bounds.min, interior.bounds.max);
        let (Some(matrix), scale) = (matrix_of(vm, id), scale_of(vm, id)) else {
            return Ok(false);
        };
        let Some(e) = ext::get(vm.objects(), id).and_then(|e| e.scene) else {
            return Ok(false);
        };
        let view = ObjectView {
            obj_to_world: matrix,
            world_to_obj: affine_inverse(&matrix),
            scale,
            obj_box: interior.bounds,
            world_box: Box3::new(e.world_min, e.world_max),
            world_radius: e.world_radius,
            type_mask: INTERIOR_TYPE_MASK,
            collision_disabled: false,
        };
        let shape =
            InteriorShape::new(Arc::clone(&interior.geom), matrix, scale).map(MapShape::Interior);
        if let Some(e) = ext::get_mut(vm.objects_mut(), id) {
            e.type_mask |= INTERIOR_TYPE_MASK;
            e.map = Some(MapObject {
                view,
                shape: shape.map(Rc::new),
            });
        }
        self.kept.crcs.borrow_mut().insert(id, interior.crc);
        super::object::add_to_scene(vm, &self.container, id);
        Ok(true)
    }

    fn on_remove(&self, vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
        let _ = vm;
        self.kept.crcs.borrow_mut().remove(&id);
        Ok(())
    }
}

/// Gives `InteriorInstance` its `onAdd` over `container`.
pub fn register(vm: &mut Vm, container: &Rc<Container>) {
    let kept = match vm.extension::<Interiors>() {
        Some(k) => k,
        None => {
            let k = Rc::new(Interiors::default());
            vm.set_extension(Rc::clone(&k));
            k
        }
    };
    vm.set_class_behaviour(
        "InteriorInstance",
        Rc::new(InteriorClass {
            container: Rc::clone(container),
            kept,
        }),
    );
}

#[cfg(test)]
#[path = "interior_tests.rs"]
mod tests;
