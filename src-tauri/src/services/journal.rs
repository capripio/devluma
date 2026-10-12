//! Operation journal: the single way long-running work records its lifecycle.
//!
//! Each call commits to SQLite first and only then emits `operation://progress`, so the UI
//! never shows a state that a restart would lose. Intent is journaled before side effects
//! (`start`), completion after (`succeed` / `fail`).

use serde::Serialize;

use crate::domain::operation::OperationStatus;
use crate::error::ErrorCode;
use crate::persistence::operations::{self as ops, EventLevel, Failure, NewOperation, Operation};
use crate::persistence::{Database, DbResult};

pub const PROGRESS_EVENT: &str = "operation://progress";

/// Event payload and list item for the UI (`OperationProgress` in `src/types/dto.ts`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationProgress {
    pub operation_id: String,
    pub operation_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
    pub status: OperationStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safe_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    pub started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
}

impl From<&Operation> for OperationProgress {
    fn from(op: &Operation) -> Self {
        Self {
            operation_id: op.id.clone(),
            operation_type: op.operation_type.clone(),
            target_id: op.target_id.clone(),
            status: op.status,
            phase: op.phase.clone(),
            current: op.progress_current,
            total: op.progress_total,
            safe_message: op.safe_error_message.clone(),
            error_code: op.error_code.clone(),
            started_at: op.started_at.clone(),
            finished_at: op.finished_at.clone(),
        }
    }
}

/// Where progress goes after it's committed (the Tauri app handle in production).
pub trait ProgressSink: Send + Sync {
    fn emit(&self, progress: &OperationProgress);
}

/// For work that runs before the UI exists (startup); the journal row is still written.
pub struct NullSink;

impl ProgressSink for NullSink {
    fn emit(&self, _progress: &OperationProgress) {}
}

impl<R: tauri::Runtime> ProgressSink for tauri::AppHandle<R> {
    fn emit(&self, progress: &OperationProgress) {
        use tauri::Emitter;
        // Emission is best-effort: the journal row is already committed and the UI
        // reconciles from `list_operations` on load.
        let _ = Emitter::emit(self, PROGRESS_EVENT, progress);
    }
}

pub struct Journal<'a> {
    db: &'a Database,
    sink: &'a dyn ProgressSink,
}

impl<'a> Journal<'a> {
    pub fn new(db: &'a Database, sink: &'a dyn ProgressSink) -> Self {
        Self { db, sink }
    }

    fn commit(
        &self,
        f: impl FnOnce(&rusqlite::Transaction<'_>) -> DbResult<Operation>,
    ) -> DbResult<Operation> {
        let op = self.db.write(f)?;
        self.sink.emit(&OperationProgress::from(&op));
        Ok(op)
    }

    /// Records intent (status `queued`) before any side effect happens.
    pub fn start(&self, new: &NewOperation<'_>) -> DbResult<Operation> {
        self.commit(|c| {
            let op = ops::create(c, new)?;
            ops::append_event(
                c,
                &op.id,
                "created",
                EventLevel::Info,
                "Operation queued",
                &serde_json::json!({}),
            )?;
            Ok(op)
        })
    }

    pub fn running(&self, id: &str) -> DbResult<Operation> {
        self.commit(|c| ops::transition(c, id, OperationStatus::Running, None))
    }

    pub fn progress(
        &self,
        id: &str,
        phase: &str,
        current: Option<i64>,
        total: Option<i64>,
    ) -> DbResult<Operation> {
        self.commit(|c| {
            let op = ops::set_progress(c, id, Some(phase), current, total)?;
            ops::append_event(
                c,
                id,
                "phase",
                EventLevel::Info,
                phase,
                &serde_json::json!({}),
            )?;
            Ok(op)
        })
    }

    /// Detail for the operation's log; not emitted as progress.
    pub fn event(
        &self,
        id: &str,
        level: EventLevel,
        message: &str,
        details: &serde_json::Value,
    ) -> DbResult<()> {
        self.db
            .write(|c| ops::append_event(c, id, "log", level, message, details).map(|_| ()))
    }

    pub fn succeed(&self, id: &str) -> DbResult<Operation> {
        self.finish(id, OperationStatus::Succeeded, None)
    }

    pub fn fail(&self, id: &str, code: ErrorCode, safe_message: &str) -> DbResult<Operation> {
        self.finish(
            id,
            OperationStatus::Failed,
            Some(Failure { code, safe_message }),
        )
    }

    pub fn cancel(&self, id: &str) -> DbResult<Operation> {
        self.finish(id, OperationStatus::Cancelled, None)
    }

    fn finish(
        &self,
        id: &str,
        to: OperationStatus,
        failure: Option<Failure<'_>>,
    ) -> DbResult<Operation> {
        let (level, message) = match to {
            OperationStatus::Succeeded => (EventLevel::Info, "Operation completed".to_owned()),
            OperationStatus::Cancelled => (EventLevel::Warn, "Operation cancelled".to_owned()),
            _ => (
                EventLevel::Error,
                failure
                    .as_ref()
                    .map_or("Operation failed".to_owned(), |f| f.safe_message.to_owned()),
            ),
        };
        self.commit(|c| {
            let op = ops::transition(c, id, to, failure)?;
            ops::append_event(c, id, "finished", level, &message, &serde_json::json!({}))?;
            Ok(op)
        })
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    pub struct RecordingSink(pub Mutex<Vec<OperationProgress>>);

    impl ProgressSink for RecordingSink {
        fn emit(&self, progress: &OperationProgress) {
            self.0.lock().unwrap().push(progress.clone());
        }
    }

    impl RecordingSink {
        pub fn statuses(&self) -> Vec<OperationStatus> {
            self.0.lock().unwrap().iter().map(|p| p.status).collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::RecordingSink;
    use super::*;
    use crate::persistence::DbError;
    use crate::persistence::test_support::db;
    use OperationStatus::*;

    const INSTALL: NewOperation<'static> = NewOperation {
        operation_type: "install_package",
        target_type: Some("package_version"),
        target_id: Some("php-8.4.1"),
    };

    #[test]
    fn emits_each_committed_state_in_order() {
        let db = db();
        let sink = RecordingSink::default();
        let j = Journal::new(&db, &sink);
        let op = j.start(&INSTALL).unwrap();
        j.running(&op.id).unwrap();
        j.progress(&op.id, "Downloading", Some(1), Some(4)).unwrap();
        j.succeed(&op.id).unwrap();
        assert_eq!(sink.statuses(), vec![Queued, Running, Running, Succeeded]);

        let events = db.read(|c| ops::list_events(c, &op.id)).unwrap();
        let types: Vec<_> = events.iter().map(|e| e.event_type.as_str()).collect();
        assert_eq!(types, vec!["created", "phase", "finished"]);
    }

    #[test]
    fn rejected_transition_emits_nothing() {
        let db = db();
        let sink = RecordingSink::default();
        let j = Journal::new(&db, &sink);
        let op = j.start(&INSTALL).unwrap();
        assert!(matches!(
            j.succeed(&op.id),
            Err(DbError::InvalidTransition { .. })
        ));
        assert_eq!(sink.statuses(), vec![Queued]);
    }

    #[test]
    fn failure_is_journaled_redacted_and_emitted() {
        let db = db();
        let sink = RecordingSink::default();
        let j = Journal::new(&db, &sink);
        let op = j.start(&INSTALL).unwrap();
        j.running(&op.id).unwrap();
        let failed = j
            .fail(
                &op.id,
                ErrorCode::IntegrityError,
                "Checksum mismatch for token=abc123",
            )
            .unwrap();
        assert_eq!(failed.status, Failed);
        let last = sink.0.lock().unwrap().last().cloned().unwrap();
        assert_eq!(last.error_code.as_deref(), Some("INTEGRITY_ERROR"));
        assert_eq!(
            last.safe_message.as_deref(),
            Some("Checksum mismatch for token=[REDACTED]")
        );
    }
}
