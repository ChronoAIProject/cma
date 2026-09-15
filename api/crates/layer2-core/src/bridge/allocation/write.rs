use crate::bridge::BridgeManagementLease;
use crate::resource_access::ResourceOwner;

use super::{
    AllocationAdmission, AllocationAuthority, AllocationError, AllocationLease,
    AllocationPredecessor, AllocationRecord, AllocationResourceReceipt, AllocationState,
};

pub struct AllocationReservation {
    record: AllocationRecord,
    authority: AllocationAuthority,
    reset: Option<(AllocationRecord, BridgeManagementLease)>,
}

impl AllocationReservation {
    pub fn new(
        admission: AllocationAdmission,
        authority: AllocationAuthority,
        now_ms: i64,
    ) -> Result<Self, AllocationError> {
        if admission.identity.generation != 1 {
            return Err(AllocationError::ResetRequired);
        }
        Self::build(admission, authority, None, now_ms)
    }
    pub fn reset(
        admission: AllocationAdmission,
        authority: AllocationAuthority,
        previous: AllocationRecord,
        management: BridgeManagementLease,
        now_ms: i64,
    ) -> Result<Self, AllocationError> {
        previous.validate()?;
        management
            .require_current()
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        if management.record() != &admission.bridge
            || previous.owner != admission.bridge.owner
            || previous.identity.bridge_id != admission.identity.bridge_id
            || previous.identity.key != admission.identity.key
            || previous.identity.generation + 1 != admission.identity.generation
            || now_ms < previous.updated_at_ms
            || previous
                .lease
                .as_ref()
                .is_some_and(|lease| lease.expires_at_ms > now_ms)
            || (previous.effect_started && previous.receipt.is_none())
        {
            return Err(AllocationError::ResetRequired);
        }
        Self::build(admission, authority, Some((previous, management)), now_ms)
    }
    fn build(
        admission: AllocationAdmission,
        authority: AllocationAuthority,
        reset: Option<(AllocationRecord, BridgeManagementLease)>,
        now_ms: i64,
    ) -> Result<Self, AllocationError> {
        authority.require_snapshot(&admission.bridge)?;
        let id = admission.identity.id()?;
        let mut record = AllocationRecord {
            operation_id: format!("{id}:provision:v1"),
            placement: authority.placement(&id)?,
            id,
            identity: admission.identity,
            owner: admission.bridge.owner.clone(),
            pin: admission.source.work_pin().clone(),
            config: admission
                .bridge
                .pinned(admission.source.work_pin())
                .map_err(|_| AllocationError::Corrupt)?
                .config
                .clone(),
            first_source: admission.source,
            predecessor: reset
                .as_ref()
                .map(|(previous, management)| AllocationPredecessor {
                    id: previous.id.clone(),
                    generation: previous.identity.generation,
                    reset_by: management.actor().clone(),
                }),
            state: AllocationState::Reserved,
            revision: 1,
            lease_fence: 0,
            lease: None,
            effect_started: false,
            receipt: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };
        if let AllocationAuthority::Existing(proof) = &authority {
            record.state = AllocationState::Ready;
            record.effect_started = true;
            record.receipt = Some(AllocationResourceReceipt {
                operation_id: record.operation_id.clone(),
                target: record.placement.target().clone(),
                receipt_digest: super::hash(&(
                    "cma.bridge.fixed-binding.v1",
                    &proof.grant().target,
                ))?,
                profiles_digest: None,
            });
        }
        record.validate()?;
        authority.require_record(&record)?;
        Ok(Self {
            record,
            authority,
            reset,
        })
    }
    pub fn record(&self) -> &AllocationRecord {
        &self.record
    }
    pub fn authority(&self) -> &AllocationAuthority {
        &self.authority
    }
    pub fn previous(&self) -> Option<&AllocationRecord> {
        self.reset.as_ref().map(|(record, _)| record)
    }
    pub fn require_current(&self) -> Result<(), AllocationError> {
        self.authority.require_record(&self.record)?;
        if let Some((_, management)) = &self.reset {
            management
                .require_current()
                .map_err(|_| AllocationError::AuthorityUnavailable)?;
        }
        Ok(())
    }
    pub fn require_at(&self, now_ms: i64) -> Result<(), AllocationError> {
        self.require_current()?;
        if self.record.created_at_ms > now_ms
            || self.previous().is_some_and(|previous| {
                previous
                    .lease
                    .as_ref()
                    .is_some_and(|lease| lease.expires_at_ms > now_ms)
            })
        {
            return Err(AllocationError::LeaseUnavailable);
        }
        Ok(())
    }
}

/// A trusted provider/resource-store reconciliation observation. It is accepted
/// only for the retained operation, owner and exact reserved resource IDs.
pub struct AllocationResourceObservation {
    pub owner: ResourceOwner,
    pub operation_id: String,
    pub target: crate::bridge::BridgeAgentTarget,
    pub receipt_digest: String,
    pub profiles_digest: Option<String>,
}

pub enum AllocationProgress {
    Start,
    Confirm(AllocationResourceObservation),
    NeedsConnection,
    FailedRetryable,
    FailedTerminal,
}

pub struct AllocationMutation {
    previous: AllocationRecord,
    record: AllocationRecord,
    authority: AllocationAuthority,
}

impl AllocationMutation {
    pub fn claim(
        previous: AllocationRecord,
        authority: AllocationAuthority,
        holder: &str,
        now_ms: i64,
        lease_ms: i64,
    ) -> Result<Self, AllocationError> {
        authority.require_record(&previous)?;
        previous.validate()?;
        if !super::super::config::valid_id(holder)
            || !(1..=60_000).contains(&lease_ms)
            || now_ms < previous.updated_at_ms
            || previous
                .lease
                .as_ref()
                .is_some_and(|lease| lease.expires_at_ms > now_ms)
            || matches!(
                previous.state,
                AllocationState::Ready | AllocationState::FailedTerminal | AllocationState::Retired
            )
        {
            return Err(AllocationError::LeaseUnavailable);
        }
        let mut record = previous.clone();
        record.lease_fence = record
            .lease_fence
            .checked_add(1)
            .ok_or(AllocationError::Corrupt)?;
        record.lease = Some(AllocationLease {
            holder: holder.to_owned(),
            fence: record.lease_fence,
            expires_at_ms: now_ms
                .checked_add(lease_ms)
                .ok_or(AllocationError::Corrupt)?,
        });
        Self::build(previous, record, authority, now_ms)
    }

    pub fn progress(
        previous: AllocationRecord,
        authority: AllocationAuthority,
        lease: &AllocationLease,
        progress: AllocationProgress,
        now_ms: i64,
    ) -> Result<Self, AllocationError> {
        previous.validate()?;
        authority.require_record(&previous)?;
        if previous.lease.as_ref() != Some(lease)
            || lease.expires_at_ms <= now_ms
            || now_ms < previous.updated_at_ms
        {
            return Err(AllocationError::LeaseUnavailable);
        }
        if matches!(
            previous.state,
            AllocationState::Ready | AllocationState::FailedTerminal | AllocationState::Retired
        ) {
            return Err(AllocationError::InvalidTransition);
        }
        let mut record = previous.clone();
        match progress {
            AllocationProgress::Start => {
                // A retry or takeover after effect start must reconcile; Start
                // cannot grant another provider create or change identities.
                if record.effect_started {
                    return Err(AllocationError::InvalidTransition);
                }
                record.effect_started = true;
                record.state = AllocationState::Provisioning;
            }
            AllocationProgress::Confirm(observation) => {
                if !record.effect_started
                    || observation.owner != record.owner
                    || observation.operation_id != record.operation_id
                    || &observation.target != record.placement.target()
                    || !super::super::partition::digest(&observation.receipt_digest)
                    || observation.profiles_digest.as_deref() != record.profiles_digest()
                {
                    return Err(AllocationError::IdentityMismatch);
                }
                record.receipt = Some(AllocationResourceReceipt {
                    operation_id: observation.operation_id,
                    target: observation.target,
                    receipt_digest: observation.receipt_digest,
                    profiles_digest: observation.profiles_digest,
                });
                record.state = AllocationState::Ready;
                record.lease = None;
            }
            AllocationProgress::NeedsConnection => {
                record.state = AllocationState::NeedsConnection;
                record.lease = None;
            }
            AllocationProgress::FailedRetryable => {
                record.state = AllocationState::FailedRetryable;
                record.lease = None;
            }
            AllocationProgress::FailedTerminal => {
                record.state = AllocationState::FailedTerminal;
                record.lease = None;
            }
        }
        Self::build(previous, record, authority, now_ms)
    }

    fn build(
        previous: AllocationRecord,
        mut record: AllocationRecord,
        authority: AllocationAuthority,
        now_ms: i64,
    ) -> Result<Self, AllocationError> {
        record.revision = record
            .revision
            .checked_add(1)
            .ok_or(AllocationError::Corrupt)?;
        record.updated_at_ms = now_ms;
        record.validate()?;
        Ok(Self {
            previous,
            record,
            authority,
        })
    }
    pub fn previous(&self) -> &AllocationRecord {
        &self.previous
    }
    pub fn record(&self) -> &AllocationRecord {
        &self.record
    }
    pub fn authority(&self) -> &AllocationAuthority {
        &self.authority
    }
    pub fn require_current(&self) -> Result<(), AllocationError> {
        self.authority.require_record(&self.record)
    }
    /// Storage checks wall-clock lease validity again after IO and before
    /// commit. A valid original authority deadline cannot extend a work lease.
    pub fn require_at(&self, now_ms: i64) -> Result<(), AllocationError> {
        self.require_current()?;
        let lease = if self.record.lease_fence > self.previous.lease_fence {
            self.record.lease.as_ref()
        } else {
            self.previous.lease.as_ref()
        };
        if self.record.updated_at_ms > now_ms
            || lease.is_none_or(|lease| lease.expires_at_ms <= now_ms)
        {
            return Err(AllocationError::LeaseUnavailable);
        }
        Ok(())
    }
}
