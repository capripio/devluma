//! Operation journal rows (`operations`, `operation_events`).
//! All text that could carry secrets is redacted *before* insert.

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;

use super::{DbError, DbResult, new_id, now};
use crate::domain::operation::OperationStatus;
use crate::error::ErrorCode;
use crate::security::redact::{redact, redact_json};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    pub id: String,
    pub operation_type: String,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub status: OperationStatus,
    pub phase: Option<String>,
    pub progress_current: Option<i64>,
    pub progress_total: Option<i64>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub error_code: Option<String>,
    pub safe_error_message: Option<String>,
    pub recovery_data: serde_json::Value,
}

const COLUMNS: &str = "id, operation_type, target_type, target_id, status, phase, progress_current, \
     progress_total, started_at, finished_at, error_code, safe_error_message, recovery_data_json";

fn from_row(r: &Row<'_>) -> rusqlite::Result<Operation> {
    let status: String = r.get(4)?;
    let recovery: String = r.get(12)?;
    Ok(Operation {
        id: r.get(0)?,
        operation_type: r.get(1)?,
        target_type: r.get(2)?,
        target_id: r.get(3)?,
        status: status.parse().map_err(|e: String| {
            rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, e.into())
        })?,
        phase: r.get(5)?,
        progress_current: r.get(6)?,
        progress_total: r.get(7)?,
        started_at: r.get(8)?,
        finished_at: r.get(9)?,
        error_code: r.get(10)?,
        safe_error_message: r.get(11)?,
        recovery_data: serde_json::from_str(&recovery).unwrap_or(serde_json::Value::Null),
    })
}

#[derive(Debug, Clone, Default)]
pub struct NewOperation<'a> {
    pub operation_type: &'a str,
    pub target_type: Option<&'a str>,
    pub target_id: Option<&'a str>,
}

pub fn create(conn: &Connection, new: &NewOperation<'_>) -> DbResult<Operation> {
    let id = new_id();
    conn.execute(
        "INSERT INTO operations (id, operation_type, target_type, target_id, status, started_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            id,
            new.operation_type,
            new.target_type,
            new.target_id,
            OperationStatus::Queued.as_str(),
            now()
        ],
    )?;
    get(conn, &id)
}

pub fn get(conn: &Connection, id: &str) -> DbResult<Operation> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM operations WHERE id = ?1"),
        [id],
        from_row,
    )
    .optional()?
    .ok_or(DbError::NotFound {
        entity: "operation",
    })
}

pub struct Failure<'a> {
    pub code: ErrorCode,
    /// Shown to users; redacted before storage.
    pub safe_message: &'a str,
}

/// Moves an operation along the state machine. Terminal states get `finished_at`.
pub fn transition(
    conn: &Connection,
    id: &str,
    to: OperationStatus,
    failure: Option<Failure<'_>>,
) -> DbResult<Operation> {
    let current = get(conn, id)?;
    if !current.status.can_transition_to(to) {
        return Err(DbError::InvalidTransition {
            from: current.status,
            to,
        });
    }
    let finished_at = to.is_terminal().then(now);
    let (code, message) = match failure {
        Some(f) => (
            Some(f.code.as_str()),
            Some(redact(f.safe_message).into_owned()),
        ),
        None => (None, None),
    };
    conn.execute(
        "UPDATE operations
         SET status = ?2, finished_at = ?3,
             error_code = COALESCE(?4, error_code),
             safe_error_message = COALESCE(?5, safe_error_message)
         WHERE id = ?1",
        params![id, to.as_str(), finished_at, code, message],
    )?;
    get(conn, id)
}

pub fn set_progress(
    conn: &Connection,
    id: &str,
    phase: Option<&str>,
    current: Option<i64>,
    total: Option<i64>,
) -> DbResult<Operation> {
    let op = get(conn, id)?;
    if op.status.is_terminal() {
        return Err(DbError::Invariant(format!(
            "Operation {id} has already finished ({})",
            op.status
        )));
    }
    conn.execute(
        "UPDATE operations SET phase = ?2, progress_current = ?3, progress_total = ?4 WHERE id = ?1",
        params![id, phase.map(|p| redact(p).into_owned()), current, total],
    )?;
    get(conn, id)
}

/// What a resume needs (staging path, previous selection, ...). Never secrets.
pub fn set_recovery_data(conn: &Connection, id: &str, data: &serde_json::Value) -> DbResult<()> {
    let json = serde_json::to_string(&redact_json(data))?;
    let n = conn.execute(
        "UPDATE operations SET recovery_data_json = ?2 WHERE id = ?1",
        params![id, json],
    )?;
    if n == 0 {
        return Err(DbError::NotFound {
            entity: "operation",
        });
    }
    Ok(())
}

/// Unfinished operations (any age) plus the most recent `recent_finished` finished ones,
/// newest first. This is what the operation tray and overview show.
pub fn list_for_display(conn: &Connection, recent_finished: u32) -> DbResult<Vec<Operation>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM operations WHERE status IN ('queued', 'running', 'needs_recovery')
         UNION ALL
         SELECT * FROM (SELECT {COLUMNS} FROM operations
                        WHERE status IN ('succeeded', 'failed', 'cancelled')
                        ORDER BY started_at DESC, id DESC LIMIT ?1)
         ORDER BY started_at DESC, id DESC"
    ))?;
    Ok(stmt
        .query_map([recent_finished], from_row)?
        .collect::<Result<_, _>>()?)
}

pub fn list_unfinished(conn: &Connection) -> DbResult<Vec<Operation>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM operations WHERE status IN ('queued', 'running', 'needs_recovery')
         ORDER BY started_at, id"
    ))?;
    Ok(stmt.query_map([], from_row)?.collect::<Result<_, _>>()?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl EventLevel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationEvent {
    pub id: String,
    pub operation_id: String,
    pub sequence: i64,
    pub event_type: String,
    pub level: String,
    pub message: String,
    pub details: serde_json::Value,
    pub created_at: String,
}

/// Appends an event with the next sequence number. Message and details are redacted.
pub fn append_event(
    conn: &Connection,
    operation_id: &str,
    event_type: &str,
    level: EventLevel,
    message: &str,
    details: &serde_json::Value,
) -> DbResult<OperationEvent> {
    let id = new_id();
    let sequence: i64 = conn.query_row(
        "SELECT COALESCE(MAX(sequence), 0) + 1 FROM operation_events WHERE operation_id = ?1",
        [operation_id],
        |r| r.get(0),
    )?;
    let message = redact(message).into_owned();
    let details = redact_json(details);
    let created_at = now();
    conn.execute(
        "INSERT INTO operation_events (id, operation_id, sequence, event_type, level, message, details_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            id,
            operation_id,
            sequence,
            event_type,
            level.as_str(),
            message,
            serde_json::to_string(&details)?,
            created_at
        ],
    )?;
    Ok(OperationEvent {
        id,
        operation_id: operation_id.to_owned(),
        sequence,
        event_type: event_type.to_owned(),
        level: level.as_str().to_owned(),
        message,
        details,
        created_at,
    })
}

pub fn list_events(conn: &Connection, operation_id: &str) -> DbResult<Vec<OperationEvent>> {
    let mut stmt = conn.prepare(
        "SELECT id, operation_id, sequence, event_type, level, message, details_json, created_at
         FROM operation_events WHERE operation_id = ?1 ORDER BY sequence",
    )?;
    Ok(stmt
        .query_map([operation_id], |r| {
            let details: String = r.get(6)?;
            Ok(OperationEvent {
                id: r.get(0)?,
                operation_id: r.get(1)?,
                sequence: r.get(2)?,
                event_type: r.get(3)?,
                level: r.get(4)?,
                message: r.get(5)?,
                details: serde_json::from_str(&details).unwrap_or(serde_json::Value::Null),
                created_at: r.get(7)?,
            })
        })?
        .collect::<Result<_, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::test_support::db;
    use OperationStatus::*;

    fn op(conn: &Connection) -> Operation {
        create(
            conn,
            &NewOperation {
                operation_type: "install_package",
                target_type: Some("package_version"),
                target_id: Some("php-8.4"),
            },
        )
        .unwrap()
    }

    #[test]
    fn lifecycle_success_sets_finished_at() {
        let db = db();
        db.write(|c| {
            let o = op(c);
            assert_eq!(o.status, Queued);
            transition(c, &o.id, Running, None)?;
            let o = set_progress(c, &o.id, Some("Downloading"), Some(5), Some(10))?;
            assert_eq!((o.progress_current, o.progress_total), (Some(5), Some(10)));
            let o = transition(c, &o.id, Succeeded, None)?;
            assert!(o.finished_at.is_some());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn invalid_transitions_are_rejected_and_nothing_changes() {
        let db = db();
        let id = db.write(|c| Ok(op(c).id)).unwrap();
        let err = db
            .write(|c| transition(c, &id, Succeeded, None))
            .unwrap_err();
        assert!(matches!(
            err,
            DbError::InvalidTransition {
                from: Queued,
                to: Succeeded
            }
        ));
        assert_eq!(db.read(|c| get(c, &id)).unwrap().status, Queued);
    }

    #[test]
    fn finished_operations_reject_progress() {
        let db = db();
        let id = db
            .write(|c| {
                let o = op(c);
                transition(c, &o.id, Cancelled, None)?;
                Ok(o.id)
            })
            .unwrap();
        assert!(matches!(
            db.write(|c| set_progress(c, &id, Some("x"), None, None)),
            Err(DbError::Invariant(_))
        ));
    }

    #[test]
    fn failure_message_is_redacted() {
        let db = db();
        let o = db
            .write(|c| {
                let o = op(c);
                transition(c, &o.id, Running, None)?;
                transition(
                    c,
                    &o.id,
                    Failed,
                    Some(Failure {
                        code: ErrorCode::NetworkError,
                        safe_message: "GET https://user:hunter22@mirror/x failed",
                    }),
                )
            })
            .unwrap();
        assert_eq!(o.error_code.as_deref(), Some("NETWORK_ERROR"));
        assert!(!o.safe_error_message.unwrap().contains("hunter22"));
    }

    #[test]
    fn events_are_sequenced_and_redacted() {
        let db = db();
        let id = db.write(|c| Ok(op(c).id)).unwrap();
        db.write(|c| {
            append_event(
                c,
                &id,
                "phase",
                EventLevel::Info,
                "Starting",
                &serde_json::json!({}),
            )?;
            append_event(
                c,
                &id,
                "process_output",
                EventLevel::Warn,
                "mariadb --password=hunter22",
                &serde_json::json!({ "token": "abc123" }),
            )
        })
        .unwrap();
        let events = db.read(|c| list_events(c, &id)).unwrap();
        assert_eq!(
            events.iter().map(|e| e.sequence).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(events[1].message, "mariadb --password=[REDACTED]");
        assert_eq!(
            events[1].details,
            serde_json::json!({ "token": "[REDACTED]" })
        );
        let raw: String = db
            .read(|c| {
                Ok(c.query_row(
                    "SELECT group_concat(message || details_json) FROM operation_events",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert!(!raw.contains("hunter22") && !raw.contains("abc123"));
    }

    #[test]
    fn display_list_keeps_all_unfinished_and_limits_finished() {
        let db = db();
        db.write(|c| {
            for _ in 0..5 {
                let o = op(c);
                transition(c, &o.id, Running, None)?;
                transition(c, &o.id, Succeeded, None)?;
            }
            let pending = op(c);
            transition(c, &pending.id, Running, None)?;
            Ok(())
        })
        .unwrap();
        let list = db.read(|c| list_for_display(c, 2)).unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list.iter().filter(|o| o.status == Running).count(), 1);
    }

    #[test]
    fn deleting_an_operation_cascades_to_its_events() {
        let db = db();
        let id = db.write(|c| Ok(op(c).id)).unwrap();
        db.write(|c| {
            append_event(
                c,
                &id,
                "phase",
                EventLevel::Info,
                "x",
                &serde_json::json!({}),
            )
        })
        .unwrap();
        db.write(|c| Ok(c.execute("DELETE FROM operations WHERE id = ?1", [&id])?))
            .unwrap();
        assert!(db.read(|c| list_events(c, &id)).unwrap().is_empty());
    }
}
