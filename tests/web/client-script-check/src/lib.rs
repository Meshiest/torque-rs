//! The client-script host's cases (`torque_client_script_host::cases`, which `tests/cases.rs` runs under Wasmtime) built
//! for a browser: each realm's guest runs in a Worker of the page's (`web/client-script.js`). `web/client-script-cases.js`
//! steps each case, waiting for the Workers between steps, under Node and in headless Chrome
//! (the justfile's `client-script-web-check`). Natively the crate is empty.
#![cfg(target_arch = "wasm32")]

use torque_client_script_host::cases::{CASES, CaseRun, Guest, Runners, Step};
use torque_client_script_host::{InstanceLimits, Runner, Settings, Trap, WebRunner, web_live};
use wasm_bindgen::prelude::*;

/// A panic's message goes to the console (the module then traps, and the case has failed).
#[wasm_bindgen(start)]
fn start() {
    console_error_panic_hook::set_once();
}

/// Workers of the page's runner.
struct Web;

impl Runners for Web {
    fn open(&mut self, guest: Guest, limits: InstanceLimits) -> Result<Box<dyn Runner>, Trap> {
        let wasm = match guest {
            Guest::Realm => Vec::new(),
            Guest::Wat(text) => wat::parse_str(text()).map_err(|e| Trap::Fault(e.to_string()))?,
        };
        Ok(Box::new(WebRunner::unlisted(&wasm, limits)))
    }

    fn live(&self) -> usize {
        web_live()
    }

    fn settings(&self) -> Settings {
        Settings::web()
    }
}

/// The cases, by name, in the order they are written.
#[wasm_bindgen]
pub fn cases() -> Vec<String> {
    CASES.iter().map(|case| case.name.to_string()).collect()
}

/// One case being run.
#[wasm_bindgen]
pub struct Run {
    run: CaseRun,
    error: String,
}

#[wasm_bindgen]
impl Run {
    /// A run of the case `name`, or nothing when there is none.
    pub fn of(name: &str) -> Option<Run> {
        let case = CASES.iter().find(|case| case.name == name)?;
        Some(Run {
            run: CaseRun::new(case, Box::new(Web)),
            error: String::new(),
        })
    }

    /// Steps the case: 0 while a Worker's entry is in flight (wait for `settled()`, then step again), 1 when it
    /// passed, 2 when it failed ([`Run::error`] says why).
    pub fn step(&mut self) -> u8 {
        match self.run.step() {
            Step::Waiting => 0,
            Step::Done(Ok(())) => 1,
            Step::Done(Err(error)) => {
                self.error = error;
                2
            }
        }
    }

    /// Why the case failed.
    pub fn error(&self) -> String {
        self.error.clone()
    }
}
