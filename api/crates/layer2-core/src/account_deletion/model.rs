use super::*;
use crate::{rfc3339_at, seconds_from_rfc3339, SubjectId};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    time::{Duration, UNIX_EPOCH},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountDeletion {
    pub id: AccountDeletionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<SubjectId>,
    pub subject_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_subject_hash: Option<String>,
    pub status: AccountDeletionStatus,
    pub step: ErasureStep,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<ErasureBlock>,
    pub requested_at: String,
    pub deadline_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expire_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverify_at: Option<String>,
    pub work: ErasureWork,
    pub inventory: ErasureInventory,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub residue: Option<ResidueReport>,
    pub revision: u64,
    pub event_seq: u64,
    pub steps: Vec<StepProgress>,
}
impl AccountDeletion {
    pub fn request(
        id: AccountDeletionId,
        subject: SubjectId,
        at: &str,
    ) -> Result<Self, AccountDeletionError> {
        if subject.as_str().trim().is_empty()
            || !crate::document_artifacts::valid_id(id.as_str(), "acd_")
        {
            return Err(AccountDeletionError::Invalid);
        }
        let hash = subject_hash(&subject);
        Ok(Self {
            id,
            subject: Some(subject),
            subject_hash: hash.clone(),
            active_subject_hash: Some(hash),
            status: AccountDeletionStatus::Running,
            step: ErasureStep::Inventory,
            block: None,
            requested_at: at.into(),
            deadline_at: later(at, Duration::from_secs(7 * 86400))?,
            completed_at: None,
            expire_at: None,
            reverify_at: None,
            work: ErasureWork {
                ready_at: at.into(),
                attempts: 0,
                next_fence: 1,
                lease: None,
            },
            inventory: ErasureInventory::default(),
            residue: None,
            revision: 1,
            event_seq: 1,
            steps: vec![],
        })
    }
    pub fn is_active(&self) -> bool {
        self.status != AccountDeletionStatus::Completed
    }
    pub fn is_due(&self, now: &str) -> bool {
        self.status == AccountDeletionStatus::Running
            && self.work.ready_at.as_str() <= now
            && self
                .work
                .lease
                .as_ref()
                .is_none_or(|lease| lease.expires_at.as_str() <= now)
    }
    pub fn claim(
        &mut self,
        holder: &str,
        now: &str,
        lease: Duration,
    ) -> Result<u64, AccountDeletionError> {
        if holder.trim().is_empty() || lease.is_zero() || !self.is_due(now) {
            return Err(AccountDeletionError::Invalid);
        }
        let expires_at = later(now, lease)?;
        if expires_at.as_str() <= now {
            return Err(AccountDeletionError::Invalid);
        }
        let fence = self.work.next_fence;
        let next = fence.checked_add(1).ok_or(AccountDeletionError::Invalid)?;
        self.bump()?;
        self.work.next_fence = next;
        self.work.lease = Some(ErasureLease {
            holder: holder.into(),
            fence,
            token: uuid::Uuid::new_v4().to_string(),
            expires_at,
        });
        Ok(fence)
    }
    pub fn advance(
        &mut self,
        fence: u64,
        at: &str,
        counts: BTreeMap<String, u64>,
    ) -> Result<ErasureEvent, AccountDeletionError> {
        self.require_fence(fence, at)?;
        let index = ERASURE_ORDER
            .iter()
            .position(|step| *step == self.step)
            .ok_or(AccountDeletionError::Invalid)?;
        let next = ERASURE_ORDER
            .get(index + 1)
            .copied()
            .ok_or(AccountDeletionError::Invalid)?;
        let event = self.event(
            at,
            ErasureEventKind::Completed {
                counts: counts.clone(),
            },
        )?;
        self.steps.push(StepProgress {
            step: self.step,
            completed_at: at.into(),
            counts,
        });
        self.step = next;
        self.schedule(at);
        Ok(event)
    }
    pub fn renew(
        &mut self,
        fence: u64,
        now: &str,
        lease: Duration,
    ) -> Result<(), AccountDeletionError> {
        self.require_fence(fence, now)?;
        if lease.is_zero() {
            return Err(AccountDeletionError::Invalid);
        }
        let expires_at = later(now, lease)?;
        if expires_at.as_str() <= now {
            return Err(AccountDeletionError::Invalid);
        }
        self.revise()?;
        self.work.lease.as_mut().unwrap().expires_at = expires_at;
        Ok(())
    }
    pub fn defer(&mut self, fence: u64, until: &str) -> Result<(), AccountDeletionError> {
        if self.status != AccountDeletionStatus::Running
            || self
                .work
                .lease
                .as_ref()
                .is_none_or(|lease| lease.fence != fence)
        {
            return Err(AccountDeletionError::StaleFence);
        }
        let ready_at = later(until, Duration::ZERO)?;
        self.revise()?;
        self.work.ready_at = ready_at;
        self.work.lease = None;
        Ok(())
    }
    pub fn retry(
        &mut self,
        fence: u64,
        at: &str,
        failure: ErasureFailureCode,
    ) -> Result<ErasureEvent, AccountDeletionError> {
        self.require_fence(fence, at)?;
        let attempts = self
            .work
            .attempts
            .checked_add(1)
            .ok_or(AccountDeletionError::Invalid)?;
        let ready_at = later(at, Duration::from_secs((1_u64 << attempts.min(8)).min(300)))?;
        let event = self.event(at, ErasureEventKind::Retried { failure })?;
        self.work.attempts = attempts;
        self.work.ready_at = ready_at;
        self.work.lease = None;
        Ok(event)
    }
    pub fn block(
        &mut self,
        fence: u64,
        at: &str,
        reason: ErasureBlock,
    ) -> Result<ErasureEvent, AccountDeletionError> {
        self.require_fence(fence, at)?;
        let event = self.event(
            at,
            ErasureEventKind::Blocked {
                reason: reason.clone(),
            },
        )?;
        self.status = AccountDeletionStatus::Blocked;
        self.block = Some(reason);
        self.work.lease = None;
        Ok(event)
    }
    pub fn rearm(&mut self, at: &str) -> Result<ErasureEvent, AccountDeletionError> {
        if self.status != AccountDeletionStatus::Blocked {
            return Err(AccountDeletionError::Invalid);
        }
        later(at, Duration::ZERO)?;
        let event = self.event(at, ErasureEventKind::Rearmed)?;
        self.status = AccountDeletionStatus::Running;
        self.block = None;
        self.schedule(at);
        Ok(event)
    }
    pub fn complete(
        &mut self,
        fence: u64,
        at: &str,
        tombstone_retention: Duration,
    ) -> Result<ErasureEvent, AccountDeletionError> {
        self.require_fence(fence, at)?;
        if self.step != ErasureStep::VerifyNoResidue
            || !self.residue.as_ref().is_some_and(ResidueReport::is_zero)
            || tombstone_retention.is_zero()
        {
            return Err(AccountDeletionError::Invalid);
        }
        let expiry = later(at, tombstone_retention)?;
        let reverify_at = later(at, Duration::from_secs(86400))?;
        let event = self.event(
            at,
            ErasureEventKind::Completed {
                counts: BTreeMap::new(),
            },
        )?;
        self.steps.push(StepProgress {
            step: ErasureStep::VerifyNoResidue,
            completed_at: at.into(),
            counts: BTreeMap::new(),
        });
        // Pending retention policy: retain aggregate counts, never the resource
        // identities or credential identifiers captured before erasure.
        self.inventory = ErasureInventory {
            counts: std::mem::take(&mut self.inventory.counts),
            ..ErasureInventory::default()
        };
        self.status = AccountDeletionStatus::Completed;
        self.subject = None;
        self.active_subject_hash = None;
        self.block = None;
        self.completed_at = Some(at.into());
        self.expire_at = Some(expiry);
        self.reverify_at = Some(reverify_at);
        self.work.lease = None;
        Ok(event)
    }
    pub fn receipt(&self) -> Receipt {
        Receipt {
            id: self.id.clone(),
            status: self.status,
            step: self.step,
            requested_at: self.requested_at.clone(),
            deadline_at: self.deadline_at.clone(),
            completed_at: self.completed_at.clone(),
            steps: self.steps.clone(),
        }
    }
    pub fn current_event(&self, at: &str, kind: ErasureEventKind) -> ErasureEvent {
        let kind = match kind {
            ErasureEventKind::Blocked { reason } => ErasureEventKind::Blocked {
                reason: reason.for_audit(),
            },
            other => other,
        };
        ErasureEvent {
            deletion_id: self.id.clone(),
            seq: self.event_seq,
            at: at.into(),
            step: self.step,
            kind,
        }
    }
    fn require_fence(&self, fence: u64, at: &str) -> Result<(), AccountDeletionError> {
        if self.status != AccountDeletionStatus::Running
            || !self
                .work
                .lease
                .as_ref()
                .is_some_and(|lease| lease.fence == fence && lease.expires_at.as_str() > at)
        {
            return Err(AccountDeletionError::StaleFence);
        }
        later(at, Duration::ZERO)?;
        Ok(())
    }
    fn event(
        &mut self,
        at: &str,
        kind: ErasureEventKind,
    ) -> Result<ErasureEvent, AccountDeletionError> {
        self.bump()?;
        Ok(self.current_event(at, kind))
    }
    fn bump(&mut self) -> Result<(), AccountDeletionError> {
        let seq = self
            .event_seq
            .checked_add(1)
            .ok_or(AccountDeletionError::Invalid)?;
        self.revise()?;
        self.event_seq = seq;
        Ok(())
    }
    fn revise(&mut self) -> Result<(), AccountDeletionError> {
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or(AccountDeletionError::Invalid)?;
        Ok(())
    }
    fn schedule(&mut self, at: &str) {
        self.work.ready_at = at.into();
        self.work.attempts = 0;
        self.work.lease = None;
    }
}
fn later(at: &str, duration: Duration) -> Result<String, AccountDeletionError> {
    let seconds = seconds_from_rfc3339(at)
        .and_then(|value| value.checked_add(duration.as_secs()))
        .ok_or(AccountDeletionError::Invalid)?;
    let instant = UNIX_EPOCH
        .checked_add(Duration::from_secs(seconds))
        .ok_or(AccountDeletionError::Invalid)?;
    Ok(rfc3339_at(instant))
}
