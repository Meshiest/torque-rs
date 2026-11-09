//! Every native a full engine registers, with the effects it declares, against the reviewed list in
//! `tests/natives/effects.txt`; and the call policy against those effects.
//!
//! A new native, or one whose effects changed, fails this test until its line is in the list. Before adding it, decide:
//! a native that opens, writes or deletes files, runs code from text or a function it names, switches mods, deletes every
//! datablock or variable, quits, or connects to an address it is given declares that where it is registered (`effects =
//! "..."` on `#[function]`, `ScriptRegistry::with_effects`, or `Vm::add_native_effects`). `TORQUE_BLESS=1` rewrites the list.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;

use torque_app::client::{ClientBuilder, ClientNet};
use torque_app::gizmos::GizmosPlugin;
use torque_app::hosting::HostingPlugin;
use torque_app::page_bridge;
use torque_app::plugins::{DefaultPlugins, ZoomPlugin};
use torque_app::server::{GameServer, Listen, ServerStart};
use torque_engine::{Effects, EngineBuilder};
use torque_plugin_gui_layout::GuiLayoutPlugin;
use torque_plugin_loader::call_policy::{
    CallKind, CallPolicy, DENIED_EFFECTS, DenyCodeAndFiles, PolicyCall, TGE_EFFECTS,
};
use torque_plugin_props::PropsPlugin;
use torque_plugin_rope::RopePlugin;
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::listing::check_listing;
use torque_ts_vm::vm::Vm;

/// The script engines of a server and a client made from DefaultPlugins and every opt-in plugin with natives, and the
/// page bridge's, each handed to `each`.
fn for_each_script_engine(mut each: impl FnMut(&Vm)) {
    let mut engine = EngineBuilder::new();
    engine.add_plugins(DefaultPlugins);
    engine
        .add_plugins(GizmosPlugin::default())
        .add_plugins(ZoomPlugin)
        .add_plugins(HostingPlugin::default())
        .add_plugins(RopePlugin::default())
        .add_plugins(PropsPlugin)
        .add_plugins(GuiLayoutPlugin);
    let engine = engine.build().expect("the engine builds");
    let files = MemFiles::new();
    files.insert("server.cs", b"function startNothing() {}");
    let server = GameServer::start_engine(
        &engine,
        files,
        &ServerStart {
            listen: Listen::Memory(SocketAddr::from((Ipv4Addr::LOCALHOST, 28_000))),
            globals: &[],
            start: "startNothing",
            args: &[],
        },
    )
    .expect("the server starts");
    let client = ClientBuilder::from_engine(&engine, None, ClientNet::None).expect("the client");
    let mut bridge = Vm::new();
    page_bridge::install(&mut bridge);
    each(server.vm());
    each(&client.vm);
    each(&bridge);
}

#[test]
fn every_native_that_writes_or_evaluates_is_denied_to_languages() {
    // Folded `namespace::name` -> (the name as listed, the effects declared).
    let mut natives: BTreeMap<String, (String, Effects)> = BTreeMap::new();
    for_each_script_engine(|vm| {
        for (namespace, name, _, _) in vm.native_signatures() {
            let shown = if namespace.is_empty() {
                name.clone()
            } else {
                format!("{namespace}::{name}")
            };
            let effects = vm.native_effects(&namespace, &name);
            // A native that declared a denied effect is refused to a language, as a function and as a method.
            if effects.intersects(DENIED_EFFECTS) {
                for kind in [CallKind::Function, CallKind::Method] {
                    let call = PolicyCall::new(vm, &namespace, &name, kind, &[]);
                    assert!(
                        DenyCodeAndFiles.check(&call).is_err(),
                        "`{shown}` declares {effects}, but a script language may call it"
                    );
                }
            }
            // The TGE names the policy holds by name are declared where they are registered: the names are a fallback.
            if let Some((_, tge)) = TGE_EFFECTS
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(&name))
                && !vm.effects_of_name(name.as_bytes()).contains(*tge)
            {
                panic!("`{shown}` is registered without declaring {tge}");
            }
            let entry = natives
                .entry(shown.to_ascii_lowercase())
                .or_insert((shown, Effects::NONE));
            entry.1 |= effects;
        }
    });
    let listing: String = natives
        .values()
        .map(|(shown, effects)| format!("{shown} {effects}\n"))
        .collect();
    check_listing(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/natives/effects.txt"),
        &listing,
    );
}
