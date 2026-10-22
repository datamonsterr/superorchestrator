//! Messages between the orchestrator and runs, progress, questions and waiting (SPEC 9).

use chrono::Utc;
use serde_json::json;
use so_core::proto::{
    AckResult, AskParentParams, AskParentResult, INVALID_PARAMS, INVALID_REQUEST, MessageView,
    ReadMessagesResult, ReportProgressParams, SendMessageParams, UNAVAILABLE,
};
use so_core::{EventType, MessageKind, RunState, new_id};
use so_store::{MessageRow, RunRow};

use super::lifecycle::excerpt;
use super::{LockKind, R, RunManager, lock, rpc};
use crate::dispatch::ConnCtx;

const MESSAGES_LIMIT: usize = 1000;

/// Marks a run `waiting` while a blocking call is in progress; ending it (or dropping the
/// guard, for a handler that is aborted) moves the run back to `running`.
pub(crate) struct WaitGuard {
    mgr: RunManager,
    run_id: String,
    ask: bool,
    armed: bool,
}

impl WaitGuard {
    pub(crate) async fn end(mut self) {
        self.armed = false;
        self.mgr.end_wait(&self.run_id, self.ask).await;
    }
}

impl Drop for WaitGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            let (mgr, id, ask) = (self.mgr.clone(), self.run_id.clone(), self.ask);
            rt.spawn(async move { mgr.end_wait(&id, ask).await });
        }
    }
}

pub(crate) fn message_view(m: MessageRow) -> MessageView {
    MessageView {
        id: m.id,
        from_run: m.from_run,
        to_run: m.to_run,
        kind: m.kind,
        body: m.body,
        created_at: m.created_at,
    }
}

impl RunManager {
    /// Moves a run between states and records `state_changed`. `false` when the run was
    /// not in `from` (or the move is not allowed).
    pub(crate) async fn move_state(
        &self,
        run_id: &str,
        from: RunState,
        to: RunState,
        reason: &str,
    ) -> bool {
        let id = run_id.to_string();
        let moved = self
            .db(move |s| {
                s.update_run_state(&id, from, to)?;
                s.get_run(&id)
            })
            .await;
        let Ok(Some(row)) = moved else { return false };
        self.emit(
            &row.session_id,
            Some(run_id),
            so_core::EventType::StateChanged,
            json!({"from": from, "to": to, "reason": reason}),
        )
        .await;
        true
    }

    /// True while a blocking call of the run is in progress.
    pub(crate) fn is_waiting(&self, run_id: &str) -> bool {
        lock(&self.inner.runs)
            .get(run_id)
            .is_some_and(|rt| rt.waits > 0)
    }

    /// True while the run is blocked in `ask_parent`, the only call an answer unblocks.
    pub(crate) fn is_asking(&self, run_id: &str) -> bool {
        lock(&self.inner.runs)
            .get(run_id)
            .is_some_and(|rt| rt.asks > 0)
    }

    /// Starts a blocking call of `run_id`: the run is `waiting` until the guard ends.
    /// `ask` marks an `ask_parent`. A run the manager does not track (already wrapped up)
    /// gets an inert guard and no registry entry.
    pub(crate) async fn begin_wait(&self, run_id: &str, ask: bool) -> WaitGuard {
        let first = {
            let mut runs = lock(&self.inner.runs);
            runs.get_mut(run_id).map(|rt| {
                rt.waits += 1;
                if ask {
                    rt.asks += 1;
                }
                rt.waits == 1
            })
        };
        // Armed before any await: if this future is dropped from here on, the guard
        // undoes the count.
        let guard = WaitGuard {
            mgr: self.clone(),
            run_id: run_id.to_string(),
            ask,
            armed: first.is_some(),
        };
        if first == Some(true) {
            self.move_state(
                run_id,
                RunState::Running,
                RunState::Waiting,
                "blocking call",
            )
            .await;
        }
        guard
    }

    async fn end_wait(&self, run_id: &str, ask: bool) {
        let last = {
            let mut runs = lock(&self.inner.runs);
            match runs.get_mut(run_id) {
                Some(rt) => {
                    rt.waits = rt.waits.saturating_sub(1);
                    if ask {
                        rt.asks = rt.asks.saturating_sub(1);
                    }
                    rt.waits == 0
                }
                None => false,
            }
        };
        if last {
            self.move_state(
                run_id,
                RunState::Waiting,
                RunState::Running,
                "call returned",
            )
            .await;
        }
    }

    /// Stores a message and wakes anything blocked on messages.
    pub(crate) async fn store_message(
        &self,
        session_id: &str,
        from: Option<&str>,
        to: Option<&str>,
        kind: MessageKind,
        body: &str,
        read: bool,
    ) -> R<MessageRow> {
        let now = Utc::now();
        let m = MessageRow {
            id: new_id(),
            session_id: session_id.to_string(),
            from_run: from.map(String::from),
            to_run: to.map(String::from),
            kind,
            body: body.to_string(),
            created_at: now,
            read_at: read.then_some(now),
        };
        let stored = m.clone();
        self.db(move |s| s.insert_message(&stored)).await?;
        self.inner.msg_tick.send_modify(|n| *n += 1);
        Ok(m)
    }

    async fn mark_read(&self, ids: Vec<String>) {
        if ids.is_empty() {
            return;
        }
        let _ = self
            .db(move |s| s.mark_messages_read(&ids, Utc::now()))
            .await;
    }

    fn same_session(ctx: &ConnCtx, row: &RunRow) -> R<()> {
        if ctx.session_id.as_deref() == Some(row.session_id.as_str()) {
            Ok(())
        } else {
            Err(rpc(INVALID_PARAMS, "that run belongs to another session"))
        }
    }

    // ------------------------------------------------------------ send_message

    /// `send_message`: stores the message, then delivers it. A live run gets it as input,
    /// a queued run gets it when it starts, and a finished run (or one at the review gate)
    /// resumes its native session with it.
    pub async fn send_message(&self, ctx: &ConnCtx, p: SendMessageParams) -> R<AckResult> {
        if p.text.trim().is_empty() {
            return Err(rpc(INVALID_PARAMS, "text must not be empty"));
        }
        let row = self.resolve_run(&p.run).await?;
        Self::same_session(ctx, &row)?;
        // A message to a run at the review gate acts like a reject: it takes the run's
        // review turn (always before the gate lock), so it waits for an accept or reject
        // in flight and then sees the state that left behind.
        let _review_turn = if row.state == RunState::PendingReview {
            Some(self.lock_run(LockKind::Review, &row.id).await)
        } else {
            None
        };
        let row = if _review_turn.is_some() {
            self.resolve_run(&row.id).await?
        } else {
            row
        };
        if row.state == RunState::Cancelled {
            return Err(rpc(INVALID_REQUEST, "the run was cancelled"));
        }
        let kind = p.kind.unwrap_or(MessageKind::Message);
        let msg = self
            .store_message(
                &row.session_id,
                ctx.run_id.as_deref(),
                Some(&row.id),
                kind,
                &p.text,
                false,
            )
            .await?;
        self.emit(
            &row.session_id,
            Some(&row.id),
            EventType::MessageSent,
            json!({"to": row.id, "from": ctx.run_id, "kind": kind, "actor": ctx.actor()}),
        )
        .await;
        if kind == MessageKind::Answer {
            // The question is answered: it no longer wakes `wait_agent`.
            let (sid, to, from) = (row.session_id.clone(), ctx.run_id.clone(), row.id.clone());
            let _ = self
                .db(move |s| {
                    let ids: Vec<String> = s
                        .unread_messages(&sid, to.as_deref(), MESSAGES_LIMIT)?
                        .into_iter()
                        .filter(|m| {
                            m.kind == MessageKind::Question
                                && m.from_run.as_deref() == Some(from.as_str())
                        })
                        .map(|m| m.id)
                        .collect();
                    s.mark_messages_read(&ids, Utc::now())
                })
                .await;
            if self.is_asking(&row.id) {
                // `ask_parent` returns it to the child.
                return Ok(AckResult { ok: true });
            }
        }
        self.deliver(&row, &msg, ctx.actor()).await?;
        Ok(AckResult { ok: true })
    }

    async fn deliver(&self, row: &RunRow, msg: &MessageRow, actor: &'static str) -> R<()> {
        match row.state {
            RunState::Queued | RunState::Starting => Ok(()),
            RunState::Succeeded | RunState::Failed | RunState::PendingReview => {
                self.resume_finished(row, &msg.body, "message", Some(msg.id.clone()), actor)
                    .await?;
                Ok(())
            }
            _ => {
                let control = lock(&self.inner.runs)
                    .get(&row.id)
                    .and_then(|rt| rt.live.as_ref().map(|l| l.control.clone()));
                // Without a live process the message stays unread; `go_live` hands it over.
                if let Some(control) = control {
                    self.deliver_claimed(msg, |text| {
                        let control = control.clone();
                        async move { control.send(&text).await.map_err(|e| e.to_string()) }
                    })
                    .await?;
                    // The run now knows this; a failover must tell the next account too.
                    let (id, body) = (row.id.clone(), msg.body.clone());
                    let _ = self.db(move |s| s.append_run_turn_prompt(&id, &body)).await;
                }
                Ok(())
            }
        }
    }

    /// Delivers one message to a live run. The message is claimed first, so a flush that
    /// races this call cannot send it too; if the send fails it goes back to unread.
    pub(crate) async fn deliver_claimed<F, Fut>(&self, msg: &MessageRow, send: F) -> R<()>
    where
        F: FnOnce(String) -> Fut,
        Fut: std::future::Future<Output = Result<(), String>>,
    {
        let id = msg.id.clone();
        let won = self
            .db(move |s| s.claim_messages(&[id], Utc::now()))
            .await?;
        if won.is_empty() {
            // Someone else (a flush) already took it.
            return Ok(());
        }
        if let Err(e) = send(msg.body.clone()).await {
            self.unclaim(won).await;
            return Err(rpc(
                UNAVAILABLE,
                format!("could not deliver the message: {e}"),
            ));
        }
        Ok(())
    }

    /// Puts claimed messages back to unread and wakes whatever waits for messages.
    pub(crate) async fn unclaim(&self, ids: Vec<String>) {
        let _ = self.db(move |s| s.mark_messages_unread(&ids)).await;
        self.inner.msg_tick.send_modify(|n| *n += 1);
    }

    /// Messages sent to or from a run, oldest first (not marked read).
    pub async fn list_messages(&self, run_id: &str) -> R<Vec<MessageView>> {
        let id = run_id.to_string();
        let rows = self
            .db(move |s| s.list_messages_by_run(&id, MESSAGES_LIMIT))
            .await?;
        Ok(rows.into_iter().map(message_view).collect())
    }

    // ----------------------------------------------------------- read_messages

    /// `read_messages`: the caller's unread messages (a run's, or the orchestrator's in its
    /// session). They are marked read.
    pub async fn read_messages(&self, ctx: &ConnCtx) -> R<ReadMessagesResult> {
        let session = ctx
            .session_id
            .clone()
            .ok_or_else(|| rpc(INVALID_REQUEST, "hello required"))?;
        let to = ctx.run_id.clone();
        let rows = self
            .db(move |s| {
                let rows = s.unread_messages(&session, to.as_deref(), MESSAGES_LIMIT)?;
                let ids: Vec<String> = rows.iter().map(|m| m.id.clone()).collect();
                s.mark_messages_read(&ids, Utc::now())?;
                Ok(rows)
            })
            .await?;
        Ok(ReadMessagesResult {
            messages: rows.into_iter().map(message_view).collect(),
        })
    }

    // --------------------------------------------------------- child-side tools

    fn caller_run(ctx: &ConnCtx, what: &str) -> R<String> {
        ctx.run_id
            .clone()
            .ok_or_else(|| rpc(INVALID_REQUEST, format!("{what} is for subagent runs")))
    }

    /// `report_progress`: records a `progress` event and tells the parent.
    pub async fn report_progress(&self, ctx: &ConnCtx, p: ReportProgressParams) -> R<AckResult> {
        let run_id = Self::caller_run(ctx, "report_progress")?;
        let row = self.resolve_run(&run_id).await?;
        let body = match &p.note {
            Some(n) => format!("{}: {} ({n})", p.step, p.status),
            None => format!("{}: {}", p.step, p.status),
        };
        self.store_message(
            &row.session_id,
            Some(&row.id),
            row.parent_run_id.as_deref(),
            MessageKind::Progress,
            &body,
            false,
        )
        .await?;
        self.emit(
            &row.session_id,
            Some(&row.id),
            EventType::Progress,
            json!({"step": p.step, "status": p.status, "note": p.note}),
        )
        .await;
        Ok(AckResult { ok: true })
    }

    /// `ask_parent`: the run is `waiting` until the parent answers
    /// (`send_message` kind `answer`) or `runs.ask_timeout` passes.
    pub async fn ask_parent(&self, ctx: &ConnCtx, p: AskParentParams) -> R<AskParentResult> {
        let run_id = Self::caller_run(ctx, "ask_parent")?;
        let row = self.resolve_run(&run_id).await?;
        let timeout = self.config().runs.ask_timeout.as_duration();
        let mut tick = self.inner.msg_tick.subscribe();
        // An answer to an earlier question that timed out must not answer this one.
        self.discard_stale_answers(&row).await;

        let guard = self.begin_wait(&row.id, true).await;
        let body = match &p.options {
            Some(o) if !o.is_empty() => format!("{}\nOptions: {}", p.question, o.join(" | ")),
            _ => p.question.clone(),
        };
        let question_msg = self
            .store_message(
                &row.session_id,
                Some(&row.id),
                row.parent_run_id.as_deref(),
                MessageKind::Question,
                &body,
                false,
            )
            .await;
        let question_id = match question_msg {
            Ok(m) => m.id,
            Err(e) => {
                guard.end().await;
                return Err(e);
            }
        };
        self.emit(
            &row.session_id,
            Some(&row.id),
            EventType::QuestionAsked,
            json!({"question": p.question, "options": p.options, "to": row.parent_run_id}),
        )
        .await;

        let started = tokio::time::Instant::now();
        let deadline = started + timeout;
        let notify_at = started + self.question_notify_after();
        let mut notified = false;
        let name = row.name.clone().unwrap_or_else(|| row.id.clone());
        let outcome = loop {
            match self.take_answer(&row).await {
                Ok(Some(answer)) => break Ok(Some(answer)),
                Ok(None) => {}
                Err(e) => break Err(e),
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                break Ok(None);
            }
            if !notified && now >= notify_at {
                notified = true;
                self.notify(
                    "question_waiting_10m",
                    &row.id,
                    &name,
                    "a question has waited 10 minutes for an answer",
                );
            }
            let wake = if notified {
                deadline
            } else {
                deadline.min(notify_at)
            };
            tokio::select! {
                _ = tick.changed() => {}
                () = tokio::time::sleep_until(wake) => {}
                () = ctx.closed() => break Err(rpc(UNAVAILABLE, "client disconnected")),
            }
        };
        guard.end().await;
        // Answered or not, the question no longer waits for the parent.
        self.mark_read(vec![question_id]).await;
        let answered = match outcome? {
            Some(answer) => AskParentResult {
                answer,
                timed_out: false,
            },
            None => AskParentResult {
                answer: "timeout".into(),
                timed_out: true,
            },
        };
        self.emit(
            &row.session_id,
            Some(&row.id),
            EventType::QuestionAnswered,
            json!({"answer": excerpt(&answered.answer, 280), "timed_out": answered.timed_out}),
        )
        .await;
        Ok(answered)
    }

    async fn discard_stale_answers(&self, row: &RunRow) {
        let (sid, id) = (row.session_id.clone(), row.id.clone());
        let _ = self
            .db(move |s| {
                let ids: Vec<String> = s
                    .unread_messages_of_kinds(
                        &sid,
                        Some(&id),
                        &[MessageKind::Answer],
                        MESSAGES_LIMIT,
                    )?
                    .into_iter()
                    .map(|m| m.id)
                    .collect();
                s.claim_messages(&ids, Utc::now())
            })
            .await;
    }

    /// The oldest unread answer for the run, claimed (so a flush cannot also take it).
    async fn take_answer(&self, row: &RunRow) -> R<Option<String>> {
        let (sid, id) = (row.session_id.clone(), row.id.clone());
        self.db(move |s| {
            for m in s.unread_messages_of_kinds(&sid, Some(&id), &[MessageKind::Answer], 100)? {
                if !s
                    .claim_messages(std::slice::from_ref(&m.id), Utc::now())?
                    .is_empty()
                {
                    return Ok(Some(m.body));
                }
            }
            Ok(None)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use so_core::RunState;

    use super::super::testutil::{insert_run, manager};
    use super::*;

    #[tokio::test]
    async fn waiting_on_an_untracked_run_neither_creates_an_entry_nor_moves_it() {
        let (m, _dir) = manager();
        let row = insert_run(&m, RunState::Succeeded);
        lock(&m.inner.runs).remove(&row.id);
        let guard = m.begin_wait(&row.id, true).await;
        assert!(!m.is_waiting(&row.id) && !lock(&m.inner.runs).contains_key(&row.id));
        guard.end().await;
        assert!(!lock(&m.inner.runs).contains_key(&row.id));
        assert_eq!(
            m.store().get_run(&row.id).unwrap().unwrap().state,
            RunState::Succeeded
        );
    }

    #[tokio::test]
    async fn a_dropped_wait_guard_puts_the_run_back_to_running() {
        let (m, _dir) = manager();
        let row = insert_run(&m, RunState::Running);
        let guard = m.begin_wait(&row.id, true).await;
        assert!(m.is_asking(&row.id));
        assert_eq!(
            m.store().get_run(&row.id).unwrap().unwrap().state,
            RunState::Waiting
        );
        drop(guard);
        for _ in 0..100 {
            if m.store().get_run(&row.id).unwrap().unwrap().state == RunState::Running {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(
            m.store().get_run(&row.id).unwrap().unwrap().state,
            RunState::Running
        );
        assert!(!m.is_waiting(&row.id));
    }

    #[tokio::test]
    async fn deliver_racing_flush_sends_a_message_once() {
        let (m, _dir) = manager();
        let row = insert_run(&m, RunState::Running);
        for round in 0..30 {
            let msg = m
                .store_message(
                    &row.session_id,
                    None,
                    Some(&row.id),
                    MessageKind::Message,
                    &format!("note {round}"),
                    false,
                )
                .await
                .unwrap();
            let sent = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
            let (s1, s2) = (sent.clone(), sent.clone());
            let (m1, m2, row2, msg2) = (m.clone(), m.clone(), row.clone(), msg.clone());
            let deliver = tokio::spawn(async move {
                m1.deliver_claimed(&msg2, |t| async move {
                    s1.lock().unwrap().push(t);
                    Ok(())
                })
                .await
                .unwrap();
            });
            let flush = tokio::spawn(async move {
                let batch = m2.claim_input(&row2).await;
                m2.hand_over(batch, |t| {
                    let s2 = s2.clone();
                    async move {
                        s2.lock().unwrap().push(t);
                        Ok(())
                    }
                })
                .await;
            });
            deliver.await.unwrap();
            flush.await.unwrap();
            assert_eq!(*sent.lock().unwrap(), [format!("note {round}")]);
        }
    }

    #[tokio::test]
    async fn a_failed_live_send_puts_the_message_back() {
        let (m, _dir) = manager();
        let row = insert_run(&m, RunState::Running);
        let msg = m
            .store_message(
                &row.session_id,
                None,
                Some(&row.id),
                MessageKind::Message,
                "hello",
                false,
            )
            .await
            .unwrap();
        let r = m
            .deliver_claimed(&msg, |_| async { Err("closed".to_string()) })
            .await;
        assert!(r.is_err());
        let again = m.claim_input(&row).await;
        assert_eq!(again.len(), 1);
    }
}
