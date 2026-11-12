//! Predict ids: how a client shows a thing it asked a server for at once, and keeps it when the server's ghost of it comes.
//!
//! The client makes its own copy of the thing, takes an id for it from [`PredictIds::issue`] (1 to 255, one not in use) and
//! sends the id with its request (a game's own server command). The server's ghost carries the id back for that client alone;
//! the client's handler [`PredictIds::adopt`]s it and keeps its copy instead of making another. A copy the server never
//! confirms within [`PREDICT_TIMEOUT`] seconds is given back by [`PredictIds::expire`], for the client to fade out over
//! [`PREDICT_FADE`] seconds. At most [`MAX_WAITING`] wait at once.
//!
//! Ropes and props use the same scheme through this type.

/// The most predictions a client waits on at once (and so the most predict ids in use).
pub const MAX_WAITING: usize = 16;

/// How long a prediction waits for the server's ghost (seconds) before it fades out.
pub const PREDICT_TIMEOUT: f32 = 2.0;

/// The seconds a prediction that was never confirmed fades out over.
pub const PREDICT_FADE: f32 = 0.25;

/// The predict id a script gave, as the wire carries it: 1 to 255 as given, 0 (no prediction) for none, 0 or anything else.
#[must_use]
pub fn wire_id(id: Option<i32>) -> u8 {
    id.and_then(|id| u8::try_from(id).ok()).unwrap_or(0)
}

/// One prediction waiting for its ghost.
#[derive(Debug, Clone, PartialEq)]
struct Waiting<T> {
    id: u8,
    item: T,
    age: f32,
}

/// The predictions a client waits on, each with its id ([module docs](self)).
#[derive(Debug, Clone, PartialEq)]
pub struct PredictIds<T> {
    waiting: Vec<Waiting<T>>,
    last: u8,
}

impl<T> Default for PredictIds<T> {
    fn default() -> Self {
        PredictIds {
            waiting: Vec::new(),
            last: 0,
        }
    }
}

impl<T> PredictIds<T> {
    /// How many wait.
    pub fn len(&self) -> usize {
        self.waiting.len()
    }

    /// Whether none wait.
    pub fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }

    /// Whether a waiting item meets `f`.
    pub fn any(&self, f: impl Fn(&T) -> bool) -> bool {
        self.waiting.iter().any(|w| f(&w.item))
    }

    /// The waiting items with their ids, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = (u8, &T)> + '_ {
        self.waiting.iter().map(|w| (w.id, &w.item))
    }

    /// The waiting items with their ids and the seconds each has waited, oldest first, to change.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (u8, f32, &mut T)> + '_ {
        self.waiting.iter_mut().map(|w| (w.id, w.age, &mut w.item))
    }

    /// Waits on `item` under a new id (1 to 255, the next one after the last given that is not in use). None when
    /// [`MAX_WAITING`] wait already.
    pub fn issue(&mut self, item: T) -> Option<u8> {
        if self.waiting.len() >= MAX_WAITING {
            return None;
        }
        let id = (1..=255u8)
            .map(|k| self.last.wrapping_add(k))
            .filter(|&id| id != 0)
            .find(|&id| self.waiting.iter().all(|w| w.id != id))?;
        self.last = id;
        self.waiting.push(Waiting { id, item, age: 0.0 });
        Some(id)
    }

    /// The item waiting under `id`, taken: the server's ghost is it. None for 0 or an id nothing waits under.
    pub fn adopt(&mut self, id: u8) -> Option<T> {
        if id == 0 {
            return None;
        }
        let at = self.waiting.iter().position(|w| w.id == id)?;
        Some(self.waiting.remove(at).item)
    }

    /// Ages every waiting item by `seconds` (a value that is not finite or below 0 counts as 0), drops those `keep` says are
    /// gone, and gives back the ones that waited [`PREDICT_TIMEOUT`] seconds, for the caller to fade out.
    pub fn expire(&mut self, seconds: f32, keep: impl Fn(&T) -> bool) -> Vec<T> {
        let seconds = if seconds.is_finite() {
            seconds.max(0.0)
        } else {
            0.0
        };
        let mut out = Vec::new();
        let mut kept = Vec::with_capacity(self.waiting.len());
        for mut w in self.waiting.drain(..) {
            if !keep(&w.item) {
                continue;
            }
            w.age += seconds;
            if w.age >= PREDICT_TIMEOUT {
                out.push(w.item);
            } else {
                kept.push(w);
            }
        }
        self.waiting = kept;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_predict_id_is_held_to_one_to_255() {
        assert_eq!(wire_id(None), 0);
        for (given, wire) in [
            (-1, 0),
            (0, 0),
            (1, 1),
            (255, 255),
            (256, 0),
            (i32::MIN, 0),
            (i32::MAX, 0),
        ] {
            assert_eq!(wire_id(Some(given)), wire, "{given}");
        }
    }

    #[test]
    fn ids_run_from_one_skip_zero_and_those_in_use_and_stop_at_the_cap() {
        let mut ids = PredictIds::default();
        let first: Vec<u8> = (0..MAX_WAITING).filter_map(|k| ids.issue(k)).collect();
        assert_eq!(first, (1..=MAX_WAITING as u8).collect::<Vec<_>>());
        assert_eq!(ids.issue(99), None, "the cap holds");
        assert_eq!(ids.adopt(3), Some(2));
        assert_eq!(ids.adopt(3), None, "adopted once");
        assert_eq!(ids.adopt(0), None);
        assert_eq!(ids.issue(7), Some(17), "the next after the last given");
        // Round the 255 and past 0, never an id in use.
        let mut ids = PredictIds {
            waiting: vec![Waiting {
                id: 1,
                item: 0,
                age: 0.0,
            }],
            last: 254,
        };
        assert_eq!(ids.issue(1), Some(255));
        assert_eq!(ids.issue(2), Some(2));
    }

    #[test]
    fn a_prediction_never_confirmed_expires_and_a_gone_one_is_dropped() {
        let mut ids = PredictIds::default();
        ids.issue("kept");
        ids.issue("gone");
        assert!(ids.expire(1.0, |_| true).is_empty());
        assert_eq!(ids.expire(f32::NAN, |&i| i != "gone"), Vec::<&str>::new());
        assert_eq!(ids.len(), 1);
        assert_eq!(ids.expire(1.0, |_| true), ["kept"]);
        assert!(ids.is_empty());
    }
}
