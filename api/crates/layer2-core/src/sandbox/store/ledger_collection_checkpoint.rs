//! B2's separately versioned checkpoint. Scheduling state never grants effects.
use super::{
    LedgerCollectionPageProgress, LedgerCollectionPending, LedgerCollectionPhase,
    LedgerCollectionProgressError, LedgerCollectorProgressV1,
};
use crate::sandbox::ledger_collection_artifact::LedgerCollectionArtifactReferenceV1;
use serde::{Deserialize, Serialize};

const FORMAT: &str = "runtime-ledger-collector-checkpoint-v2";
type Result<T> = std::result::Result<T, LedgerCollectionProgressError>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerCollectionCheckpointV2 {
    pub format: String,
    /// V1's shape is unchanged; only the enclosing V2 planner admits final ack.
    pub progress: LedgerCollectorProgressV1,
    pub settled_after: Option<String>,
    /// Exact originals live only in A's archive, including before Selected.
    pub accepted_artifact: Option<LedgerCollectionArtifactReferenceV1>,
    /// Revision from the verified cold final receipt, retained until settlement.
    pub acknowledged_revision: Option<u64>,
}

/// Internal projection after A's exact cold receipt bytes/checksum are verified.
/// This is neither an archive encoding nor an execution permit. The transport
/// must never construct it from unverified public JSON or a completion reply.
#[derive(Debug, Clone)]
pub struct LedgerCollectionFinalAcknowledgement {
    pub operation_id: String,
    pub binding_sha256: String,
    pub plan_sha256: String,
    pub group_key: String,
    pub source_sha256: String,
    pub observation_sha256: String,
    pub before_revision: u64,
    pub after_revision: u64,
    pub receipt_sha256: String,
}

#[derive(Debug, Clone)]
pub enum LedgerCollectionCheckpointMutation {
    Claim {
        holder_sha256: String,
        now: i64,
        expires_at: i64,
    },
    PinPage {
        fence: u64,
        now: i64,
        page: LedgerCollectionPageProgress,
    },
    RecordReference {
        fence: u64,
        now: i64,
        reference: LedgerCollectionArtifactReferenceV1,
    },
    Select {
        fence: u64,
        now: i64,
        pending: LedgerCollectionPending,
    },
    Acknowledge {
        fence: u64,
        now: i64,
        acknowledgement: LedgerCollectionFinalAcknowledgement,
    },
    /// Caller re-read the cold receipt and supplies its unchanged exact hash.
    SettleAcknowledged {
        fence: u64,
        now: i64,
        operation_id: String,
        receipt_sha256: String,
    },
    /// Caller verified a RetainLive/RetainUnknown observation: no source effects.
    SelectRetained {
        fence: u64,
        now: i64,
        pending: LedgerCollectionPending,
    },
    SettleRetained {
        fence: u64,
        now: i64,
        operation_id: String,
    },
    Advance {
        fence: u64,
        now: i64,
        active_revision: u64,
        page_sha256: String,
    },
    Refresh {
        fence: u64,
        now: i64,
        active_revision: u64,
    },
}

impl LedgerCollectionCheckpointV2 {
    pub fn new(
        binding_sha256: String,
        maintenance_epoch: u64,
        active_revision: u64,
        holder_sha256: String,
        now: i64,
        expires_at: i64,
    ) -> Result<Self> {
        Ok(Self {
            format: FORMAT.into(),
            progress: LedgerCollectorProgressV1::new(
                binding_sha256,
                maintenance_epoch,
                active_revision,
                holder_sha256,
                now,
                expires_at,
            )?,
            settled_after: None,
            accepted_artifact: None,
            acknowledged_revision: None,
        })
    }

    pub fn is_initial(&self) -> bool {
        let p = &self.progress;
        self.validate()
            && p.revision == 1
            && p.lease.fence == 1
            && p.sweep == 1
            && p.page.is_none()
            && p.pending.is_none()
            && p.cursor.is_none()
            && p.last_ack_sha256.is_none()
            && !p.complete
            && self.settled_after.is_none()
            && self.accepted_artifact.is_none()
            && self.acknowledged_revision.is_none()
    }

    pub fn validate(&self) -> bool {
        let p = &self.progress;
        self.format == FORMAT
            && p.validate()
            && self.settled_after.as_deref().is_none_or(digest)
            && p.cursor.as_deref().is_none_or(cursor)
            && p.page.as_ref().is_none_or(|page| {
                page.next_cursor.as_deref().is_none_or(cursor)
                    && page.remaining_groups.first().is_none_or(|first| {
                        self.settled_after
                            .as_ref()
                            .is_none_or(|after| first > after)
                    })
            })
            && (!p.complete || (p.cursor.is_none() && self.accepted_artifact.is_none()))
            && self.accepted_artifact.as_ref().is_none_or(|r| {
                r.validate()
                    && r.binding_sha256 == p.binding_sha256
                    && self.first_group() == Some(r.group_key.as_str())
                    && p.pending.as_ref().is_none_or(|pending| {
                        r.operation_id == pending.operation_id && r.group_key == pending.group_key
                    })
            })
            && p.pending.as_ref().is_none_or(|pending| {
                self.first_group() == Some(pending.group_key.as_str())
                    && match pending.phase {
                        LedgerCollectionPhase::Selected | LedgerCollectionPhase::Acknowledged => {
                            self.accepted_artifact.is_some()
                        }
                        LedgerCollectionPhase::Retained => self.accepted_artifact.is_none(),
                        _ => false,
                    }
            })
            && match (&p.pending, self.acknowledged_revision) {
                (Some(pending), Some(revision)) => {
                    pending.phase == LedgerCollectionPhase::Acknowledged
                        && p.active_revision.checked_add(1) == Some(revision)
                }
                (Some(pending), None) => pending.phase != LedgerCollectionPhase::Acknowledged,
                (None, revision) => revision.is_none(),
            }
            && serde_json::to_vec(self).is_ok_and(|bytes| bytes.len() <= 32 * 1024)
    }

    fn first_group(&self) -> Option<&str> {
        self.progress
            .page
            .as_ref()?
            .remaining_groups
            .first()
            .map(String::as_str)
    }

    fn require_claim(&self, fence: u64, now: i64) -> Result<()> {
        if !self.validate() {
            return Err(LedgerCollectionProgressError::Invalid);
        }
        if fence != self.progress.lease.fence || now < 0 || now >= self.progress.lease.expires_at {
            return Err(LedgerCollectionProgressError::StaleClaim);
        }
        Ok(())
    }

    fn bump(&self) -> Result<Self> {
        let mut next = self.clone();
        next.progress.revision = self
            .progress
            .revision
            .checked_add(1)
            .ok_or(LedgerCollectionProgressError::Invalid)?;
        Ok(next)
    }

    fn checked(self) -> Result<Self> {
        if self.validate() {
            Ok(self)
        } else {
            Err(LedgerCollectionProgressError::Invalid)
        }
    }

    fn idle(&self) -> Result<()> {
        if self.progress.pending.is_some() || self.accepted_artifact.is_some() {
            return Err(LedgerCollectionProgressError::PendingTransition);
        }
        Ok(())
    }
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn cursor(value: &str) -> bool {
    !value.is_empty() && value.len() <= 4096 && !value.chars().any(char::is_control)
}

#[async_trait::async_trait]
pub trait LedgerCollectionCheckpointStore: Send + Sync {
    async fn ledger_collection_checkpoint(
        &self,
        sandbox: &crate::SandboxId,
    ) -> std::result::Result<Option<LedgerCollectionCheckpointV2>, super::super::SandboxStoreError>;
    /// No implicit conversion of a B1 marker, even when it looks idle.
    async fn retain_ledger_collection_checkpoint(
        &self,
        sandbox: &crate::SandboxId,
        expected_sandbox_revision: u64,
        initial: LedgerCollectionCheckpointV2,
    ) -> std::result::Result<(), super::super::SandboxStoreError>;
    async fn update_ledger_collection_checkpoint(
        &self,
        sandbox: &crate::SandboxId,
        expected_sandbox_revision: u64,
        expected_progress_revision: u64,
        mutation: LedgerCollectionCheckpointMutation,
    ) -> std::result::Result<LedgerCollectionCheckpointV2, super::super::SandboxStoreError>;
}

#[cfg(test)]
#[path = "ledger_collection_checkpoint_tests.rs"]
mod tests;
#[path = "ledger_collection_checkpoint_transitions.rs"]
mod transitions;
