//! Session lifecycle, recovery state, and the stop-reason vocabulary.

use serde::{Deserialize, Serialize};

use crate::ids::{KernelId, SessionId, SubjectId};
use crate::terminal::TerminalGeometry;

/// Lifecycle state of a session. `Idle` means the runtime was reclaimed while
/// the session itself remains valid and revivable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// Runtime is being provisioned.
    Provisioning,
    /// Runtime is up and accepting work.
    Ready,
    /// A turn is in progress.
    Running,
    /// Runtime reclaimed; session intact and revivable.
    Idle,
    /// Terminal state.
    Terminated,
}

impl SessionStatus {
    /// Whether no further work can occur under this session id.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Terminated)
    }
}

/// Why a session stopped or paused.
///
/// `RequiresAction` is the trap: it means the runtime is waiting on the client
/// and is explicitly **not** terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The turn finished normally.
    EndTurn,
    /// The owner asked for termination.
    UserTerminated,
    /// Reclaimed after being idle too long.
    IdleTimeout,
    /// Hit the maximum lifetime.
    LifetimeCap,
    /// Waiting on the client for an approval or a tool result.
    RequiresAction,
    /// Credentials or authorization were withdrawn.
    Revoked,
    /// A spend or usage ceiling was reached.
    BudgetReached,
    /// Stopped by a failure.
    Error,
}

impl StopReason {
    /// Whether this reason means the session can never run again.
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::RequiresAction)
    }
}

/// Operational health, reported alongside lifecycle so operators read this
/// control plane the same way they read neighbouring ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryState {
    /// Nothing unusual.
    Normal,
    /// Reclaimed, revivable.
    Idle,
    /// Being restored. Routine during a revive.
    Recovering,
    /// Running with reduced capability.
    Degraded,
    /// The control plane cannot observe the runtime right now.
    Unknown,
    /// Permanently withdrawn from service.
    Retired,
    /// Configuration is not usable.
    Invalid,
}

impl RecoveryState {
    /// Whether a client should render this as a failure.
    ///
    /// `Recovering` and `Unknown` are routine: the first happens on every
    /// revive, the second means observation is temporarily unavailable. Neither
    /// means the session is broken, and painting them as errors is the single
    /// most common client bug this predicate exists to prevent.
    pub fn is_failure(self) -> bool {
        matches!(self, Self::Degraded | Self::Retired | Self::Invalid)
    }
}

/// What a caller asks for when creating a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSpec {
    /// Which runtime implementation should back the session.
    pub kernel: KernelId,
    /// Terminal geometry to start the runtime at.
    #[serde(default)]
    pub geometry: Option<TerminalGeometry>,
}

/// A session and everything the core knows about it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Stable identifier.
    pub id: SessionId,
    /// The principal that owns it. Every operation is scoped to this subject.
    pub owner: SubjectId,
    /// Which runtime backs it.
    pub kernel: KernelId,
    /// Lifecycle position.
    pub status: SessionStatus,
    /// Operational health.
    pub recovery_state: RecoveryState,
    /// Why it stopped, when it has stopped.
    pub stop_reason: Option<StopReason>,
    /// Current terminal geometry.
    pub geometry: TerminalGeometry,
    /// Highest sequence number appended for this session.
    pub last_seq: u64,
    /// Creation timestamp, RFC 3339 in UTC.
    pub created_at: String,
}

impl Session {
    /// Whether `subject` may operate on this session.
    ///
    /// Ownership is the whole authorization model at this layer: there is no
    /// sharing, no delegation, and no role hierarchy in this version.
    pub fn is_owned_by(&self, subject: &SubjectId) -> bool {
        &self.owner == subject
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_action_is_not_terminal() {
        assert!(!StopReason::RequiresAction.is_terminal());
        assert!(StopReason::EndTurn.is_terminal());
        assert!(StopReason::Error.is_terminal());
    }

    #[test]
    fn routine_recovery_states_are_not_failures() {
        assert!(!RecoveryState::Recovering.is_failure());
        assert!(!RecoveryState::Unknown.is_failure());
        assert!(!RecoveryState::Normal.is_failure());
        assert!(!RecoveryState::Idle.is_failure());
    }

    #[test]
    fn genuinely_broken_states_are_failures() {
        assert!(RecoveryState::Degraded.is_failure());
        assert!(RecoveryState::Retired.is_failure());
        assert!(RecoveryState::Invalid.is_failure());
    }

    #[test]
    fn only_terminated_is_a_terminal_status() {
        assert!(SessionStatus::Terminated.is_terminal());
        for s in [
            SessionStatus::Provisioning,
            SessionStatus::Ready,
            SessionStatus::Running,
            SessionStatus::Idle,
        ] {
            assert!(!s.is_terminal(), "{s:?} must not be terminal");
        }
    }

    #[test]
    fn ownership_is_exact_match_only() {
        let owner = SubjectId("subject-a".into());
        let session = Session {
            id: SessionId::generate(),
            owner: owner.clone(),
            kernel: KernelId::new("example-runtime"),
            status: SessionStatus::Ready,
            recovery_state: RecoveryState::Normal,
            stop_reason: None,
            geometry: TerminalGeometry::default(),
            last_seq: 0,
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        assert!(session.is_owned_by(&owner));
        assert!(!session.is_owned_by(&SubjectId("subject-b".into())));
    }
}
