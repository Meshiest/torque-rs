//! The window's input in a browser. A page that loses the focus (another tab, another window) or the pointer lock is sent no
//! release for the keys and buttons let go meanwhile, so the window lets go of every key it saw held ([`HeldKeys`]) and every
//! button when the page loses the focus, is hidden, or loses the pointer lock. A browser keeps Esc for itself while the pointer
//! is locked: it ends the lock before the page sees the key. A lock lost while the game still holds the mouse is therefore the
//! game's Esc ([`lock_loss_is_escape`]), posted as the key's press and release, so the game's binding (a menu) runs and the
//! mouse is free over it. Unless an Esc reached the game just before ([`ESCAPE_WINDOW_MS`]): a browser that passes the key on
//! (Chromium's keyboard lock in full screen) must not have it run twice.
//!
//! `PageEvents` (web builds) listens on the page for the losses the window's own events do not carry: the window's `blur`
//! and the document's `pointerlockchange`.

use crate::input::Key;

/// How long after an Esc reached the game a lost pointer lock is taken as that same Esc (ms).
pub const ESCAPE_WINDOW_MS: u64 = 500;

/// The keys held down, as the window's key events said.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HeldKeys {
    keys: Vec<Key>,
}

impl HeldKeys {
    /// A key event: `key` went down (`down`) or up.
    pub fn key(&mut self, key: Key, down: bool) {
        if !down {
            self.keys.retain(|k| *k != key);
        } else if !self.keys.contains(&key) {
            self.keys.push(key);
        }
    }

    /// The keys held now, in the order they went down; none are held after.
    pub fn take(&mut self) -> Vec<Key> {
        std::mem::take(&mut self.keys)
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

/// What the page said since the window last looked.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PageNews {
    /// The page lost the focus (the window's `blur`).
    pub focus_lost: bool,
    /// The pointer lock ended (by the browser's Esc, or the game's own release).
    pub lock_lost: bool,
}

/// Whether a pointer lock lost at `now_ms` is the game's Esc: the game still holds the mouse (`wants_mouse`: it did not let go
/// itself) and no Esc reached it in the last [`ESCAPE_WINDOW_MS`] (`last_escape_ms`, when one did).
pub fn lock_loss_is_escape(wants_mouse: bool, last_escape_ms: Option<u64>, now_ms: u64) -> bool {
    wants_mouse && last_escape_ms.is_none_or(|t| now_ms.saturating_sub(t) >= ESCAPE_WINDOW_MS)
}

/// The page's listeners: what they saw waits in a cell until the window takes it ([`PageEvents::take`]).
#[cfg(target_arch = "wasm32")]
pub struct PageEvents {
    news: std::rc::Rc<std::cell::Cell<PageNews>>,
    _listeners: Vec<wasm_bindgen::closure::Closure<dyn FnMut(web_sys::Event)>>,
}

#[cfg(target_arch = "wasm32")]
impl PageEvents {
    /// Listens on the page's window (`blur`) and document (`pointerlockchange`); none where the page has neither.
    pub fn listen() -> PageEvents {
        use wasm_bindgen::JsCast as _;
        let news = std::rc::Rc::new(std::cell::Cell::new(PageNews::default()));
        let mut listeners = Vec::new();
        let Some(window) = web_sys::window() else {
            return PageEvents {
                news,
                _listeners: listeners,
            };
        };
        let on_blur = {
            let news = news.clone();
            wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::Event)>::new(
                move |_: web_sys::Event| {
                    news.set(PageNews {
                        focus_lost: true,
                        ..news.get()
                    });
                },
            )
        };
        let _ = window.add_event_listener_with_callback("blur", on_blur.as_ref().unchecked_ref());
        listeners.push(on_blur);
        if let Some(document) = window.document() {
            let on_lock = {
                let news = news.clone();
                let document = document.clone();
                wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::Event)>::new(
                    move |_: web_sys::Event| {
                        if document.pointer_lock_element().is_none() {
                            news.set(PageNews {
                                lock_lost: true,
                                ..news.get()
                            });
                        }
                    },
                )
            };
            let _ = document.add_event_listener_with_callback(
                "pointerlockchange",
                on_lock.as_ref().unchecked_ref(),
            );
            listeners.push(on_lock);
        }
        PageEvents {
            news,
            _listeners: listeners,
        }
    }

    /// What the page said since the last call.
    pub fn take(&self) -> PageNews {
        self.news.take()
    }
}

#[cfg(test)]
#[path = "web_input_tests.rs"]
mod tests;
