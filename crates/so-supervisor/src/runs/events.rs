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
    pub(super) fn publish(&self, ev: &Event) -> u64 {
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
        seq
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
        self.emit_delivered(session_id, run_id, event_type, data)
            .await;
    }

    /// Like [`RunManager::emit`], and returns the event as subscribers got it (with its
    /// sequence number), for a caller that also pushes it to a client.
    pub async fn emit_delivered(
        &self,
        session_id: &str,
        run_id: Option<&str>,
        event_type: EventType,
        data: Value,
    ) -> Option<Delivered> {
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
            return None;
        }
        let seq = self.inner.bus.publish(&ev);
        Some(Delivered { seq, event: ev })
    }

    pub(super) fn forget_session_seq(&self, id: &str) {
        self.inner.bus.forget_session(id);
    }
}

#[cfg(test)]
mod tests {
    use super::super::testutil::manager;
    use super::*;

    fn sub_count(m: &RunManager) -> usize {
        lock(&m.inner.bus.state).subs.values().map(Vec::len).sum()
    }

    #[tokio::test]
    async fn dropping_a_subscription_removes_the_subscriber() {
        let (m, _dir) = manager();
        let sessions = ["s1".to_string(), "s2".to_string()];
        let a = m.subscribe(&sessions, None);
        let b = m.subscribe(&sessions[..1], Some(&["r".to_string()]));
        assert_eq!(sub_count(&m), 3);
        drop(a);
        assert_eq!(sub_count(&m), 1);
        drop(b);
        assert_eq!(sub_count(&m), 0);
    }

    #[tokio::test]
    async fn run_filter_is_applied_when_publishing() {
        let (m, _dir) = manager();
        let mut sub = m.subscribe(&["s".to_string()], Some(&["r1".to_string()]));
        for run in ["r2", "r1"] {
            m.emit("s", Some(run), EventType::Progress, serde_json::json!({}))
                .await;
        }
        let got = sub.recv().await.unwrap();
        assert_eq!(got.event.run_id.as_deref(), Some("r1"));
        assert_eq!(got.seq, 2);
        assert!(sub.try_recv().is_err());
    }
}
