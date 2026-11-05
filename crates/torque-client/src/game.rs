//! What the client's engine half asks the game ([`ClientGame`], a `Vm` extension like the server's `NetCodecs`): the network codecs
//! the client speaks, the key names, the string coder of the install, the class table's fields and how the console types print, the
//! renderer's hooks, the rules of `setScreenMode` and what the prediction asks
//! ([`crate::predict::game::PredictGame`]). Without a game's, each is Torque's own.

use std::any::Any;
use std::fmt::Debug;
use std::path::Path;
use std::rc::Rc;

use gui::keys::KeyTables;
use net::huffman::Table;
use torque_server::net::codecs::{NetCodecs, TorqueCodecs};
use ts_vm::fields::{FieldRules, FieldType};
use ts_vm::vm::Vm;

use crate::datablocks::Unpacked;
use crate::render::{NoRenderer, RenderResets};
use crate::screen::{ScreenRules, TgeScreen};

/// A static field of a class: name, type, element count.
pub type ClassField = (&'static str, FieldType, usize);

/// A game's answers to the client's engine half. Every method has Torque's answer by default.
pub trait ClientGame: Debug {
    /// The network codecs the client speaks (the handshake's widths, the query's version fields, the LAN rules, the commands'
    /// prefixes).
    fn codecs(&self) -> Rc<dyn NetCodecs> {
        Rc::new(TorqueCodecs)
    }

    /// The key names and the keyboard layout the action maps read and write.
    fn key_tables(&self) -> KeyTables {
        KeyTables::engine()
    }

    /// The string coder of the install at `root` (the server query reads the answers' strings with it).
    fn string_table(&self, root: &Path) -> Result<Rc<Table>, String> {
        let _ = root;
        Ok(Rc::new(Table::torque()))
    }

    /// A class's static fields as the class table has them, the inherited ones included (none by default: a datablock the
    /// server sends then gets no field text).
    fn class_fields(&self, class: &str) -> Vec<ClassField> {
        let _ = class;
        Vec::new()
    }

    /// How the console types print a field's value.
    fn field_rules(&self) -> FieldRules {
        FieldRules::default()
    }

    /// A datablock event as the game's unpack makes it (`event`: the event as the game's connection read it); none: the client
    /// cannot make it. Torque's unpacks TGE's ([`crate::conn::tge::TgeDatablockEvent`]: each field as the console prints
    /// it by [`ClientGame::field_rules`]).
    fn unpack_datablock(&self, event: &dyn Any) -> Option<Unpacked> {
        let event = event.downcast_ref::<crate::conn::tge::TgeDatablockEvent>()?;
        Some(crate::conn::tge::unpack_datablock(
            self.field_rules(),
            event,
        ))
    }

    /// What the renderer is told when what it looked up goes stale.
    fn renderer(&self) -> Rc<dyn RenderResets> {
        Rc::new(NoRenderer)
    }

    /// What the client's prediction asks the game: a [`Prediction`](crate::predict::game::Prediction) for the world type the game's connection keeps, behind `Any`
    /// ([`crate::predict::game::prediction_for`] takes it back); none for Torque's.
    fn prediction(&self) -> Option<Rc<dyn Any>> {
        None
    }

    /// The rules of `setScreenMode` and the modes a headless run lists (Torque's: TGE 1.4's, [`crate::screen::TgeScreen`]).
    fn screen_rules(&self) -> Rc<dyn ScreenRules> {
        Rc::new(TgeScreen)
    }

    /// The game's files as a connection reads them (the shapes a datablock names: a WheeledVehicleData's wheel count, the
    /// vehicle the prediction drives); none reads no file.
    fn file_reader(&self) -> Option<crate::predict::FileReader> {
        None
    }
}

/// Torque's answers.
#[derive(Debug, Default)]
pub struct TorqueClient;

impl ClientGame for TorqueClient {}

struct Installed(Rc<dyn ClientGame>);

/// The game's answers for `vm` from now on.
pub fn set_client_game(vm: &mut Vm, game: Rc<dyn ClientGame>) {
    vm.set_extension(Rc::new(Installed(game)));
}

/// The answers `vm`'s game installed, if it installed any.
pub fn installed_game(vm: &Vm) -> Option<Rc<dyn ClientGame>> {
    vm.extension::<Installed>().map(|i| Rc::clone(&i.0))
}

/// The answers `vm`'s game installed, else Torque's.
pub fn client_game(vm: &Vm) -> Rc<dyn ClientGame> {
    vm.extension::<Installed>().map_or_else(
        || Rc::new(TorqueClient) as Rc<dyn ClientGame>,
        |i| Rc::clone(&i.0),
    )
}

/// The ghost-index widths of the game's servers: the narrowest table an accept may give and the widest (an engine server's may be
/// wider, up to `net::cap::NET_MAX_GHOST_BITS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GhostWidths {
    pub min: u8,
    pub max: u8,
}

impl GhostWidths {
    /// The widths of `codecs`' smallest and largest ghost tables.
    pub fn of(codecs: &dyn NetCodecs) -> GhostWidths {
        GhostWidths {
            min: codecs.ghost_id_bits(0),
            max: codecs.ghost_id_bits(u32::MAX),
        }
    }
}
