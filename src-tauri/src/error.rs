//! Typed IPC error shape shared by every command (trd.md §12).
//!
//! Messages here are shown to users, so they must be safe: no secrets, no raw stack
//! traces, no unredacted process output. Put diagnostic detail in `details` only after
//! redaction.

use serde::Serialize;

use crate::persistence::DbError;

/// Stable error codes. Keep in sync with `ErrorCode` in `src/lib/ipc/errors.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    NetworkError,
    IntegrityError,
    PermissionDenied,
    PortInUse,
    ConfigInvalid,
    ProcessExited,
    DiskSpaceLow,
    DataDirectoryExists,
    RecoveryRequired,
    ValidationError,
    NotFound,
    NotImplemented,
    Internal,
}

impl ErrorCode {
    /// The serialized form, for storage in `operations.error_code`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NetworkError => "NETWORK_ERROR",
            Self::IntegrityError => "INTEGRITY_ERROR",
            Self::PermissionDenied => "PERMISSION_DENIED",
            Self::PortInUse => "PORT_IN_USE",
            Self::ConfigInvalid => "CONFIG_INVALID",
            Self::ProcessExited => "PROCESS_EXITED",
            Self::DiskSpaceLow => "DISK_SPACE_LOW",
            Self::DataDirectoryExists => "DATA_DIRECTORY_EXISTS",
            Self::RecoveryRequired => "RECOVERY_REQUIRED",
            Self::ValidationError => "VALIDATION_ERROR",
            Self::NotFound => "NOT_FOUND",
            Self::NotImplemented => "NOT_IMPLEMENTED",
            Self::Internal => "INTERNAL",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IpcError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    pub recoverable: bool,
}

impl IpcError {
    pub fn new(code: ErrorCode, message: impl Into<String>, recoverable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
            operation_id: None,
            recoverable,
        }
    }

    pub fn internal() -> Self {
        Self::new(
            ErrorCode::Internal,
            "Devluma hit an unexpected internal error.",
            true,
        )
    }
}

pub type IpcResult<T> = Result<T, IpcError>;

impl From<&DbError> for IpcError {
    fn from(err: &DbError) -> Self {
        use DbError::*;
        match err {
            NotFound { entity } => Self::new(
                ErrorCode::NotFound,
                format!("The requested {entity} no longer exists."),
                false,
            ),
            InvalidTransition { from, to } => Self::new(
                ErrorCode::ValidationError,
                format!("This operation can't change from {from} to {to}."),
                false,
            ),
            Invariant(message) => Self::new(ErrorCode::ValidationError, message.clone(), false),
            ChecksumMismatch { .. } | NewerSchema { .. } => Self::new(
                ErrorCode::RecoveryRequired,
                "Devluma's metadata database was written by a different version of Devluma. It was left unchanged.",
                false,
            ),
            MigrationFailed { .. } => Self::new(
                ErrorCode::RecoveryRequired,
                "Updating Devluma's metadata database failed. It was left at its previous version.",
                false,
            ),
            // Raw SQLite/IO messages can contain paths or values; never forward them.
            Sqlite(_) | Io(_) | Json(_) => Self::new(
                ErrorCode::Internal,
                "A metadata database error occurred.",
                true,
            ),
        }
    }
}

impl From<DbError> for IpcError {
    fn from(err: DbError) -> Self {
        Self::from(&err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_to_ipc_contract() {
        let err = IpcError::new(ErrorCode::PortInUse, "Port 80 is in use.", true);
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "code": "PORT_IN_USE",
                "message": "Port 80 is in use.",
                "recoverable": true
            })
        );
    }

    #[test]
    fn as_str_matches_serde() {
        for code in [
            ErrorCode::NetworkError,
            ErrorCode::NotFound,
            ErrorCode::DataDirectoryExists,
        ] {
            assert_eq!(serde_json::to_value(code).unwrap(), code.as_str());
        }
    }

    #[test]
    fn database_internals_are_not_leaked() {
        let raw = rusqlite::Error::InvalidPath("C:\\Users\\someone\\secret.db".into());
        let ipc = IpcError::from(DbError::Sqlite(raw));
        assert_eq!(ipc.code, ErrorCode::Internal);
        assert!(!ipc.message.contains("secret"));
    }
}
