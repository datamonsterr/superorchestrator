//! `GET /api/v1/stream`: server-sent events with a replay buffer (design 2.3).
//!
//! The [`Hub`] numbers every message with one monotonic id (the SSE `id`), keeps the last
//! [`BUFFER`] of them and broadcasts them. A client that reconnects with `Last-Event-ID`
//! gets the messages after that id, or a `resync` when the id is too old or from before a
//! restart.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Sse;
use axum::response::sse::{Event as SseEvent, KeepAlive};
use chrono::Utc;
use futures_util::stream::{self, Stream, StreamExt};
use serde::{Deserialize, Serialize};
use so_core::proto::AccountView;
use so_core::{EventType, new_id};
use so_supervisor::SupervisorState;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

use crate::extract::ApiQuery;
use crate::state::AppState;
use crate::types::{RunViewPublic, StreamHello};

/// Messages kept for replay.
pub const BUFFER: usize = 5000;
/// Comment line interval that keeps proxies and the browser connected.
pub const KEEPALIVE: Duration = Duration::from_secs(15);

#[derive(Debug)]
pub struct Msg {
    pub id: u64,
    pub name: &'static str,
    pub data: String,
    /// Session the message belongs to, for the `?session=` filter.
    pub session: Option<String>,
}

struct Inner {
    seq: u64,
    buf: VecDeque<Arc<Msg>>,
}

pub struct Hub {
    pub boot_id: String,
    inner: Mutex<Inner>,
    tx: broadcast::Sender<Arc<Msg>>,
}

impl Hub {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            boot_id: new_id(),
            inner: Mutex::new(Inner {
                seq: 0,
                buf: VecDeque::with_capacity(BUFFER),
            }),
            tx: broadcast::channel(BUFFER).0,
        })
    }

    /// Numbers, buffers and broadcasts one message.
    pub fn publish(&self, name: &'static str, data: &impl Serialize, session: Option<String>) {
        let Ok(data) = serde_json::to_string(data) else {
            return;
        };
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.seq += 1;
        let msg = Arc::new(Msg {
            id: g.seq,
            name,
            data,
            session,
        });
        if g.buf.len() == BUFFER {
            g.buf.pop_front();
        }
        g.buf.push_back(msg.clone());
        // Sent under the lock, so a subscriber that snapshots under it sees every message
        // exactly once (in the snapshot or on its receiver).
        let _ = self.tx.send(msg);
    }

    pub fn seq(&self) -> u64 {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).seq
    }

    /// Reads a `Last-Event-ID` (`<boot_id>:<seq>`). `Some(None)`: no header. `None`: the
    /// header is unusable (another boot, or not parsable), so the client must resync.
    pub fn parse_last_id(&self, raw: Option<&str>) -> Option<Option<u64>> {
        let Some(raw) = raw else {
            return Some(None);
        };
        let (boot, seq) = raw.trim().rsplit_once(':')?;
        (boot == self.boot_id).then_some(())?;
        Some(Some(seq.parse().ok()?))
    }

    /// Like [`Hub::attach`] for a client whose id cannot be honoured.
    fn attach_resync(&self) -> (Replay, broadcast::Receiver<Arc<Msg>>) {
        let _g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        (Replay::Resync, self.tx.subscribe())
    }

    /// Replay for a reconnecting client plus a receiver for everything after it.
    fn attach(&self, last_id: Option<u64>) -> (Replay, broadcast::Receiver<Arc<Msg>>) {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let rx = self.tx.subscribe();
        let replay = match last_id {
            None => Replay::Nothing,
            Some(id) if id > g.seq => Replay::Resync,
            Some(id) => {
                let oldest = g.buf.front().map_or(g.seq + 1, |m| m.id);
                if id + 1 < oldest {
                    Replay::Resync
                } else {
                    Replay::Messages(g.buf.iter().filter(|m| m.id > id).cloned().collect())
                }
            }
        };
        (replay, rx)
    }

    /// Feeds the hub from the run manager's event bus and derives the `run`, `session` and
    /// `headroom` messages. Runs until the supervisor's bus closes.
    pub fn spawn_pump(self: &Arc<Self>, sup: Arc<SupervisorState>) {
        let hub = self.clone();
        let mut workflows = sup.runs.subscribe_workflows();
        let workflow_hub = self.clone();
        let sup_for_lag = sup.clone();
        tokio::spawn(async move {
            loop {
                match workflows.recv().await {
                    Ok(update) => {
                        let mut payload = serde_json::to_value(&update).unwrap_or_default();
                        so_core::redact::redact_json(&mut payload);
                        workflow_hub.publish("workflow", &payload, update.session_id.clone());
                    }
                    // Missed some: say where every running workflow stands now, so a client
                    // does not keep a stale state.
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let Ok(runs) = sup_for_lag.runs.list_workflow_runs(None, 200).await else {
                            continue;
                        };
                        for r in runs.into_iter().filter(|r| r.state == "running") {
                            let data = so_core::proto::WorkflowEventData {
                                id: r.id,
                                name: r.name,
                                state: r.state,
                                step: None,
                                session_id: r.session_id.clone(),
                                ts: chrono::Utc::now(),
                            };
                            workflow_hub.publish("workflow", &data, r.session_id);
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        let mut sub = sup.runs.subscribe_all();
        tokio::spawn(async move {
            while let Some(d) = sub.recv().await {
                let session = d.event.session_id.clone();
                let mut payload = d.to_json();
                so_core::redact::redact_json(&mut payload);
                hub.publish("event", &payload, Some(session.clone()));
                derive(&hub, &sup, &d.event).await;
            }
        });
    }
}

enum Replay {
    Nothing,
    Resync,
    Messages(Vec<Arc<Msg>>),
}

/// The state messages that follow an event.
async fn derive(hub: &Hub, sup: &Arc<SupervisorState>, ev: &so_core::Event) {
    use EventType::*;
    let Some(run_id) = ev.run_id.clone() else {
        return;
    };
    if matches!(
        ev.event_type,
        RunQueued | RunStarted | StateChanged | RunFinished | Usage | Escalated
    ) && let Ok(row) = sup.runs.resolve_run(&run_id).await
        && let Ok(view) = sup.runs.run_view(row).await
    {
        let v = RunViewPublic::from(view);
        if let Ok(mut json) = serde_json::to_value(&v) {
            if let Some(o) = json.as_object_mut() {
                o.remove("recent_events");
            }
            so_core::redact::redact_json(&mut json);
            hub.publish("run", &json, Some(ev.session_id.clone()));
        }
    }
    if matches!(ev.event_type, RunQueued | RunFinished | Escalated)
        && let Ok(s) = crate::api::sessions::summary_of(sup, &ev.session_id).await
    {
        hub.publish("session", &s, Some(ev.session_id.clone()));
    }
    if ev.event_type == Usage
        && let Ok(row) = sup.runs.resolve_run(&run_id).await
        && let Some(account) = row.account
        && let Ok(rows) = sup.db(|s| s.list_headroom(10_000)).await
    {
        for h in rows.into_iter().filter(|h| h.account == account) {
            hub.publish("headroom", &h, None);
        }
    }
}

fn sse(hub: &Hub, m: &Msg) -> SseEvent {
    SseEvent::default()
        .event(m.name)
        .id(format!("{}:{}", hub.boot_id, m.id))
        .data(&m.data)
}

fn resync_event(hub: &Hub) -> SseEvent {
    SseEvent::default()
        .event("resync")
        .id(format!("{}:{}", hub.boot_id, hub.seq()))
        .data("{}")
}

#[derive(Debug, Deserialize)]
pub struct StreamQuery {
    session: Option<String>,
}

pub async fn get_stream(
    State(st): State<AppState>,
    ApiQuery(q): ApiQuery<StreamQuery>,
    headers: HeaderMap,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let hub = st.hub.clone();
    let raw = headers
        .get("last-event-id")
        .map(|v| v.to_str().unwrap_or(""));
    let parsed = hub.parse_last_id(raw);
    let last_id = parsed.flatten();
    let (replay, rx) = match parsed {
        Some(last) => hub.attach(last),
        None => hub.attach_resync(),
    };
    let hello = StreamHello {
        seq: hub.seq(),
        server_time: Utc::now(),
        boot_id: hub.boot_id.clone(),
    };
    let mut first = vec![
        SseEvent::default()
            .event("hello")
            .data(serde_json::to_string(&hello).unwrap_or_default()),
    ];
    let wanted = {
        let session = q.session.clone();
        move |m: &Msg| session.is_none() || m.session.is_none() || m.session == session
    };
    let mut floor = last_id.unwrap_or(0);
    match replay {
        Replay::Nothing => floor = hello.seq,
        Replay::Resync => {
            first.push(resync_event(&hub));
            floor = hello.seq;
        }
        Replay::Messages(ms) => {
            for m in ms.iter().filter(|m| wanted(m)) {
                first.push(sse(&hub, m));
            }
            floor = ms.last().map_or(floor, |m| m.id.max(floor));
        }
    }
    let live_hub = hub.clone();
    let sse_hub = hub.clone();
    let live = BroadcastStream::new(rx).filter_map(move |r| {
        let out = match r {
            Ok(m) if m.id > floor && wanted(&m) => Some(sse(&sse_hub, &m)),
            Ok(_) => None,
            Err(_) => Some(resync_event(&live_hub)),
        };
        async move { out }
    });
    // Held by the stream, so the supervisor does not idle-exit while a client is connected.
    let guard = st.sup.activity_guard();
    let mut stop = st.stop.subscribe();
    let ended = async move {
        let _ = stop.wait_for(|v| *v).await;
    };
    let body = stream::iter(first)
        .chain(live)
        .take_until(ended)
        .map(move |e| {
            let _keep = &guard;
            Ok(e)
        });
    Sse::new(body).keep_alive(KeepAlive::new().interval(KEEPALIVE).text("keepalive"))
}

/// Publishes an `account` message (after an API change to an account).
pub fn publish_account(hub: &Hub, view: &AccountView) {
    hub.publish("account", view, None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_event_id_carries_the_boot() {
        let hub = Hub::new();
        let id = |seq: u64| format!("{}:{seq}", hub.boot_id);
        assert_eq!(hub.parse_last_id(Some(&id(7))), Some(Some(7)));
        assert_eq!(hub.parse_last_id(None), Some(None));
        for bad in ["7", "x:7", &format!("{}:z", hub.boot_id), ""] {
            assert_eq!(hub.parse_last_id(Some(bad)), None, "{bad}");
        }
    }

    fn replay_len(hub: &Hub, last: Option<u64>) -> Option<usize> {
        match hub.attach(last).0 {
            Replay::Messages(m) => Some(m.len()),
            _ => None,
        }
    }

    #[test]
    fn buffer_keeps_the_last_5000_and_old_ids_need_a_resync() {
        let hub = Hub::new();
        for i in 0..(BUFFER + 10) {
            hub.publish("event", &i, None);
        }
        let seq = hub.seq();
        assert_eq!(seq as usize, BUFFER + 10);
        // Oldest buffered id is 11: id 10 is contiguous, id 9 has a gap.
        assert_eq!(replay_len(&hub, Some(10)), Some(BUFFER));
        assert!(matches!(hub.attach(Some(9)).0, Replay::Resync));
        assert_eq!(replay_len(&hub, Some(seq)), Some(0));
        assert!(matches!(hub.attach(Some(seq + 1)).0, Replay::Resync));
        assert!(matches!(hub.attach(None).0, Replay::Nothing));
    }
}
