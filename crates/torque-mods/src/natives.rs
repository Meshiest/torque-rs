//! The console functions for mods.

use torque_engine::{VmMut, function};
use torque_server::net::channels::ChannelHub;
use torque_server::net::commands::find_net_connection;

use crate::mods::{ModStatus, Mods};
use crate::offer::Request;

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

/// how many mods the server requires of its clients (on a client, how many its server announced)
#[function(name = "getRequiredModCount")]
pub(crate) fn get_required_mod_count(vm: VmMut) -> i32 {
    Mods::of(&vm).map_or(0, |mods| {
        i32::try_from(mods.required_info().len()).unwrap_or(i32::MAX)
    })
}

/// the ids of the mods the server requires of its clients, separated by spaces (on a client, the ones its server announced)
#[function(name = "getRequiredModList")]
pub(crate) fn get_required_mod_list(vm: VmMut) -> String {
    Mods::of(&vm).map_or(String::new(), |mods| {
        mods.required_info()
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    })
}

/// one fact about a required mod: version, hash (64 hexadecimal digits), bytes, files or download (1 when the server sends it to a client that lacks it); empty for a mod or a fact there is not
#[function(name = "getRequiredModField")]
pub(crate) fn get_required_mod_field(vm: VmMut, id: &str, field: &str) -> String {
    let Some(mods) = Mods::of(&vm) else {
        return String::new();
    };
    let Some(m) = mods
        .required_info()
        .into_iter()
        .find(|m| m.id.eq_ignore_ascii_case(id))
    else {
        return String::new();
    };
    match field.to_ascii_lowercase().as_str() {
        "version" => m.version,
        "hash" => m.hash,
        "bytes" => m.bytes.to_string(),
        "files" => m.files.to_string(),
        "download" => u8::from(m.download).to_string(),
        _ => String::new(),
    }
}

/// where a client's download of the server's mod content stands: idle, waiting (accepted, nothing announced yet), downloading, done or failed
#[function(name = "getModDownloadState")]
pub(crate) fn get_mod_download_state(vm: VmMut) -> String {
    Mods::of(&vm)
        .map_or("idle", |mods| mods.download_state())
        .to_string()
}

/// the download under way as four numbers separated by spaces: bytes received, bytes in all, mods mounted, mods sent
#[function(name = "getModDownloadProgress")]
pub(crate) fn get_mod_download_progress(vm: VmMut) -> String {
    let (received, total, mounted, sent) =
        Mods::of(&vm).map_or((0, 0, 0, 0), |mods| mods.download_progress());
    format!("{received} {total} {mounted} {sent}")
}

/// deletes the mod content kept from servers; returns the bytes freed
#[function(name = "clearModDownloadCache")]
pub(crate) fn clear_mod_download_cache(vm: VmMut) -> String {
    Mods::of(&vm)
        .map_or(0, |mods| mods.clear_download_cache())
        .to_string()
}

/// Drops a client the mods refuse: a request nobody honest makes.
fn kick(vm: &mut VmMut, obj: torque_ts_vm::sim::ObjId, reason: &str) {
    // The reasons are fixed plain text.
    let _ = vm.eval(format!("{obj}.delete(\"{reason}\");").as_bytes());
}

/// a client asks the server for a run of blocks of a required mod (the client's own downloading sends it; nothing else should)
#[function(name = "serverCmdTorqueModsGet")]
pub(crate) fn server_cmd_get(
    mut vm: VmMut,
    client: &str,
    mod_index: i32,
    file: i32,
    start: i32,
    count: i32,
) {
    let (Some(obj), Some(mods), Some(hub)) = (
        find_net_connection(&vm, client.as_bytes()),
        Mods::of(&vm),
        vm.extension::<ChannelHub>(),
    ) else {
        return;
    };
    let (Ok(mod_index), Ok(file), Ok(start), Ok(count)) = (
        u32::try_from(mod_index),
        u32::try_from(file),
        u32::try_from(start),
        u32::try_from(count),
    ) else {
        kick(&mut vm, obj, "Mod request out of range.");
        return;
    };
    let request = Request {
        mod_index,
        file,
        start,
        count,
    };
    if let Err(reason) = mods.serve_request(&hub, obj, request) {
        mods.end_serving(obj);
        kick(&mut vm, obj, reason);
    }
}

/// a client says it has every mod the server requires, mounted (the server then lets its join go on)
#[function(name = "serverCmdTorqueModsReady")]
pub(crate) fn server_cmd_ready(vm: VmMut, client: &str) {
    if let (Some(obj), Some(mods)) = (find_net_connection(&vm, client.as_bytes()), Mods::of(&vm)) {
        mods.mark_ready(obj);
    }
}
