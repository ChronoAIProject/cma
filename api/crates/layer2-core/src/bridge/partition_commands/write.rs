use super::*;
use crate::bridge::{
    allocation::{
        AllocationAdmission, AllocationAuthority, AllocationReservation, AllocationState,
    },
    ingress::InboxAdmission,
    partition::{NormalizedPartitionSource, PartitionSourcePin},
};

/// Nonserializable seal for current owner/admin authority and original evidence.
pub struct PartitionCommandWrite {
    pub(super) manager: BridgeManagementLease,
    pub(super) request: PartitionCommandRequest,
    pub(super) receipt_id: String,
    pub(super) request_sha256: String,
    pub(super) snapshot: PartitionCommandSnapshot,
}
impl PartitionCommandWrite {
    pub fn management(&self) -> &BridgeManagementLease {
        &self.manager
    }
    pub fn request(&self) -> &PartitionCommandRequest {
        &self.request
    }
    pub fn receipt_id(&self) -> &str {
        &self.receipt_id
    }
    pub fn request_sha256(&self) -> &str {
        &self.request_sha256
    }
    pub fn snapshot(&self) -> &PartitionCommandSnapshot {
        &self.snapshot
    }
    pub fn require_current(&self) -> Result<(), PartitionCommandError> {
        self.manager.require_current()?;
        let previous = &self.snapshot.allocation;
        previous.validate()?;
        if previous.id != self.request.allocation_id
            || previous.revision != self.request.expected_revision
            || self.manager.record().revision != self.request.expected_bridge_revision
        {
            return Err(PartitionCommandError::Conflict);
        }
        validate_snapshot(&self.manager, &self.snapshot)?;
        Ok(())
    }
    pub fn admission(&self) -> Result<AllocationAdmission, PartitionCommandError> {
        self.require_current()?;
        let source =
            NormalizedPartitionSource::from_stored(&self.snapshot.source, self.manager.record())
                .map_err(|_| PartitionCommandError::Corrupt)?;
        Ok(
            AllocationAdmission::from_accepted(self.manager.record().clone(), &source)?
                .after_reset(&self.snapshot.allocation)?,
        )
    }
    pub fn reset(
        &self,
        authority: AllocationAuthority,
        now_ms: i64,
    ) -> Result<AllocationReservation, PartitionCommandError> {
        if self.request.action != PartitionCommandAction::Reset {
            return Err(PartitionCommandError::Invalid);
        }
        Ok(AllocationReservation::reset(
            self.admission()?,
            authority,
            self.snapshot.allocation.clone(),
            self.manager.same_request()?,
            now_ms,
        )?)
    }
    pub fn retired(&self, now_ms: i64) -> Result<AllocationRecord, PartitionCommandError> {
        self.require_current()?;
        let mut result = self.snapshot.allocation.clone();
        if now_ms < result.updated_at_ms
            || result
                .lease
                .as_ref()
                .is_some_and(|l| l.expires_at_ms > now_ms)
            || (result.effect_started && result.receipt.is_none())
            || (self.request.action == PartitionCommandAction::Detach
                && result.state == AllocationState::Retired)
        {
            return Err(PartitionCommandError::Conflict);
        }
        result.state = AllocationState::Retired;
        result.lease = None;
        result.revision = result
            .revision
            .checked_add(1)
            .ok_or(PartitionCommandError::Corrupt)?;
        result.updated_at_ms = now_ms;
        result.validate()?;
        Ok(result)
    }
    pub fn receipt(
        &self,
        retired: &AllocationRecord,
        successor: Option<&AllocationRecord>,
    ) -> Result<PartitionCommandReceipt, PartitionCommandError> {
        if self.retired(retired.updated_at_ms)? != *retired {
            return Err(PartitionCommandError::Corrupt);
        }
        let receipt = PartitionCommandReceipt {
            id: self.receipt_id.clone(),
            bridge_id: self.manager.record().id.clone(),
            request_sha256: self.request_sha256.clone(),
            actor: self.manager.actor().clone(),
            action: self.request.action,
            expected_bridge_revision: self.request.expected_bridge_revision,
            allocation_id: self.snapshot.allocation.id.clone(),
            generation: self.snapshot.allocation.identity.generation,
            previous_revision: self.snapshot.allocation.revision,
            result: retired.into(),
            successor: successor.map(Into::into),
            created_at_ms: retired.updated_at_ms,
        };
        receipt.validate()?;
        Ok(receipt)
    }
}

pub fn validate_snapshot(
    manager: &BridgeManagementLease,
    snapshot: &PartitionCommandSnapshot,
) -> Result<(), PartitionCommandError> {
    manager.require_current()?;
    let record = &snapshot.allocation;
    record.validate()?;
    if record.owner != manager.record().owner
        || record.identity.bridge_id != manager.record().id
        || manager.record().pinned(&record.pin)?.config != record.config
        || snapshot.source.owner != record.owner
        || snapshot.source.admission != InboxAdmission::Accepted
        || PartitionSourcePin::for_inbox(&snapshot.source)
            .map_err(|_| PartitionCommandError::Corrupt)?
            != record.first_source
    {
        return Err(PartitionCommandError::Corrupt);
    }
    let normalized = NormalizedPartitionSource::from_stored(&snapshot.source, manager.record())
        .map_err(|_| PartitionCommandError::Corrupt)?;
    match normalized
        .admit(&record.config)
        .map_err(|_| PartitionCommandError::Corrupt)?
    {
        crate::bridge::partition::PartitionAdmissionDecision::Admitted { key, .. }
            if key == record.identity.key =>
        {
            Ok(())
        }
        _ => Err(PartitionCommandError::Corrupt),
    }
}
