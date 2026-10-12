//! Service lifecycle states (trd.md §7).
//!
//! ```text
//! stopped ─► starting ─► running ⇄ degraded
//!    ▲           │          │  │       │
//!    │           ▼          │  ▼       ▼
//!    └──────── failed ◄─────┘ stopping ─► stopped
//! unknown ─► running (adopted) | stopped
//! ```
//! `failed` means the process exited without being asked to, or never became ready.
//! Every state change goes through [`ServiceState::can_transition_to`].

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    NotInstalled,
    Stopped,
    Starting,
    Running,
    Degraded,
    Stopping,
    Failed,
    Unknown,
}

impl ServiceState {
    pub const ALL: [Self; 8] = [
        Self::NotInstalled,
        Self::Stopped,
        Self::Starting,
        Self::Running,
        Self::Degraded,
        Self::Stopping,
        Self::Failed,
        Self::Unknown,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotInstalled => "not_installed",
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Degraded => "degraded",
            Self::Stopping => "stopping",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.as_str() == s)
    }

    /// A process is (believed to be) alive in these states.
    pub fn is_live(self) -> bool {
        matches!(
            self,
            Self::Starting | Self::Running | Self::Degraded | Self::Stopping
        )
    }

    pub fn can_start(self) -> bool {
        matches!(self, Self::Stopped | Self::Failed)
    }

    pub fn can_stop(self) -> bool {
        matches!(self, Self::Running | Self::Degraded | Self::Starting)
    }

    pub fn can_transition_to(self, to: Self) -> bool {
        use ServiceState::*;
        matches!(
            (self, to),
            (Stopped | Failed, Starting)
                | (Starting, Running | Failed | Stopping)
                | (Running, Degraded | Stopping | Failed)
                | (Degraded, Running | Stopping | Failed)
                | (Stopping, Stopped | Failed)
                | (Failed, Stopped)
                | (Unknown, Running | Degraded | Stopped)
                | (NotInstalled, Stopped)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::ServiceState::{self, *};

    #[test]
    fn normal_lifecycle() {
        let path = [
            Stopped, Starting, Running, Degraded, Running, Stopping, Stopped,
        ];
        for w in path.windows(2) {
            assert!(w[0].can_transition_to(w[1]), "{:?} -> {:?}", w[0], w[1]);
        }
    }

    #[test]
    fn no_shortcuts() {
        assert!(
            !Stopped.can_transition_to(Running),
            "must go through starting"
        );
        assert!(
            !Running.can_transition_to(Stopped),
            "must go through stopping"
        );
        assert!(!Stopping.can_transition_to(Running));
        assert!(!Failed.can_transition_to(Running));
    }

    #[test]
    fn round_trips_and_guards() {
        for s in ServiceState::ALL {
            assert_eq!(ServiceState::parse(s.as_str()), Some(s));
        }
        assert!(Failed.can_start() && Stopped.can_start() && !Running.can_start());
        assert!(Running.can_stop() && !Stopped.can_stop());
    }
}
