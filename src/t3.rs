use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentPhase {
    Idle,
    Starting,
    Running,
    WaitingApproval,
    WaitingInput,
    Completed,
    Failed,
}

#[derive(Clone, Debug)]
pub struct ThreadSlot {
    pub title: String,
    pub phase: AgentPhase,
}

pub struct T3State {
    connection: Connection,
}

impl T3State {
    pub fn open(path: &Path) -> Result<Self> {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("failed to open T3 state database {}", path.display()))?;
        connection.busy_timeout(std::time::Duration::from_millis(100))?;
        Ok(Self { connection })
    }

    pub fn slots(&self) -> Result<Vec<ThreadSlot>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT t.title,
                    t.pending_approval_count,
                    t.pending_user_input_count,
                    COALESCE(s.status, ''),
                    COALESCE(v.state, ''),
                    v.completed_at
             FROM projection_threads t
             LEFT JOIN projection_thread_sessions s ON s.thread_id = t.thread_id
             LEFT JOIN projection_turns v ON v.row_id = (
               SELECT row_id FROM projection_turns
               WHERE thread_id = t.thread_id ORDER BY requested_at DESC LIMIT 1
             )
             WHERE t.deleted_at IS NULL AND t.archived_at IS NULL
             ORDER BY
               CASE
                 WHEN t.settled_override = 'settled'
                  AND t.pending_approval_count = 0
                  AND t.pending_user_input_count = 0
                  AND COALESCE(s.status, '') NOT IN ('starting', 'running')
                 THEN 1 ELSE 0
               END ASC,
               CASE
                 WHEN t.settled_override IS NULL OR t.settled_override != 'settled'
                   OR t.pending_approval_count > 0 OR t.pending_user_input_count > 0
                   OR COALESCE(s.status, '') IN ('starting', 'running')
                 THEN t.created_at
               END DESC,
               CASE WHEN t.settled_override = 'settled'
                 THEN COALESCE(t.latest_user_message_at, t.updated_at)
               END DESC,
               t.thread_id ASC
             LIMIT 3",
        )?;
        let rows = statement.query_map([], |row| {
            let approval: i64 = row.get(1)?;
            let input: i64 = row.get(2)?;
            let session: String = row.get(3)?;
            let turn: String = row.get(4)?;
            let completed_at: Option<String> = row.get(5)?;
            Ok(ThreadSlot {
                title: row.get(0)?,
                phase: resolve_phase(approval, input, &session, &turn, completed_at.is_some()),
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("failed reading T3 thread slots")
    }
}

fn resolve_phase(
    approval: i64,
    input: i64,
    session: &str,
    turn: &str,
    completed: bool,
) -> AgentPhase {
    if approval > 0 {
        return AgentPhase::WaitingApproval;
    }
    if input > 0 {
        return AgentPhase::WaitingInput;
    }
    if matches!(session, "error" | "failed") || matches!(turn, "error" | "failed") {
        return AgentPhase::Failed;
    }
    if session == "starting" {
        return AgentPhase::Starting;
    }
    if session == "running" || matches!(turn, "running" | "in_progress") {
        return AgentPhase::Running;
    }
    if turn == "completed"
        || (turn == "interrupted" && completed)
        || matches!(session, "ready" | "idle")
    {
        return AgentPhase::Completed;
    }
    AgentPhase::Idle
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blockers_take_priority_over_running() {
        assert_eq!(
            resolve_phase(1, 0, "running", "running", false),
            AgentPhase::WaitingApproval
        );
        assert_eq!(
            resolve_phase(0, 1, "running", "running", false),
            AgentPhase::WaitingInput
        );
    }

    #[test]
    fn recognizes_terminal_states() {
        assert_eq!(resolve_phase(0, 0, "error", "", false), AgentPhase::Failed);
        assert_eq!(
            resolve_phase(0, 0, "ready", "completed", true),
            AgentPhase::Completed
        );
    }
}
