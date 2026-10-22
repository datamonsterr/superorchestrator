//! Messages between the orchestrator and runs, progress, questions and waiting (SPEC 9).

use std::time::Duration;

use chrono::Utc;
use serde_json::json;
use so_core::proto::{
    AckResult, AskParentParams, AskParentResult, INVALID_PARAMS, INVALID_REQUEST, MessageView,
    ReadMessagesResult, ReportProgressParams, SendMessageParams, UNAVAILABLE,
};
use so_core::{EventType, MessageKind, RunState, new_id};
use so_store::{MessageRow, RunRow};

use super::lifecycle::excerpt;
use super::{R, RunManager, lock, rpc};
use crate::dispatch::ConnCtx;

/// A question that stays unanswered this long raises `question_waiting_10m`.
const QUESTION_NOTIFY_AFTER: Duration = Duration::from_secs(600);
const MESSAGES_LIMIT: usize = 1000;

/// Marks a run `waiting` while a blocking call is in progress; ending it (or dropping the
/// guard, for a handler that is aborted) moves the run back to `running`.
pub(crate) struct WaitGuard {
    mgr: RunManager,
    run_id: String,
    armed: bool,
}

impl WaitGuard {
    pub(crate) async fn end(mut self) {
        self.armed = false;
        self.mgr.end_wait(&self.run_id).await;
    }
}

impl Drop for WaitGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            let (mgr, id) = (self.mgr.clone(), self.run_id.clone());
            rt.spawn(async move { mgr.end_wait(&id).await });
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

    /// Starts a blocking call of `run_id`: the run is `waiting` until the guard ends.
    pub(crate) async fn begin_wait(&self, run_id: &str) -> WaitGuard {
        let first = {
            let mut runs = lock(&self.inner.runs);
            let rt = runs.entry(run_id.to_string()).or_default();
            rt.waits += 1;
            rt.waits == 1
        };
        if first {
            self.move_state(
                run_id,
                RunState::Running,
                RunState::Waiting,
                "blocking call",
            )
            .await;
        }
        WaitGuard {
            mgr: self.clone(),
            run_id: run_id.to_string(),
            armed: true,
        }
    }

    async fn end_wait(&self, run_id: &str) {
        let last = {
            let mut runs = lock(&self.inner.runs);
            match runs.get_mut(run_id) {
                Some(rt) => {
                    rt.waits = rt.waits.saturating_sub(1);
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
            json!({"to": row.id, "from": ctx.run_id, "kind": kind}),
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
            if self.is_waiting(&row.id) {
                // `ask_parent` returns it to the child.
                return Ok(AckResult { ok: true });
            }
        }
        self.deliver(&row, &msg).await?;
        Ok(AckResult { ok: true })
    }

    async fn deliver(&self, row: &RunRow, msg: &MessageRow) -> R<()> {
        match row.state {
            RunState::Queued | RunState::Starting => Ok(()),
            RunState::Succeeded | RunState::Failed | RunState::PendingReview => {
                self.resume_for_message(row, &msg.body).await?;
                self.mark_read(vec![msg.id.clone()]).await;
                Ok(())
            }
            _ => {
                let control = lock(&self.inner.runs)
                    .get(&row.id)
                    .and_then(|rt| rt.live.as_ref().map(|l| l.control.clone()));
                if let Some(control) = control {
                    control.send(&msg.body).await.map_err(|e| {
                        rpc(UNAVAILABLE, format!("could not deliver the message: {e}"))
                    })?;
                    self.mark_read(vec![msg.id.clone()]).await;
                }
                Ok(())
            }
        }
    }

    /// Brings a finished run (or one at the review gate) back to `running` and resumes
    /// its native session with `text`.
    async fn resume_for_message(&self, row: &RunRow, text: &str) -> R<()> {
        let id = row.id.clone();
        let from = row.state;
        let moved = if from == RunState::PendingReview {
            self.db_raw(move |s| s.update_run_state(&id, from, RunState::Running))
                .await
                .is_ok()
        } else {
            self.db(move |s| s.reopen_run(&id)).await?
        };
        if !moved {
            // Someone else already resumed it; the message waits unread.
            return Ok(());
        }
        lock(&self.inner.runs).entry(row.id.clone()).or_default();
        self.emit(
            &row.session_id,
            Some(&row.id),
            EventType::StateChanged,
            json!({"from": from, "to": RunState::Running, "reason": "message"}),
        )
        .await;
        if let Err(reason) = self.resume_run(row, text).await {
            self.fail_run(&row.id, so_core::ErrorKind::Other, &reason)
                .await;
            return Err(rpc(UNAVAILABLE, reason));
        }
        Ok(())
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

        let guard = self.begin_wait(&row.id).await;
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
        let notify_at = started + QUESTION_NOTIFY_AFTER;
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
                )
                .await;
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
                    .unread_messages(&sid, Some(&id), MESSAGES_LIMIT)?
                    .into_iter()
                    .filter(|m| m.kind == MessageKind::Answer)
                    .map(|m| m.id)
                    .collect();
                s.mark_messages_read(&ids, Utc::now())
            })
            .await;
    }

    /// The oldest unread answer for the run, marked read.
    async fn take_answer(&self, row: &RunRow) -> R<Option<String>> {
        let (sid, id) = (row.session_id.clone(), row.id.clone());
        self.db(move |s| {
            let first = s
                .unread_messages(&sid, Some(&id), MESSAGES_LIMIT)?
                .into_iter()
                .find(|m| m.kind == MessageKind::Answer);
            if let Some(m) = &first {
                s.mark_messages_read(std::slice::from_ref(&m.id), Utc::now())?;
            }
            Ok(first.map(|m| m.body))
        })
        .await
    }
}
