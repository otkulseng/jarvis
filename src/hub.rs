//! Between the model and the browsers. Changes are applied to the model as
//! they arrive, and what they mean is gathered here and sent on a few times a
//! second, so that a burst of a thousand changes reaches a browser as one
//! small batch and never as a thousand.
//!
//! A browser that cannot keep up is let go. It connects again by itself and
//! starts from a fresh picture, so nothing here ever piles up without limit.

use crate::model::{Model, Out};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::Mutex;
use tokio::sync::mpsc;

/// How many batches may wait for one browser before it is let go.
const WAITING_BATCHES: usize = 64;

struct Browser {
    topics: HashSet<String>,
    batches: mpsc::Sender<String>,
}

#[derive(Default)]
struct Inner {
    model: Model,
    browsers: Vec<Browser>,
    /// What has happened since the last batch was sent.
    pending: Vec<Out>,
}

#[derive(Default)]
pub struct Hub {
    inner: Mutex<Inner>,
}

impl Hub {
    /// A change from jarvis.
    pub fn apply(&self, change: &Value) {
        let mut inner = self.inner.lock().expect("the hub's lock");
        for out in inner.model.apply(change) {
            // Of two states of the same thing, only the newer is worth sending.
            match out.key.as_ref().and_then(|key| inner.pending.iter().position(|p| p.key.as_ref() == Some(key))) {
                Some(at) => inner.pending[at] = out,
                None => inner.pending.push(out),
            }
        }
    }

    /// Start counting again from what jarvis says now, and have every
    /// browser start again from that picture.
    pub fn load(&self, pairs: &[Value], waiting: &[Value]) {
        let mut inner = self.inner.lock().expect("the hub's lock");
        inner.model.load(pairs, waiting);
        inner.pending.clear();
        inner.browsers.clear();
    }

    /// Send what is pending to every browser that asked for it.
    pub fn flush(&self) {
        let mut inner = self.inner.lock().expect("the hub's lock");
        if inner.pending.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut inner.pending);
        inner.browsers.retain(|browser| {
            let frames: Vec<&Value> =
                pending.iter().filter(|out| out.topics.iter().any(|t| browser.topics.contains(t))).map(|out| &out.frame).collect();
            frames.is_empty() || browser.batches.try_send(json!(frames).to_string()).is_ok()
        });
    }

    /// A browser connects. Returns the first batch (who is serving it, and
    /// the whole map) and the channel on which everything after it arrives.
    /// The sender is for what must be fetched first (see `threads`).
    pub fn connect(&self, topics: HashSet<String>, hello: Value) -> (String, mpsc::Sender<String>, mpsc::Receiver<String>) {
        let (batches, receiver) = mpsc::channel(WAITING_BATCHES);
        let mut inner = self.inner.lock().expect("the hub's lock");
        let mut first = vec![hello];
        first.extend(inner.model.map());
        inner.browsers.push(Browser { topics, batches: batches.clone() });
        (json!(first).to_string(), batches, receiver)
    }
}
