//! Operation lifecycle (schema.md `operations.status`).
//!
//! ```text
//! queued ──► running ──► succeeded
//!   │          │  ├────► failed
//!   │          │  └────► cancelled
//!   │          ▼
//!   └──► needs_recovery ──► running (resume) | failed | cancelled
//! queued ──► cancelled
//! ```
//! `needs_recovery` is set by startup reconciliation for work interrupted by an app exit.
//! Terminal states never change again.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    NeedsRecovery,
}

impl OperationStatus {
    pub const ALL: [Self; 6] = [
        Self::Queued,
        Self::Running,
        Self::Succeeded,
        Self::Failed,
        Self::Cancelled,
        Self::NeedsRecovery,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::NeedsRecovery => "needs_recovery",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }

    /// Work that is not finished: shown in the tray and reconciled at startup.
    pub fn is_unfinished(self) -> bool {
        !self.is_terminal()
    }

    pub fn can_transition_to(self, to: Self) -> bool {
        use OperationStatus::*;
        matches!(
            (self, to),
            (Queued, Running | Cancelled | NeedsRecovery)
                | (Running, Succeeded | Failed | Cancelled | NeedsRecovery)
                | (NeedsRecovery, Running | Failed | Cancelled)
        )
    }
}

impl fmt::Display for OperationStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for OperationStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|v| v.as_str() == s)
            .ok_or_else(|| format!("unknown operation status {s:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::OperationStatus::{self, *};

    #[test]
    fn terminal_states_are_final() {
        for from in [Succeeded, Failed, Cancelled] {
            for to in OperationStatus::ALL {
                assert!(
                    !from.can_transition_to(to),
                    "{from} -> {to} must be rejected"
                );
            }
        }
    }

    #[test]
    fn allowed_paths() {
        assert!(Queued.can_transition_to(Running));
        assert!(Running.can_transition_to(Succeeded));
        assert!(Running.can_transition_to(NeedsRecovery));
        assert!(NeedsRecovery.can_transition_to(Running));
        assert!(
            !Queued.can_transition_to(Succeeded),
            "must run before succeeding"
        );
        assert!(!Running.can_transition_to(Queued));
        assert!(
            !NeedsRecovery.can_transition_to(Succeeded),
            "must resume before succeeding"
        );
    }

    #[test]
    fn string_round_trip_matches_schema() {
        for s in OperationStatus::ALL {
            assert_eq!(s.as_str().parse::<OperationStatus>().unwrap(), s);
        }
        assert!("done".parse::<OperationStatus>().is_err());
    }
}
