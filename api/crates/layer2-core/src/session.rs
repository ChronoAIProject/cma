//! Session lifecycle, recovery state, and the stop-reason vocabulary.

use serde::{Deserialize, Serialize};

use crate::ids::{EnvironmentProfileId, KernelId, SessionId, SubjectId};
use crate::options::AgentOptions;
use crate::terminal::TerminalGeometry;

/// A named step in bringing a session up.
///
/// Ordered, and each knows its position, so a client can show how far along
/// something is without hard-coding the sequence — adding a step here changes
/// what every client displays rather than requiring each to be taught.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvisioningStage {
    /// The request arrived and the session exists.
    Accepted,
    /// A runtime has been asked for.
    RuntimeRequested,
    /// The provider gave us one.
    RuntimeCreated,
    /// Connecting to its terminal.
    Attaching,
    /// Usable.
    Ready,
}

impl ProvisioningStage {
    /// Every stage, in the order they happen.
    pub const ALL: [Self; 5] = [
        Self::Accepted,
        Self::RuntimeRequested,
        Self::RuntimeCreated,
        Self::Attaching,
        Self::Ready,
    ];

    /// How far through the sequence this stage sits, counting from one.
    pub fn position(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0) + 1
    }

    /// How many stages there are in total.
    pub fn total() -> usize {
        Self::ALL.len()
    }
}

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
    /// Being carried onto a different runtime.
    ///
    /// Neither usable nor stopped. The session is intact and will come back on
    /// its own, but the runtime under it is being replaced, so anything typed
    /// now would go to a sandbox that is about to stop existing. Distinct from
    /// `Provisioning` because nothing was asked for and nothing is starting
    /// from nothing — there is a conversation being carried across.
    Rotating,
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

    /// Whether this session is mid-handover between runtimes.
    pub fn is_rotating(self) -> bool {
        matches!(self, Self::Rotating)
    }

    /// Whether this session should have a runtime running for it right now.
    ///
    /// The desired state, against which reality is reconciled. A stopped
    /// session deliberately wants none — that is what stopping meant — and a
    /// terminated one is over.
    pub fn wants_runtime(self) -> bool {
        matches!(
            self,
            Self::Provisioning | Self::Ready | Self::Running | Self::Rotating
        )
    }

    /// Whether a client should accept input for this session.
    ///
    /// Rotation is the case worth naming: the session is healthy, it is not
    /// stopped, and input must still be refused, because the runtime it would
    /// reach is being torn down.
    pub fn is_interactive(self) -> bool {
        matches!(self, Self::Ready | Self::Running)
    }

    /// Whether the owner can pick this session up again.
    ///
    /// Stopping is reversible and ending is not; a person needs to know which
    /// one they are looking at without having to remember what they did.
    pub fn is_revivable(self) -> bool {
        matches!(self, Self::Idle)
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
    /// How the agent should behave, chosen at creation and fixed for the
    /// session's life.
    ///
    /// Filled in from the profile the session names, like the kernel: what a
    /// caller sent here would be discarded, which is why a caller is not asked.
    pub options: AgentOptions,
    /// Terminal geometry to start the runtime at.
    #[serde(default)]
    pub geometry: Option<TerminalGeometry>,
    /// A reusable environment bundle to start the session with.
    ///
    /// Absent does not mean bare: what the platform mandates is applied either
    /// way, so this selects what to add rather than whether to have anything
    /// at all.
    #[serde(default)]
    pub environment_profile: Option<EnvironmentProfileId>,
    /// A credential the owner has delegated to the runtime, if any.
    ///
    /// The core stores and forwards this without interpreting it. It is never
    /// serialized back out, never logged, and never rendered by an interface:
    /// it exists so the runtime can act on the owner's behalf, not so anything
    /// here can read it.
    #[serde(skip)]
    pub subject_credential: Option<String>,
}

/// A session and everything the core knows about it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// How this session's agent was configured.
    ///
    /// Persisted with the session rather than only being passed at creation,
    /// because reviving a stopped session has to produce the session someone
    /// had. Rebuilding it on defaults would silently swap the model out from
    /// under them.
    #[serde(default)]
    pub options: AgentOptions,
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
    /// The environment bundle this session was started with, if any.
    ///
    /// Held with the session rather than only in the request that created it,
    /// for the same reason as its options: a runtime replaced later has to be
    /// built the same way, and a session that quietly lost its tools would be
    /// a different session wearing the same identifier.
    #[serde(default)]
    pub environment_profile: Option<EnvironmentProfileId>,
    /// Which runtime version is serving this session.
    ///
    /// The control plane knows which version it expects; this says which one
    /// the session actually has, and a session whose version is behind is
    /// carried onto the current one rather than left on an image nobody
    /// maintains any more.
    ///
    /// Absent on sessions created before versions were tracked, which is read
    /// as "not the current one" — the safe direction, since the alternative is
    /// leaving a session on an unknown image forever.
    #[serde(default)]
    pub runtime_version: Option<String>,
    /// How many times this session has been carried onto a newer runtime.
    #[serde(default)]
    pub rotations: u32,
    /// When the snapshot the current runtime resumed from was taken.
    ///
    /// Part of the session rather than bookkeeping about its runtime, because
    /// it is something its owner is told: it says the point their work was
    /// carried across from, so anything after it can be recognised as missing
    /// rather than puzzled over.
    #[serde(default)]
    pub rotated_at: Option<String>,
    /// What this session was granted when it was created.
    ///
    /// Held with the session for the same reason its options are: it decides
    /// what the session can be asked to do, and recomputing it later against a
    /// runtime whose abilities have since changed would silently make a live
    /// session into a different one.
    ///
    /// Empty on sessions recorded before this existed, and settled the first
    /// time such a session is placed or picked back up — `settle_capabilities`
    /// derives it against the runtime it actually has and writes it down.
    ///
    /// The empty state is therefore transient rather than safe. It was
    /// described as safe on the assumption that a session holding nothing
    /// simply claims nothing; in fact every gate that consults the set refuses,
    /// including the one behind each keystroke, so a session left empty is
    /// listed, openable and dead.
    #[serde(default)]
    pub capabilities: crate::capabilities::SessionCapabilities,
    /// What this session's credential may reach, decided when it was created.
    ///
    /// Held with the session rather than resolved from its profile whenever
    /// somebody asks. A profile can be edited after a session started from it,
    /// and the session keeps the scope it was provisioned with — so reading
    /// the profile to learn a session's reach gives the wrong answer precisely
    /// when it matters, which is after somebody has widened or narrowed the
    /// bundle a live session was started from.
    ///
    /// `None` on sessions recorded before this existed, which is not the same
    /// fact as an empty grant: the first is "we did not write it down" and the
    /// second is "this session may reach nothing". They are told apart because
    /// the first still has to fall back to its profile, and the second must
    /// not.
    #[serde(default)]
    pub granted_services: Option<Vec<crate::environment::ServiceId>>,
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
            options: AgentOptions::default(),
            environment_profile: None,
            geometry: TerminalGeometry::default(),
            last_seq: 0,
            runtime_version: None,
            rotations: 0,
            rotated_at: None,
            capabilities: Default::default(),
            granted_services: None,
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        assert!(session.is_owned_by(&owner));
        assert!(!session.is_owned_by(&SubjectId("subject-b".into())));
    }
}
