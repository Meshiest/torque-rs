//! The bridge between a web build's page and the game's scripts (the client's script engine).
//!
//! **Page to script** (the page's `torque` object, `web/bridge.js`): `call(fn, ...args)` calls a script function and gives
//! what it returned, `eval(text)` runs script text (a syntax error comes back as an error with its line and column, and
//! nothing of the text runs), `getVar(name)` and `setVar(name, value)` read and set a global (`$name`). Every request is a
//! promise: the window serves the requests at its next frame ([`serve`]), at most [`MAX_SERVED_PER_FRAME`] a frame, in the
//! order they came. Requests made before the game is running wait for it, at most [`MAX_QUEUED`]; past that they are
//! refused. The page's author controls the page, so the page may run any script: these are the page's own calls.
//!
//! **Script to page.** The page registers named handlers (`torque.on(name, fn)`, [`register_handler`]); a script calls one
//! with `callPage(name, args...)` and gets what it returned as a string ("" for a name the page did not register: an
//! event no page listens to reaches nothing). Script can reach nothing else of the page: there is no way for script to run
//! the page's own code, and it emits page events (a map loaded, the player died) only through the handlers the page chose to
//! register.
//!
//! **A remote server never reaches the page directly.** A server runs script on the client only through the game's
//! `clientCmd` functions (`commandToClient`). A game that wants a server's message to reach the page writes a `clientCmd`
//! that itself calls `callPage` with a registered name; it must not hand a name or code the server sent to `callPage`, nor
//! register a handler that runs what it is given.
//!
//! **Caps.** Every argument is a string of at most [`MAX_ARG_BYTES`], at most [`MAX_ARGS`] of them; names are identifiers of
//! at most [`MAX_NAME_BYTES`]; an `eval` text is at most [`MAX_TEXT_BYTES`]; script calls the page at most
//! [`MAX_PAGE_CALLS_PER_FRAME`] times a frame.
//!
//! The script engine also gets `setRenderQuality(name)` (the renderer's quality from the next frame,
//! [`crate::web_page::request_quality`]): the page's settings panel calls it.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, VecDeque};
use std::rc::Rc;

use torque_ts_vm::vm::{Native, Vm};

/// The most arguments a call carries (either way).
pub const MAX_ARGS: usize = 16;
/// The most bytes one argument (or a variable's value) has.
pub const MAX_ARG_BYTES: usize = 4096;
/// The longest name of a function, a handler or a variable, in bytes.
pub const MAX_NAME_BYTES: usize = 64;
/// The most bytes of script text one `eval` runs.
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
/// The most requests from the page that wait to be served.
pub const MAX_QUEUED: usize = 256;
/// The most requests from the page served in one frame (the rest wait for the next).
pub const MAX_SERVED_PER_FRAME: usize = 64;
/// The most times script calls the page in one frame.
pub const MAX_PAGE_CALLS_PER_FRAME: u32 = 32;
/// The most handlers the page registers.
pub const MAX_HANDLERS: usize = 64;

/// Why a request or a call across the bridge was refused or failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeError {
    /// More than [`MAX_ARGS`] arguments.
    TooManyArgs(usize),
    /// An argument (or a value, or an `eval` text) over its cap: its place and its length.
    TooLong { what: String, bytes: usize },
    /// A name that is not an identifier of at most [`MAX_NAME_BYTES`].
    BadName(String),
    /// No script function of that name.
    NoFunction(String),
    /// `callPage` to a name the page did not register.
    NotRegistered(String),
    /// More than [`MAX_PAGE_CALLS_PER_FRAME`] calls to the page this frame, or [`MAX_HANDLERS`] handlers.
    TooManyCalls,
    /// [`MAX_QUEUED`] requests already wait.
    QueueFull,
    /// The page has no handlers (natively, or before the page set its dispatcher).
    NoPage,
    /// An `eval` text with a syntax error: nothing of it ran. The line is the text's (from 1), the column the byte of that
    /// line the parser stopped at (from 1), when the report shows it.
    Syntax { line: u32, column: Option<u32> },
    /// The script engine stopped the run, or the page's handler threw.
    Failed(String),
}

impl BridgeError {
    /// A short name for the kind of error (the page's `error.kind`).
    pub fn kind(&self) -> &'static str {
        match self {
            BridgeError::TooManyArgs(_) => "too-many-args",
            BridgeError::TooLong { .. } => "too-long",
            BridgeError::BadName(_) => "bad-name",
            BridgeError::NoFunction(_) => "no-function",
            BridgeError::NotRegistered(_) => "not-registered",
            BridgeError::TooManyCalls => "too-many-calls",
            BridgeError::QueueFull => "queue-full",
            BridgeError::NoPage => "no-page",
            BridgeError::Syntax { .. } => "syntax",
            BridgeError::Failed(_) => "failed",
        }
    }

    /// The error as JSON for the page: `kind`, `message`, and `line` and `column` for a syntax error.
    pub fn to_json(&self) -> String {
        let mut v = serde_json::json!({ "kind": self.kind(), "message": self.to_string() });
        if let BridgeError::Syntax { line, column } = self {
            v["line"] = (*line).into();
            if let Some(c) = column {
                v["column"] = (*c).into();
            }
        }
        v.to_string()
    }
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BridgeError::TooManyArgs(n) => write!(f, "{n} arguments (at most {MAX_ARGS})"),
            BridgeError::TooLong { what, bytes } => {
                write!(f, "{what} has {bytes} bytes (over the cap)")
            }
            BridgeError::BadName(n) => write!(
                f,
                "{n:?} is not a name (an identifier of at most {MAX_NAME_BYTES} bytes)"
            ),
            BridgeError::NoFunction(n) => write!(f, "no script function {n}"),
            BridgeError::NotRegistered(n) => write!(f, "the page registered no handler {n:?}"),
            BridgeError::TooManyCalls => write!(f, "too many calls"),
            BridgeError::QueueFull => write!(f, "{MAX_QUEUED} requests already wait"),
            BridgeError::NoPage => write!(f, "no page to call"),
            BridgeError::Syntax { line, column } => match column {
                Some(c) => write!(f, "syntax error at line {line}, column {c}"),
                None => write!(f, "syntax error at line {line}"),
            },
            BridgeError::Failed(e) => write!(f, "{e}"),
        }
    }
}

/// A request from the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Call { function: String, args: Vec<String> },
    Eval { text: String },
    GetVar { name: String },
    SetVar { name: String, value: String },
}

/// Where a request's answer goes (the page's promise).
pub type Reply = Box<dyn FnOnce(Result<String, BridgeError>)>;

/// The page's side of `callPage`: its handler `name` called with the arguments, and what it returned (Err: it threw).
pub type PageDispatch = Rc<dyn Fn(&str, &[String]) -> Result<String, String>>;

#[derive(Default)]
struct Bridge {
    queue: VecDeque<(Request, Reply)>,
    handlers: BTreeSet<String>,
    page: Option<PageDispatch>,
}

thread_local! {
    static BRIDGE: RefCell<Bridge> = RefCell::default();
    static PAGE_CALLS: Cell<u32> = const { Cell::new(0) };
}

/// Whether `name` is an identifier (`[A-Za-z_][A-Za-z0-9_]*`) of at most [`MAX_NAME_BYTES`].
pub fn is_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= MAX_NAME_BYTES
        && bytes
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// A global's name as the engine knows it (`$` and identifiers joined by `::`; the `$` may be left out).
pub fn var_name(name: &str) -> Result<String, BridgeError> {
    let bare = name.strip_prefix('$').unwrap_or(name);
    let ok = bare.len() <= MAX_NAME_BYTES && !bare.is_empty() && bare.split("::").all(is_name);
    if ok {
        Ok(format!("${bare}"))
    } else {
        Err(BridgeError::BadName(name.to_string()))
    }
}

/// The arguments within the caps.
pub fn check_args(args: &[String]) -> Result<(), BridgeError> {
    if args.len() > MAX_ARGS {
        return Err(BridgeError::TooManyArgs(args.len()));
    }
    match args.iter().position(|a| a.len() > MAX_ARG_BYTES) {
        Some(i) => Err(BridgeError::TooLong {
            what: format!("argument {}", i + 1),
            bytes: args[i].len(),
        }),
        None => Ok(()),
    }
}

/// The request within the caps.
pub fn check(request: &Request) -> Result<(), BridgeError> {
    match request {
        Request::Call { function, args } => {
            if !is_name(function) {
                return Err(BridgeError::BadName(function.clone()));
            }
            check_args(args)
        }
        Request::Eval { text } if text.len() > MAX_TEXT_BYTES => Err(BridgeError::TooLong {
            what: "the text".into(),
            bytes: text.len(),
        }),
        Request::Eval { .. } => Ok(()),
        Request::GetVar { name } => var_name(name).map(|_| ()),
        Request::SetVar { name, value } => {
            var_name(name)?;
            if value.len() > MAX_ARG_BYTES {
                return Err(BridgeError::TooLong {
                    what: "the value".into(),
                    bytes: value.len(),
                });
            }
            Ok(())
        }
    }
}

/// Takes a request from the page: refused at once (through `reply`) when it breaks a cap or [`MAX_QUEUED`] wait; else it
/// waits for [`serve`].
pub fn request(request: Request, reply: Reply) {
    if let Err(e) = check(&request) {
        reply(Err(e));
        return;
    }
    let refused = BRIDGE.with_borrow_mut(|b| {
        if b.queue.len() >= MAX_QUEUED {
            return Some(reply);
        }
        b.queue.push_back((request, reply));
        None
    });
    if let Some(reply) = refused {
        reply(Err(BridgeError::QueueFull));
    }
}

/// The requests waiting.
pub fn queued() -> usize {
    BRIDGE.with_borrow(|b| b.queue.len())
}

/// The page registers its handler `name` (`torque.on`).
pub fn register_handler(name: &str) -> Result<(), BridgeError> {
    if !is_name(name) {
        return Err(BridgeError::BadName(name.to_string()));
    }
    BRIDGE.with_borrow_mut(|b| {
        if b.handlers.len() >= MAX_HANDLERS && !b.handlers.contains(name) {
            return Err(BridgeError::TooManyCalls);
        }
        b.handlers.insert(name.to_string());
        Ok(())
    })
}

/// The page drops its handler `name` (`torque.off`).
pub fn unregister_handler(name: &str) {
    BRIDGE.with_borrow_mut(|b| b.handlers.remove(name));
}

/// Whether the page registered `name`.
pub fn registered(name: &str) -> bool {
    BRIDGE.with_borrow(|b| b.handlers.contains(name))
}

/// Sets how `callPage` reaches the page's handlers (in a browser, the page's `torque.dispatch`).
pub fn set_page(page: Option<PageDispatch>) {
    BRIDGE.with_borrow_mut(|b| b.page = page);
}

/// Script calls the page's handler `name` with `args` (`callPage`): refused for a name the page did not register, past the
/// caps, or with no page. The handler runs with nothing of the bridge held, so it may make requests of its own.
pub fn call_page(name: &str, args: &[String]) -> Result<String, BridgeError> {
    check_args(args)?;
    let page = BRIDGE.with_borrow(|b| {
        if !b.handlers.contains(name) {
            return Err(BridgeError::NotRegistered(name.to_string()));
        }
        b.page.clone().ok_or(BridgeError::NoPage)
    })?;
    let calls = PAGE_CALLS.get();
    if calls >= MAX_PAGE_CALLS_PER_FRAME {
        return Err(BridgeError::TooManyCalls);
    }
    PAGE_CALLS.set(calls + 1);
    page(name, args).map_err(BridgeError::Failed)
}

/// Gives `vm` the bridge's natives: `callPage(name, args...)` and `setRenderQuality(name)`.
pub fn install(vm: &mut Vm) {
    let max = i32::try_from(MAX_ARGS + 2).unwrap_or(i32::MAX);
    vm.register("callPage", 2, max, "callPage(name, args...);", |vm, c| {
        let text = |a: &Vec<u8>| String::from_utf8_lossy(a).into_owned();
        let name = c.argv.get(1).map(text).unwrap_or_default();
        let args: Vec<String> = c.rest(2).iter().map(text).collect();
        match call_page(&name, &args) {
            Ok(out) => Native::Str(out.into_bytes()),
            // an event no page listens to
            Err(BridgeError::NotRegistered(_)) => Native::Str(Vec::new()),
            Err(e) => {
                vm.error(format!("callPage: {name}: {e}"));
                Native::Str(Vec::new())
            }
        }
    });
    vm.register(
        "setRenderQuality",
        2,
        2,
        "setRenderQuality(low|medium|high|ultra);",
        |vm, c| {
            let name = String::from_utf8_lossy(c.arg(1)).into_owned();
            match crate::web_page::request_quality(&name) {
                Ok(()) => Native::Bool(true),
                Err(e) => {
                    vm.error(format!("setRenderQuality: {e}"));
                    Native::Bool(false)
                }
            }
        },
    );
}

/// The window's call once a frame: the count of calls to the page starts again, and up to [`MAX_SERVED_PER_FRAME`] waiting
/// requests are run on `vm` and answered, oldest first. Nothing of the bridge is held while one runs.
pub fn serve(vm: &mut Vm) {
    PAGE_CALLS.set(0);
    for _ in 0..MAX_SERVED_PER_FRAME {
        let Some((request, reply)) = BRIDGE.with_borrow_mut(|b| b.queue.pop_front()) else {
            break;
        };
        reply(run(vm, &request));
    }
}

/// Runs one request on `vm`.
pub fn run(vm: &mut Vm, request: &Request) -> Result<String, BridgeError> {
    check(request)?;
    let text = |v: Vec<u8>| String::from_utf8_lossy(&v).into_owned();
    match request {
        Request::Call { function, args } => {
            let exists = vm
                .invoke(None, b"isFunction", &[function.as_bytes().to_vec()])
                .map(text)
                .unwrap_or_default();
            if exists != "1" {
                return Err(BridgeError::NoFunction(function.clone()));
            }
            let argv: Vec<Vec<u8>> = args.iter().map(|a| a.as_bytes().to_vec()).collect();
            vm.invoke(None, function.as_bytes(), &argv)
                .map(text)
                .map_err(|e| BridgeError::Failed(format!("{function}: {e}")))
        }
        Request::Eval { text: source } => {
            syntax_check(source)?;
            vm.eval(source.as_bytes())
                .map(text)
                .map_err(|e| BridgeError::Failed(e.to_string()))
        }
        Request::GetVar { name } => Ok(vm.global(&var_name(name)?).map(text).unwrap_or_default()),
        Request::SetVar { name, value } => {
            vm.set_global(&var_name(name)?, value);
            Ok(String::new())
        }
    }
}

/// Parses `source` as the engine's scripts are parsed: Err with the line and column of the first syntax error.
pub fn syntax_check(source: &str) -> Result<(), BridgeError> {
    use torque_ts_compiler::parser;
    const FILE: &[u8] = b"page";
    let parsed = parser::parse(source.as_bytes(), Some(FILE));
    if !parsed.syntax_error {
        return Ok(());
    }
    let report: Vec<String> = parsed
        .diagnostics
        .iter()
        .map(|d| String::from_utf8_lossy(&d.text).into_owned())
        .collect();
    Err(syntax_place(source, &report))
}

/// The line and column of a syntax error in `source` from the parser's report (`page Line: <n> - Syntax error.`, then the
/// text around the place, `##` on both sides of the byte the scanner had reached): the column is that byte's on its line (from
/// 1), when the text around it is found on that line of `source`. A scanner that reached the end of the text (the report
/// stops at the first `##`) places the error at the text's last character that is not white space.
pub fn syntax_place(source: &str, report: &[String]) -> BridgeError {
    let line = report
        .iter()
        .find_map(|l| {
            let rest = l.split("Line: ").nth(1)?;
            rest.split(|c: char| !c.is_ascii_digit())
                .next()?
                .parse()
                .ok()
        })
        .unwrap_or(1);
    // The column of the byte at `place` when it is on the reported line.
    let column_at = |place: usize| {
        let line_start = source[..place].rfind('\n').map_or(0, |i| i + 1);
        let on = 1 + source[..place].matches('\n').count();
        if u32::try_from(on).ok()? != line {
            return None;
        }
        u32::try_from(place - line_start + 1).ok()
    };
    let column = report.iter().skip(1).find_map(|l| {
        let (before, rest) = l.split_once("##")?;
        let Some((at, after)) = rest.split_once("##") else {
            // the end of the text: its last character that is not white space
            source.ends_with(before).then_some(())?;
            let last = source.trim_end().char_indices().last()?.0;
            return column_at(last);
        };
        let around = format!("{before}{at}{after}");
        // the occurrence of the text around the place whose place is on the reported line
        source
            .match_indices(&around)
            .find_map(|(start, _)| column_at(start + before.len()))
    });
    BridgeError::Syntax { line, column }
}

/// In a browser: the page's side of the bridge (`web/bridge.js`'s `torque` object).
#[cfg(target_arch = "wasm32")]
pub mod page {
    use std::rc::Rc;

    use wasm_bindgen::JsCast as _;
    use wasm_bindgen::prelude::*;

    use super::{BridgeError, Request};

    #[wasm_bindgen]
    extern "C" {
        /// The page's handler `name` called with `args` (the page's `torque.dispatch`).
        #[wasm_bindgen(catch, js_namespace = torque, js_name = dispatch)]
        fn page_dispatch(name: &str, args: js_sys::Array) -> Result<JsValue, JsValue>;
    }

    /// The message of something the page threw.
    fn thrown(e: &JsValue) -> String {
        e.dyn_ref::<js_sys::Error>()
            .map(|e| String::from(e.message()))
            .or_else(|| e.as_string())
            .unwrap_or_else(|| "the page's handler threw".to_string())
    }

    /// `callPage` reaches the page's handlers through `torque.dispatch`.
    pub fn connect() {
        super::set_page(Some(Rc::new(|name: &str, args: &[String]| {
            let list = js_sys::Array::new();
            for a in args {
                list.push(&JsValue::from_str(a));
            }
            page_dispatch(name, list)
                .map(|v| v.as_string().unwrap_or_default())
                .map_err(|e| thrown(&e))
        })));
    }

    /// The page's request (`torque.call`, `eval`, `getVar`, `setVar`): `kind` is `call`, `eval`, `get` or `set`, `a` the
    /// function, text or variable, `b` the value set, `args` a call's arguments; the answer goes to `resolve` (the result
    /// string) or `reject` (the error as JSON, [`BridgeError::to_json`]).
    pub fn request(
        kind: &str,
        a: String,
        b: String,
        args: Vec<String>,
        resolve: JsValue,
        reject: JsValue,
    ) {
        let (Ok(resolve), Ok(reject)) = (
            resolve.dyn_into::<js_sys::Function>(),
            reject.dyn_into::<js_sys::Function>(),
        ) else {
            return;
        };
        let reply: super::Reply = Box::new(move |answer| {
            let _ = match answer {
                Ok(out) => resolve.call1(&JsValue::NULL, &JsValue::from_str(&out)),
                Err(e) => reject.call1(&JsValue::NULL, &JsValue::from_str(&e.to_json())),
            };
        });
        let request = match kind {
            "call" => Request::Call { function: a, args },
            "eval" => Request::Eval { text: a },
            "get" => Request::GetVar { name: a },
            "set" => Request::SetVar { name: a, value: b },
            other => return reply(Err(BridgeError::Failed(format!("no request {other:?}")))),
        };
        super::request(request, reply);
    }
}

#[cfg(test)]
#[path = "page_bridge_tests.rs"]
mod tests;
