//! Shared Session summary aggregation.
use so_store::{SessionRow, SessionStats, Store};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Active,
    Escalated,
    Failed,
    Done,
}

pub struct Summary {
    pub stats: SessionStats,
    pub status: Status,
}

pub fn summarize(store: &Store, row: &SessionRow) -> so_store::Result<Summary> {
    let stats = store.session_stats(&row.id)?;
    let status = if row.ended_at.is_none() {
        if stats.escalated > 0 {
            Status::Escalated
        } else {
            Status::Active
        }
    } else if row.status == "failed" || stats.failed > 0 {
        Status::Failed
    } else {
        Status::Done
    };
    Ok(Summary { stats, status })
}
