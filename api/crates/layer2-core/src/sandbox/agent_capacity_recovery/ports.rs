//! Local S5 ports. Mongo adapters consume S1 capture and S4 prepare/persist in
//! one existing transaction. Refusal is the only permitted unimplemented path.
use async_trait::async_trait;

use super::activation::RecoveryActivation;
use super::state::{RecoveryClaim, RecoveryConfirmation, RecoveryRequest, RecoveryWork};
use super::{CapacityRecoveryContext, CapacityRecoveryDisposition};
use crate::sandbox::{Agent, AgentLogEntry, Sandbox, Workspace, Worktree};
use crate::Caller;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RecoveryError {
    #[error("capacity recovery is disabled")]
    Disabled,
    #[error("capacity recovery dependency is unavailable")]
    Unavailable,
    #[error("capacity recovery is not authorized")]
    Unauthorized,
    #[error("capacity recovery authority changed")]
    Conflict,
    #[error("capacity recovery evidence is invalid")]
    InvalidEvidence,
    #[error("capacity recovery disposition: {0:?}")]
    Disposition(CapacityRecoveryDisposition),
}

/// Captured proof remains opaque and belongs to the store/session. The service
/// cannot manufacture a successful source proof from these decoded records.
pub struct RecoverySnapshot<P> {
    pub proof: P,
    pub agent: Agent,
    pub sandbox: Sandbox,
    pub workspace: Workspace,
    pub worktree: Worktree,
    pub original_failure: AgentLogEntry,
}

impl<P> RecoverySnapshot<P> {
    pub fn context(&self) -> CapacityRecoveryContext<'_> {
        CapacityRecoveryContext {
            agent: &self.agent,
            sandbox: &self.sandbox,
            workspace: &self.workspace,
            worktree: &self.worktree,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitOutcome<T> {
    Recorded(T),
    KnownAbort,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryLookup {
    /// Requires positive never-attempted/known-abort evidence. A missing
    /// publication under an uncertain commit MUST be Unknown instead.
    NotStarted,
    Recorded(Box<RecoveryWork>),
    Unknown,
}

pub enum ClaimOutcome {
    Claimed(Box<RecoveryClaim>),
    Waiting(Box<RecoveryWork>),
    Terminal(Box<RecoveryWork>),
    Unknown,
}

#[async_trait]
pub trait CapacityRecoveryStore: Send + Sync {
    type Proof: Send + Sync;

    /// Every call rechecks current owner/resource policy, including receipt reads.
    async fn lookup(
        &self,
        caller: &Caller,
        request: &RecoveryRequest,
    ) -> Result<RecoveryLookup, RecoveryError>;

    async fn capture(
        &self,
        caller: &Caller,
        request: &RecoveryRequest,
    ) -> Result<RecoverySnapshot<Self::Proof>, RecoveryError>;

    /// Revalidate capture, parent intent, original effects, both durable gates,
    /// lease expiry and cohort inside the transaction. Write non-ready work,
    /// seal/successor/projections/receipt and the single Agent CAS atomically.
    /// No implementation may substitute a standalone work-row insert.
    async fn admit(
        &self,
        caller: &Caller,
        snapshot: RecoverySnapshot<Self::Proof>,
        work: RecoveryWork,
        next_agent: Agent,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError>;

    /// Reconcile uncertain prior publication before claiming. A new holder
    /// advances the Agent's native writer fence with the recovery lease. Parent
    /// stop/delete/retirement supersedes before any runtime call. Admission
    /// gate closure does not block exact accepted work.
    async fn claim(
        &self,
        caller: &Caller,
        request: &RecoveryRequest,
        holder: &str,
        now: &str,
        expires_at: &str,
    ) -> Result<ClaimOutcome, RecoveryError>;

    /// Verify the current claim and live parents again; persist confirmed state,
    /// actual lifecycle log/projection and exact completion receipt via S4.
    /// This is also the durable output-recording boundary before acknowledgement.
    async fn complete(
        &self,
        caller: &Caller,
        claim: &RecoveryClaim,
        confirmation: &RecoveryConfirmation,
        now: &str,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError>;

    /// Re-read exact completed publication, current Agent/native/input identity
    /// and live parents. Absence or unresolved evidence never authorizes attach.
    async fn activation(
        &self,
        _caller: &Caller,
        _completed: &RecoveryWork,
    ) -> Result<RecoveryActivation, RecoveryError> {
        Err(RecoveryError::Unavailable)
    }

    async fn retry(
        &self,
        caller: &Caller,
        claim: &RecoveryClaim,
        now: &str,
        retry_at: &str,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError>;
}

/// A current process-local attachment is already published at the exact
/// committed identity/fence, or a newly retained connection needs activation.
/// AlreadyActivated never substitutes for the store's fresh receipt/authority
/// checks and carries no invented native confirmation.
pub enum CompletedAttachment<A> {
    AlreadyActivated,
    Reattached {
        attachment: A,
        confirmation: Box<RecoveryConfirmation>,
    },
}

/// Intentionally has no Message, Steer, settings/context replay, first-message,
/// open-new-conversation or runtime-replacement method. An implementation must
/// use the original open receipt and reconnect only the pinned native target.
#[async_trait]
pub trait CapacityRecoveryRuntime: Send + Sync {
    type Attachment: Send;

    async fn reattach_existing(
        &self,
        claim: &RecoveryClaim,
    ) -> Result<(Self::Attachment, RecoveryConfirmation), RecoveryError>;

    /// Reattach the exact committed native fence after a crash before activation.
    /// A provider must atomically replace an older connection for the same
    /// operation/fence, reject a newer fence, and retain unacknowledged output.
    /// No input/settings replay or new native process is permitted.
    async fn reattach_completed(
        &self,
        _activation: &RecoveryActivation,
    ) -> Result<CompletedAttachment<Self::Attachment>, RecoveryError> {
        Err(RecoveryError::Unavailable)
    }

    /// Called only after an exact durable completed receipt, never on Unknown.
    /// A dropped/unacknowledged attachment must leave output available to retry.
    async fn acknowledge(
        &self,
        attachment: Self::Attachment,
        completed: &RecoveryWork,
    ) -> Result<(), RecoveryError>;
}
