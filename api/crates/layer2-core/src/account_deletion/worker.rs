pub use super::worker_types::*;
use super::{AccountDeletion, AccountDeletionId, ErasureLease};
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalRequest {
    key: String,
    binding: JournalBinding,
    command: JournalCommand,
}

impl JournalRequest {
    pub fn new(binding: JournalBinding, command: JournalCommand) -> Result<Self, JournalError> {
        let key = super::worker_validation::key(&binding, &command)?;
        Ok(Self {
            key,
            binding,
            command,
        })
    }
    pub fn validate(&self) -> Result<(), JournalError> {
        if self.key != super::worker_validation::key(&self.binding, &self.command)? {
            return Err(JournalError::BindingMismatch);
        }
        Ok(())
    }
    pub fn key(&self) -> &str {
        &self.key
    }
    pub fn binding(&self) -> &JournalBinding {
        &self.binding
    }
    pub fn command(&self) -> &JournalCommand {
        &self.command
    }
}
impl std::fmt::Debug for JournalRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("JournalRequest([redacted])")
    }
}

#[derive(Clone)]
pub struct JournalClaim {
    checkpoint: JournalCheckpoint,
    lease: ErasureLease,
    next_fence: u64,
}
impl std::fmt::Debug for JournalClaim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("JournalClaim([redacted])")
    }
}
impl JournalClaim {
    /// Storage must still compare this complete handle with durable authority.
    pub fn from_record(
        checkpoint: JournalCheckpoint,
        parent: &AccountDeletion,
    ) -> Result<Self, JournalError> {
        let lease = parent.work.lease.clone().ok_or(JournalError::StaleClaim)?;
        if checkpoint.binding.deletion_id != parent.id
            || checkpoint.parent_revision != parent.revision
            || checkpoint.binding.step != parent.step
            || lease.token.is_empty()
            || lease.fence.checked_add(1) != Some(parent.work.next_fence)
        {
            return Err(JournalError::StaleClaim);
        }
        Ok(Self {
            checkpoint,
            lease,
            next_fence: parent.work.next_fence,
        })
    }
    pub fn checkpoint(&self) -> &JournalCheckpoint {
        &self.checkpoint
    }
    pub fn lease(&self) -> &ErasureLease {
        &self.lease
    }
    pub fn next_fence(&self) -> u64 {
        self.next_fence
    }
}

#[derive(Clone)]
pub struct JournalBudget {
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
}
impl JournalBudget {
    pub fn new(duration: Duration) -> Result<Self, JournalError> {
        if duration.is_zero() || duration > Duration::from_secs(30) {
            return Err(JournalError::Invalid);
        }
        Ok(Self {
            deadline: Instant::now() + duration,
            cancelled: Arc::new(AtomicBool::new(false)),
        })
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
    pub fn remaining(&self) -> Result<Duration, JournalError> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(JournalError::Cancelled);
        }
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or(JournalError::Cancelled)
    }
}

#[derive(Debug)]
pub struct JournalRecorded {
    pub receipt: JournalReceipt,
    pub claim: Option<JournalClaim>,
}

#[derive(Debug)]
pub enum JournalOutcome {
    Recorded(Box<JournalRecorded>),
    RecoveryPending { operation: String },
    Idle,
}
#[derive(Debug)]
pub enum JournalResolution {
    Committed(JournalReceipt),
    NotCommittedAfterFence {
        operation: String,
        claim: JournalClaim,
    },
    Unresolved,
}

#[async_trait::async_trait]
pub trait ErasureJournal: Send + Sync {
    async fn inspect(
        &self,
        id: &AccountDeletionId,
    ) -> Result<Option<JournalCheckpoint>, JournalError>;
    async fn enroll(
        &self,
        request: &JournalRequest,
        budget: &JournalBudget,
    ) -> Result<JournalOutcome, JournalError>;
    async fn claim_due(
        &self,
        request: &JournalRequest,
        budget: &JournalBudget,
    ) -> Result<JournalOutcome, JournalError>;
    async fn commit(
        &self,
        claim: &JournalClaim,
        request: &JournalRequest,
        budget: &JournalBudget,
    ) -> Result<JournalOutcome, JournalError>;
    async fn resolve(
        &self,
        request: &JournalRequest,
        fence: Option<&JournalClaim>,
        budget: &JournalBudget,
    ) -> Result<JournalResolution, JournalError>;
}
