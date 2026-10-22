//! Storing events and fanning them out to subscribers.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use chrono::Utc;
use serde_json::Value;
use so_core::{Event, EventType, new_id};
use tokio::sync::mpsc;

use super::{RunManager, lock};

/// An event as delivered to a subscriber: the stored event plus the per-session sequence
/// number, which orders events within a session for UIs.
#[derive(Debug, Clone)]
pub struct Delivered {
    pub seq: u64,
    pub event: Event,
}

impl Delivered {
    /// The `event` notification payload: the stored event with `seq` added.
    pub fn to_json(&self) -> Value {
        let mut v = serde_json::to_value(&self.event).unwrap_or(Value::Null);
        if let Some(o) = v.as_object_mut() {
            o.insert("seq".into(), self.seq.into());
        }
        v
    }
}

struct Sub {
    id: u64,
    tx: mpsc::UnboundedSender<Delivered>,
    /// Only these runs; `None` is the whole session.
    runs: Option<HashSet<String>>,
}

#[derive(Default)]
struct BusState {
    next_id: u64,
    subs: HashMap<String, Vec<Sub>>,
    /// Subscribers to every session (the web UI's stream).
    all: Vec<(u64, mpsc::UnboundedSender<Delivered>)>,
    seqs: HashMap<String, u64>,
}

#[derive(Default)]
pub(super) struct Bus {
    state: Mutex<BusState>,
}

impl Bus {
    pub(super) fn publish(&self, ev: &Event) {
        let mut st = lock(&self.state);
        let seq = {
            let n = st.seqs.entry(ev.session_id.clone()).or_default();
            *n += 1;
            *n
        };
        st.all.retain(|(_, tx)| {
            tx.send(Delivered {
                seq,
                event: ev.clone(),
            })
            .is_ok()
        });
        if let Some(subs) = st.subs.get(&ev.session_id) {
            for s in subs {
                let wanted = s
                    .runs
                    .as_ref()
                    .is_none_or(|r| ev.run_id.as_ref().is_some_and(|id| r.contains(id)));
                if wanted {
                    let _ = s.tx.send(Delivered {
                        seq,
                        event: ev.clone(),
                    });
                }
            }
        }
    }

    pub(super) fn forget_session(&self, id: &str) {
        lock(&self.state).seqs.remove(id);
    }

    fn remove(&self, sub_id: u64, sessions: &[String]) {
        let mut st = lock(&self.state);
        st.all.retain(|(id, _)| *id != sub_id);
        for s in sessions {
            if let Some(v) = st.subs.get_mut(s) {
                v.retain(|x| x.id != sub_id);
                if v.is_empty() {
                    st.subs.remove(s);
                }
            }
        }
    }
}

/// Events of some sessions from the moment of subscribing. Dropping it unsubscribes.
pub struct Subscription {
    rx: mpsc::UnboundedReceiver<Delivered>,
    bus: Arc<Bus>,
    id: u64,
    sessions: Vec<String>,
}

impl Subscription {
    pub async fn recv(&mut self) -> Option<Delivered> {
        self.rx.recv().await
    }

    pub fn try_recv(&mut self) -> Result<Delivered, mpsc::error::TryRecvError> {
        self.rx.try_recv()
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.bus.remove(self.id, &self.sessions);
    }
}

impl RunManager {
    /// Subscribes to the events of `sessions`, optionally only those of `runs`.
    pub fn subscribe(&self, sessions: &[String], runs: Option<&[String]>) -> Subscription {
        let (tx, rx) = mpsc::unbounded_channel();
        let bus = self.inner.bus.clone();
        let mut st = lock(&bus.state);
        st.next_id += 1;
        let id = st.next_id;
        for s in sessions {
            st.subs.entry(s.clone()).or_default().push(Sub {
                id,
                tx: tx.clone(),
                runs: runs.map(|r| r.iter().cloned().collect()),
            });
        }
        drop(st);
        Subscription {
            rx,
            bus,
            id,
            sessions: sessions.to_vec(),
        }
    }

    /// Subscribes to the events of every session from the moment of subscribing.
    pub fn subscribe_all(&self) -> Subscription {
        let (tx, rx) = mpsc::unbounded_channel();
        let bus = self.inner.bus.clone();
        let mut st = lock(&bus.state);
        st.next_id += 1;
        let id = st.next_id;
        st.all.push((id, tx));
        drop(st);
        Subscription {
            rx,
            bus,
            id,
            sessions: Vec::new(),
        }
    }

    /// Stores an event, then delivers it to subscribers.
    pub async fn emit(
        &self,
        session_id: &str,
        run_id: Option<&str>,
        event_type: EventType,
        data: Value,
    ) {
        let ev = Event {
            id: new_id(),
            session_id: session_id.to_string(),
            run_id: run_id.map(String::from),
            ts: Utc::now(),
            event_type,
            data,
        };
        let stored = ev.clone();
        if let Err(e) = self.db(move |s| s.append_event(&stored)).await {
            tracing::warn!(error = %e.message, "could not store event");
            return;
        }
        self.inner.bus.publish(&ev);
    }

    pub(super) fn forget_session_seq(&self, id: &str) {
        self.inner.bus.forget_session(id);
    }
}
