//! Every case of `cases` under Wasmtime. The realm's guest is `torque_lua_client.wasm` (`just lua-client-web`), found at
//! TORQUE_CLIENT_GUEST, else in the build folder's `lua-web`; without it the cases that need it are skipped, unless
//! TORQUE_CLIENT_GUEST_REQUIRED is set (the `client-script-host-test` recipe sets it).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use torque_client_script_host::cases::{CASES, CaseRun, Guest, Runners, Step};
use torque_client_script_host::{GuestEngine, InstanceLimits, RealmGuest, Runner, Settings, Trap};

/// Wasmtime runners over one engine.
struct Native {
    engine: GuestEngine,
    realm: Option<RealmGuest>,
    /// Every guest compiled, for counting their instances.
    guests: Vec<RealmGuest>,
}

impl Runners for Native {
    fn open(&mut self, guest: Guest, limits: InstanceLimits) -> Result<Box<dyn Runner>, Trap> {
        let guest = match guest {
            Guest::Realm => self
                .realm
                .clone()
                .ok_or_else(|| Trap::Fault("no realm guest".into()))?,
            Guest::Wat(text) => {
                let wasm = wat::parse_str(text()).map_err(|e| Trap::Fault(e.to_string()))?;
                let guest = RealmGuest::unlisted(&self.engine, &wasm)
                    .map_err(|e| Trap::Fault(e.to_string()))?;
                self.guests.push(guest.clone());
                guest
            }
        };
        Ok(Box::new(guest.open(limits)?))
    }

    fn live(&self) -> usize {
        self.realm
            .iter()
            .chain(&self.guests)
            .map(RealmGuest::live)
            .sum()
    }

    fn settings(&self) -> Settings {
        Settings::native()
    }
}

fn guest_path() -> PathBuf {
    if let Some(path) = std::env::var_os("TORQUE_CLIENT_GUEST") {
        return path.into();
    }
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"),
        PathBuf::from,
    );
    target.join("lua-web").join("torque_lua_client.wasm")
}

#[test]
fn every_case_passes_under_wasmtime() {
    let engine = GuestEngine::new(Duration::from_millis(5)).expect("an engine");
    let path = guest_path();
    let realm = match std::fs::read(&path) {
        Ok(bytes) => Some(RealmGuest::unlisted(&engine, &bytes).expect("the realm guest links")),
        Err(error) => {
            assert!(
                std::env::var_os("TORQUE_CLIENT_GUEST_REQUIRED").is_none(),
                "{}: {error} (build it with `just lua-client-web`)",
                path.display()
            );
            None
        }
    };
    let (mut passed, mut skipped, mut failed) = (0, 0, Vec::new());
    for case in CASES {
        if realm.is_none() && case.needs_realm_guest() {
            skipped += 1;
            continue;
        }
        let runners = Native {
            engine: engine.clone(),
            realm: realm.clone(),
            guests: Vec::new(),
        };
        let started = Instant::now();
        let outcome = match CaseRun::new(case, Box::new(runners)).step() {
            Step::Done(outcome) => outcome,
            Step::Waiting => Err(format!("{}: a native runner waited", case.name)),
        };
        println!(
            "native: {} {} ({:.2} s)",
            if outcome.is_ok() { "ok" } else { "FAILED" },
            case.name,
            started.elapsed().as_secs_f64()
        );
        match outcome {
            Ok(()) => passed += 1,
            Err(error) => failed.push(error),
        }
    }
    println!(
        "native: {passed} of {} cases passed, {skipped} skipped",
        CASES.len()
    );
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

#[test]
fn a_guest_off_the_list_is_refused_before_it_is_compiled() {
    let engine = GuestEngine::new(Duration::from_millis(5)).expect("an engine");
    let wasm = wat::parse_str("(module)").expect("a module");
    let error = RealmGuest::new(&engine, &wasm)
        .err()
        .expect("no release lists this module");
    assert!(
        error.to_string().contains("is not one this build accepts"),
        "{error}"
    );
    // The test constructor skips the list and nothing else.
    assert!(RealmGuest::unlisted(&engine, &wasm).is_ok());
    assert!(RealmGuest::unlisted(&engine, b"not a module").is_err());
}
