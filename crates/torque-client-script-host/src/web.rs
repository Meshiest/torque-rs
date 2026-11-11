//! The web runner: the realm's guest in a Worker of the page's, run by `web/client-script.js`, which the page installs
//! as `globalThis.torqueClientScript` before the game's module starts. An entry is posted to the Worker and its outcome
//! picked up on a later poll; the Worker's watchdog ends it (and the Worker) past its deadline. The module's memory
//! maximum is set to the realm's cap before it is compiled, so a growth past it fails there as it does natively.

use std::time::Duration;

use js_sys::{Reflect, Uint8Array};
use wasm_bindgen::prelude::*;

use crate::guests::{GuestRejected, check_guest};
use crate::runner::{Entry, InstanceLimits, Outcome, Reply, Runner, Trap, gone};

#[wasm_bindgen]
extern "C" {
    /// One realm's Worker (`web/client-script.js`'s `open`).
    type JsRealm;

    #[wasm_bindgen(js_namespace = torqueClientScript, js_name = open)]
    fn open_realm(wasm: &[u8], deadline_ms: f64, memory: f64) -> JsRealm;

    #[wasm_bindgen(js_namespace = torqueClientScript, js_name = live)]
    fn live() -> u32;

    #[wasm_bindgen(method)]
    fn start(this: &JsRealm, kind: u32, now_us: f64, a: &[u8], b: &[u8], c: &[u8]);

    #[wasm_bindgen(method)]
    fn take(this: &JsRealm) -> JsValue;

    #[wasm_bindgen(method)]
    fn close(this: &JsRealm);
}

/// The Workers that exist.
pub fn web_live() -> usize {
    live() as usize
}

/// Whether the page started its runner (`globalThis.torqueClientScript`, with its `open`): a page built with `web/build.mjs
/// --client-scripts` whose guest passed the build's list. Without it [`WebRunner`] cannot open a realm, so a game offers no
/// runner and its client tells servers it cannot run their scripts.
pub fn page_runner_ready() -> bool {
    let key = |name: &str| JsValue::from_str(name);
    Reflect::get(&js_sys::global(), &key("torqueClientScript"))
        .ok()
        .filter(JsValue::is_object)
        .and_then(|runner| Reflect::get(&runner, &key("open")).ok())
        .is_some_and(|open| open.is_function())
}

/// One realm's instance in a Worker.
pub struct WebRunner {
    realm: Option<JsRealm>,
    /// The outcome of an entry started on a gone instance.
    gone: bool,
}

impl WebRunner {
    /// A fresh instance of `wasm` (empty: the page's realm guest, which `web/client-script.js` checked against the build's
    /// list when the page started) within `limits`.
    ///
    /// # Errors
    ///
    /// [`GuestRejected`] when `wasm` is given and its hash is not one this build accepts.
    pub fn open(wasm: &[u8], limits: InstanceLimits) -> Result<WebRunner, GuestRejected> {
        if !wasm.is_empty() {
            check_guest(wasm)?;
        }
        Ok(WebRunner::open_unchecked(wasm, limits))
    }

    /// [`WebRunner::open`] without the list: for the guest a test has just built.
    #[cfg(feature = "unreviewed-tiers")]
    pub fn unlisted(wasm: &[u8], limits: InstanceLimits) -> WebRunner {
        WebRunner::open_unchecked(wasm, limits)
    }

    fn open_unchecked(wasm: &[u8], limits: InstanceLimits) -> WebRunner {
        WebRunner {
            realm: Some(open_realm(
                wasm,
                limits.deadline.as_secs_f64() * 1000.0,
                limits.memory as f64,
            )),
            gone: false,
        }
    }
}

impl Drop for WebRunner {
    fn drop(&mut self) {
        if let Some(realm) = self.realm.take() {
            realm.close();
        }
    }
}

impl Runner for WebRunner {
    fn start(&mut self, entry: Entry) {
        let Some(realm) = self.realm.as_ref() else {
            self.gone = true;
            return;
        };
        let none: &[u8] = &[];
        let config;
        let (now_us, a, b, c): (u64, &[u8], &[u8], &[u8]) = match &entry {
            Entry::New(c) => {
                config = c.to_bytes();
                (0, &config, none, none)
            }
            Entry::AddScript {
                mod_id,
                path,
                source,
            } => (0, mod_id.as_bytes(), path.as_bytes(), source),
            Entry::Run {
                now_us,
                mod_id,
                path,
            } => (*now_us, mod_id.as_bytes(), path.as_bytes(), none),
            Entry::Frame { now_us, events } => (*now_us, events, none, none),
        };
        realm.start(entry.code(), now_us as f64, a, b, c);
    }

    fn poll(&mut self) -> Option<Outcome> {
        if std::mem::take(&mut self.gone) {
            return Some(Err(gone()));
        }
        let value = self.realm.as_ref()?.take();
        if value.is_undefined() {
            return None;
        }
        let outcome = outcome_of(&value);
        if outcome.is_err()
            && let Some(realm) = self.realm.take()
        {
            realm.close();
        }
        Some(outcome)
    }
}

/// The outcome a Worker's reply `value` reports: a malformed one is a fault, never a guess.
fn outcome_of(value: &JsValue) -> Outcome {
    let field =
        |name: &str| Reflect::get(value, &JsValue::from_str(name)).unwrap_or(JsValue::UNDEFINED);
    let malformed = || Trap::Fault("the Worker's reply does not follow the runner's form".into());
    let whole = |v: JsValue, max: f64| {
        v.as_f64()
            .filter(|n| n.fract() == 0.0 && (0.0..=max).contains(&n.abs()))
            .ok_or_else(malformed)
    };
    if let Some(trap) = field("trap").as_string() {
        return Err(match trap.as_str() {
            "deadline" => Trap::Deadline,
            "memory" => Trap::Memory,
            _ => Trap::Fault(
                field("detail")
                    .as_string()
                    .unwrap_or_else(|| "the Worker failed".into()),
            ),
        });
    }
    let status = whole(field("status"), f64::from(i32::MAX))? as i32;
    let pending = whole(field("pending"), f64::from(u32::MAX))? as u32;
    let elapsed = field("ms")
        .as_f64()
        .filter(|ms| ms.is_finite() && *ms >= 0.0)
        .ok_or_else(malformed)?;
    let ops = field("ops")
        .dyn_into::<Uint8Array>()
        .map_err(|_| malformed())?;
    Ok(Reply {
        status,
        ops: ops.to_vec(),
        pending,
        elapsed: Duration::from_secs_f64(elapsed / 1000.0),
        long: false,
    })
}
