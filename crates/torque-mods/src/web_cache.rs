//! The browser's backing for the cache of downloaded content: an IndexedDB database of the page's origin.
//!
//! The cache is one [`MirroredStore`]: the page's first call ([`open_browser_cache`]) reads the database into memory, and
//! from then on the cache runs on memory and each change is sent to the database in a transaction of its own, which the browser
//! runs in the order they were made. Two object stores hold an entry: `content` has the bytes under the entry's name (what a
//! folder's file is on the disk) and `used` has the stamp of its last use, so a use rewrites a number and not the mod.
//!
//! Storage that is missing (a browser that refuses it in a private window, a database that does not open and read within
//! [`OPEN_MS`], any error on the way) leaves the cache in memory for the session, as does a database that refuses a write (a full
//! quota): the entry is used from memory and downloaded again by the next page. What the database held is read back as a
//! folder's files are, and bounded before it is copied out of the browser's own memory: a name that is not an entry's, a value
//! that is not bytes or does not fit the cap, and a stamp with no entry are deleted, and the values are read one at a time, most
//! recently used first, so one that is too large is never copied ([`read_all`]). The hash check on every read
//! ([`MirroredStore::load`]) comes after.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use js_sys::{Array, Promise, Uint8Array};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{JsFuture, future_to_promise};
use web_sys::{Event, IdbDatabase, IdbRequest, IdbTransactionMode};

use crate::cache::{CacheStore, Listed, MAX_ENTRIES, index_of};
use crate::mirror::{Backing, MirroredStore, Record};

/// The database's name.
const DATABASE: &str = "torque-rs-mod-content";
/// The object store of entries' bytes.
const CONTENT: &str = "content";
/// The object store of entries' stamps.
const USED: &str = "used";
/// How long the database has to open and be read before the cache stays in memory (a browser that never answers must not keep a
/// game from starting).
const OPEN_MS: i32 = 5_000;

/// The most bytes the browser's cache holds, in memory and in the database (what the cache keeps is all in the page's memory).
pub const WEB_CACHE_BYTES: u64 = 128 << 20;

type Shared = Rc<RefCell<MirroredStore<Idb>>>;

thread_local! {
    static OPENED: RefCell<Option<Shared>> = const { RefCell::new(None) };
}

/// Opens the page's database and reads the cache's entries from it, so a [`crate::CacheChoice::PerUser`] cache of the engines made
/// after this keeps what it downloads across page loads. Call it once before the first engine is made, from the page's start.
/// Returns whether storage is in use. It never fails: every error (no storage, a refusal, a database that is broken, one that does
/// not open and read within five seconds) leaves every cache in memory for the session and gives `false`.
pub async fn open_browser_cache() -> bool {
    if is_open() {
        return true;
    }
    let abandoned = Rc::new(Cell::new(false));
    let flag = Rc::clone(&abandoned);
    let work = future_to_promise(async move {
        if let Some((db, store)) = load().await {
            if flag.get() {
                db.close();
            } else {
                OPENED.with(|slot| *slot.borrow_mut() = Some(Rc::new(RefCell::new(store))));
            }
        }
        Ok(JsValue::UNDEFINED)
    });
    let racers: Array = [JsValue::from(work), JsValue::from(after(OPEN_MS))]
        .into_iter()
        .collect();
    let _ = JsFuture::from(Promise::race(&racers)).await;
    abandoned.set(true);
    is_open()
}

fn is_open() -> bool {
    OPENED.with(|slot| slot.borrow().is_some())
}

/// The open database and the store its entries make, `None` when anything fails.
async fn load() -> Option<(IdbDatabase, MirroredStore<Idb>)> {
    let db = open().await.ok()?;
    let Ok((records, doomed)) = read_all(&db).await else {
        db.close();
        return None;
    };
    // Errors of requests the cache makes are the cache's to ignore: the page's console is not told of them.
    let quiet = Closure::<dyn FnMut(Event)>::new(|event: Event| event.prevent_default());
    db.set_onerror(Some(quiet.as_ref().unchecked_ref()));
    quiet.forget();
    let idb = Idb { db: db.clone() };
    for name in &doomed {
        idb.remove(name);
    }
    Some((db, MirroredStore::load(idb, records, WEB_CACHE_BYTES)))
}

/// The cache's store for an engine made now: the opened database's, `None` when [`open_browser_cache`] did not open one.
pub(crate) fn opened() -> Option<Box<dyn CacheStore>> {
    OPENED.with(|slot| slot.borrow().clone().map(|s| Box::new(Handle(s)) as _))
}

/// The one store the engines of a page share.
struct Handle(Shared);

impl CacheStore for Handle {
    fn list(&self) -> Vec<Listed> {
        self.0.borrow().list()
    }

    fn read(&self, name: &str, max: u64) -> Option<Vec<u8>> {
        self.0.borrow().read(name, max)
    }

    fn read_head(&self, name: &str, n: usize) -> Option<Vec<u8>> {
        self.0.borrow().read_head(name, n)
    }

    fn write(&mut self, name: &str, bytes: &[u8]) -> bool {
        self.0.borrow_mut().write(name, bytes)
    }

    fn remove(&mut self, name: &str) {
        self.0.borrow_mut().remove(name);
    }

    fn touch(&mut self, name: &str, stamp: u64) {
        self.0.borrow_mut().touch(name, stamp);
    }

    fn limit(&self) -> u64 {
        self.0.borrow().limit()
    }
}

/// A request's outcome as a promise, with the handlers that settle it.
struct Settled {
    promise: Promise,
    handlers: Vec<Closure<dyn FnMut(Event)>>,
}

/// Starts waiting for `request`: its handlers are set now, so the request may finish before [`Settled::wait`] is called.
fn settled(request: &IdbRequest) -> Settled {
    let mut handlers = Vec::new();
    let promise = Promise::new(&mut |resolve, reject| {
        let source = request.clone();
        let done = Closure::<dyn FnMut(Event)>::new(move |_| {
            let result = source.result().unwrap_or(JsValue::UNDEFINED);
            let _ = resolve.call1(&JsValue::UNDEFINED, &result);
        });
        let failed = Closure::<dyn FnMut(Event)>::new(move |event: Event| {
            event.prevent_default();
            let _ = reject.call1(&JsValue::UNDEFINED, &JsValue::from_str("a request failed"));
        });
        request.set_onsuccess(Some(done.as_ref().unchecked_ref()));
        request.set_onerror(Some(failed.as_ref().unchecked_ref()));
        handlers = vec![done, failed];
    });
    Settled { promise, handlers }
}

impl Settled {
    /// The request's result; its handlers are freed once it has settled.
    async fn wait(self) -> Result<JsValue, JsValue> {
        let result = JsFuture::from(self.promise).await;
        drop(self.handlers);
        result
    }
}

/// A promise that settles with nothing after `ms` milliseconds.
fn after(ms: i32) -> Promise {
    Promise::new(&mut |resolve, _| {
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        }
    })
}

fn names(stores: &[&str]) -> Array {
    stores.iter().map(|s| JsValue::from_str(s)).collect()
}

async fn open() -> Result<IdbDatabase, JsValue> {
    let factory = web_sys::window()
        .ok_or("no window")?
        .indexed_db()?
        .ok_or("no IndexedDB")?;
    let request = factory.open_with_u32(DATABASE, 1)?;
    let creating = request.clone();
    let upgrade = Closure::<dyn FnMut(Event)>::new(move |_| {
        if let Ok(result) = creating.result()
            && let Ok(db) = result.dyn_into::<IdbDatabase>()
        {
            let _ = db.create_object_store(CONTENT);
            let _ = db.create_object_store(USED);
        }
    });
    request.set_onupgradeneeded(Some(upgrade.as_ref().unchecked_ref()));
    let opened = settled(&request).wait().await?;
    drop(upgrade);
    opened
        .dyn_into::<IdbDatabase>()
        .map_err(|_| "the database did not open".into())
}

/// The entries the database holds, read as far as the cap allows (at most twice the entries a cache lists), and the names of what
/// is to be deleted instead: names that are not entries', values that are not bytes or do not fit, and stamps with no entry.
async fn read_all(db: &IdbDatabase) -> Result<(Vec<Record>, Vec<String>), JsValue> {
    let tx = db.transaction_with_str_sequence(&names(&[CONTENT, USED]))?;
    let content = tx.object_store(CONTENT)?;
    let used = tx.object_store(USED)?;
    let limit = u32::try_from(MAX_ENTRIES.saturating_mul(2)).unwrap_or(u32::MAX);
    let all = JsValue::UNDEFINED;
    let content_keys = settled(&content.get_all_keys_with_key_and_limit(&all, limit)?);
    let used_keys = settled(&used.get_all_keys_with_key_and_limit(&all, limit)?);
    let used_values = settled(&used.get_all_with_key_and_limit(&all, limit)?);
    let array = |value: JsValue| value.dyn_into::<Array>().unwrap_or_default();
    let content_keys = array(content_keys.wait().await?);
    let used_keys = array(used_keys.wait().await?);
    let used_values = array(used_values.wait().await?);
    let stamps: HashMap<String, u64> = used_keys
        .iter()
        .zip(used_values.iter())
        .filter_map(|(key, value)| Some((key.as_string()?, value.as_f64()? as u64)))
        .collect();
    let mut found: Vec<String> = content_keys.iter().filter_map(|k| k.as_string()).collect();
    let present: HashSet<&String> = found.iter().collect();
    let mut doomed: Vec<String> = stamps
        .keys()
        .filter(|name| !present.contains(name))
        .cloned()
        .collect();
    found.retain(|name| {
        let entry = [Listed {
            name: name.clone(),
            size: 0,
            stamp: 0,
        }];
        let valid = !index_of(&entry).is_empty();
        if !valid {
            doomed.push(name.clone());
        }
        valid
    });
    found.sort_by_key(|name| std::cmp::Reverse(stamps.get(name).copied().unwrap_or(0)));
    let mut room = WEB_CACHE_BYTES;
    let mut records = Vec::new();
    for name in found {
        let Ok(value) = read_value(db, &name).await else {
            continue;
        };
        let Ok(bytes) = value.dyn_into::<Uint8Array>() else {
            doomed.push(name);
            continue;
        };
        let len = u64::from(bytes.length());
        if len > room {
            doomed.push(name);
            continue;
        }
        room -= len;
        let stamp = stamps.get(&name).copied().unwrap_or(0);
        records.push(Record {
            name,
            bytes: bytes.to_vec(),
            stamp,
        });
    }
    Ok((records, doomed))
}

/// One entry's stored value, in a transaction of its own (one that waits between requests would end).
async fn read_value(db: &IdbDatabase, name: &str) -> Result<JsValue, JsValue> {
    let tx = db.transaction_with_str(CONTENT)?;
    let request = tx.object_store(CONTENT)?.get(&JsValue::from_str(name))?;
    settled(&request).wait().await
}

/// The database a cache mirrors to. Every call makes a transaction and leaves it to the browser; one that cannot be made (the
/// connection closed) or fails is not the cache's concern.
struct Idb {
    db: IdbDatabase,
}

impl Idb {
    fn write(
        &self,
        stores: &[&str],
        work: impl FnOnce(&web_sys::IdbTransaction) -> Result<(), JsValue>,
    ) {
        let Ok(tx) = self
            .db
            .transaction_with_str_sequence_and_mode(&names(stores), IdbTransactionMode::Readwrite)
        else {
            return;
        };
        let _ = work(&tx);
    }
}

impl Backing for Idb {
    fn put(&self, name: &str, bytes: &[u8]) {
        self.write(&[CONTENT], |tx| {
            tx.object_store(CONTENT)?
                .put_with_key(&Uint8Array::from(bytes), &JsValue::from_str(name))
                .map(drop)
        });
    }

    fn remove(&self, name: &str) {
        self.write(&[CONTENT, USED], |tx| {
            let key = JsValue::from_str(name);
            tx.object_store(CONTENT)?.delete(&key)?;
            tx.object_store(USED)?.delete(&key).map(drop)
        });
    }

    fn stamp(&self, name: &str, stamp: u64) {
        self.write(&[USED], |tx| {
            tx.object_store(USED)?
                .put_with_key(&JsValue::from_f64(stamp as f64), &JsValue::from_str(name))
                .map(drop)
        });
    }
}
