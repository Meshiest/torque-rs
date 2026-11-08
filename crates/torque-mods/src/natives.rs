//! The console functions for mods.

use torque_engine::{VmMut, function};

use crate::mods::{ModStatus, Mods};

/// how many mods were found, the ones left out too
#[function(name = "getModCount")]
pub(crate) fn get_mod_count(vm: VmMut) -> i32 {
    Mods::of(&vm).map_or(0, |mods| {
        i32::try_from(mods.list().len()).unwrap_or(i32::MAX)
    })
}

/// the ids of the loaded mods in load order, separated by spaces (all of them, the ones left out too, with `true`)
#[function(name = "getModList")]
pub(crate) fn get_mod_list(vm: VmMut, all: Option<bool>) -> String {
    let Some(mods) = Mods::of(&vm) else {
        return String::new();
    };
    let all = all.unwrap_or(false);
    mods.list()
        .iter()
        .filter(|m| all || !matches!(m.status, ModStatus::Skipped(_)))
        .map(|m| m.info.id.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// one fact about a mod: version, name, description, author, category, status (pending, enabled, disabled or skipped), reason, source, package, files, bytes, clientside, or a field the layout kept; empty for a mod or a fact there is not
#[function(name = "getModField")]
pub(crate) fn get_mod_field(vm: VmMut, id: &str, field: &str) -> String {
    let Some(mods) = Mods::of(&vm) else {
        return String::new();
    };
    let Some(m) = mods.get(id) else {
        return String::new();
    };
    match field.to_ascii_lowercase().as_str() {
        "version" => m.info.version.to_string(),
        "name" => m.info.name,
        "description" => m.info.description,
        "author" => m.info.author,
        "category" => m.info.category.unwrap_or_default(),
        "status" => match m.status {
            ModStatus::Pending => "pending",
            ModStatus::Enabled => "enabled",
            ModStatus::Disabled => "disabled",
            ModStatus::Skipped(_) => "skipped",
        }
        .to_string(),
        "reason" => match m.status {
            ModStatus::Skipped(why) => why,
            _ => String::new(),
        },
        "source" => m.source,
        "package" => m.package,
        "files" => m.files.to_string(),
        "bytes" => m.bytes.to_string(),
        "clientside" => u8::from(m.info.client_side).to_string(),
        other => m.info.extra.get(other).cloned().unwrap_or_default(),
    }
}

/// whether the mod is loaded and switched on
#[function(name = "isModEnabled")]
pub(crate) fn is_mod_enabled(vm: VmMut, id: &str) -> bool {
    Mods::of(&vm)
        .and_then(|mods| mods.get(id))
        .is_some_and(|m| m.status == ModStatus::Enabled)
}

/// switches a mod on (its files mount again and its scripts run for the stages that ran); false, with the reason on the console, when it cannot
#[function(name = "enableMod")]
pub(crate) fn enable_mod(mut vm: VmMut, id: &str) -> bool {
    let Some(mods) = Mods::of(&vm) else {
        vm.error("enableMod: this script engine has no mods".to_string());
        return false;
    };
    match mods.enable(&mut vm, id) {
        Ok(()) => true,
        Err(why) => {
            vm.warn(format!("enableMod: {why}"));
            false
        }
    }
}

/// switches a mod off, and the mods that require it (its files unmount and its package is deactivated); false, with the reason on the console, when it cannot
#[function(name = "disableMod")]
pub(crate) fn disable_mod(mut vm: VmMut, id: &str) -> bool {
    let Some(mods) = Mods::of(&vm) else {
        vm.error("disableMod: this script engine has no mods".to_string());
        return false;
    };
    match mods.disable(&mut vm, id) {
        Ok(off) => {
            if let Some(rest) = off.get(1..).filter(|rest| !rest.is_empty()) {
                vm.print(format!("disableMod: also switched off: {}", rest.join(" ")).as_bytes());
            }
            true
        }
        Err(why) => {
            vm.warn(format!("disableMod: {why}"));
            false
        }
    }
}

/// runs the entry scripts of a load stage of every enabled mod, in load order, and returns how many ran (a stage runs once for a mod)
#[function(name = "loadMods")]
pub(crate) fn load_mods(mut vm: VmMut, stage: &str) -> i32 {
    let Some(mods) = Mods::of(&vm) else {
        vm.error("loadMods: this script engine has no mods".to_string());
        return 0;
    };
    match mods.load_stage(&mut vm, stage) {
        Ok(ran) => i32::try_from(ran).unwrap_or(i32::MAX),
        Err(why) => {
            vm.error(format!("loadMods: {why}"));
            0
        }
    }
}

/// the declared load stages, separated by spaces
#[function(name = "getModStages")]
pub(crate) fn get_mod_stages(vm: VmMut) -> String {
    Mods::of(&vm).map_or(String::new(), |mods| mods.stages().names().join(" "))
}

/// lists the mods on the console: order, id, version, status and where each came from
#[function(name = "dumpMods")]
pub(crate) fn dump_mods(mut vm: VmMut) {
    let Some(mods) = Mods::of(&vm) else {
        vm.print(b"No mods.");
        return;
    };
    let list = mods.list();
    vm.print(format!("{} mods:", list.len()).as_bytes());
    for (at, m) in list.iter().enumerate() {
        let status = match &m.status {
            ModStatus::Pending => "pending".to_string(),
            ModStatus::Enabled => "enabled".to_string(),
            ModStatus::Disabled => "disabled".to_string(),
            ModStatus::Skipped(why) => format!("skipped: {why}"),
        };
        vm.print(
            format!(
                "  {at}: {} {} ({status}) from {}",
                m.info.id, m.info.version, m.source
            )
            .as_bytes(),
        );
    }
}
