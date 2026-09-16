use std::time::Duration;

use crate::resource_access::{CallerContext, ResourceActor};

use super::{
    authority::BridgeAuthority, service::require_issuer, BridgeError, BridgeId, BridgeRecord,
    BridgeService,
};

/// Request-local management proof for secret-custody adapters. This cannot be
/// deserialized or minted from a receipt. The original human/API actor and
/// current organization deadline survive subsequent provider and storage IO.
pub struct BridgeManagementLease {
    record: BridgeRecord,
    actor: ResourceActor,
    authority: BridgeAuthority,
    deadline: tokio::time::Instant,
}

impl BridgeManagementLease {
    /// Share the same request-local proof across atomic command components.
    /// This never refreshes its authority or deadline.
    pub(super) fn same_request(&self) -> Result<Self, BridgeError> {
        self.require_current()?;
        Ok(Self {
            record: self.record.clone(),
            actor: self.actor.clone(),
            authority: BridgeAuthority {
                can_manage: self.authority.can_manage,
                deadline: self.authority.deadline,
            },
            deadline: self.deadline,
        })
    }
    pub fn record(&self) -> &BridgeRecord {
        &self.record
    }
    pub fn actor(&self) -> &ResourceActor {
        &self.actor
    }
    pub fn deadline(&self) -> tokio::time::Instant {
        self.deadline
    }
    /// A custody/provider observation can only shorten this request's authority.
    pub fn bound_deadline(&mut self, deadline: tokio::time::Instant) {
        self.deadline = self.deadline.min(deadline);
    }
    pub fn require_current(&self) -> Result<(), BridgeError> {
        self.authority.require(true)?;
        if tokio::time::Instant::now() >= self.deadline {
            return Err(BridgeError::AuthorityUnavailable);
        }
        Ok(())
    }
}

impl BridgeService {
    pub async fn management_lease(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &BridgeId,
        expected_revision: u64,
    ) -> Result<BridgeManagementLease, BridgeError> {
        let started = tokio::time::Instant::now();
        let record = self.store.bridge(id).await?.ok_or(BridgeError::NotFound)?;
        require_issuer(&record, context)?;
        let authority = self
            .authority(context, credential, &record.owner, true)
            .await?;
        if record.revision != expected_revision {
            return Err(BridgeError::Conflict);
        }
        let deadline = authority
            .deadline
            .unwrap_or(started + Duration::from_secs(15))
            .min(started + Duration::from_secs(15));
        let lease = BridgeManagementLease {
            record,
            actor: context.actor.clone(),
            authority,
            deadline,
        };
        self.validate_management(&lease).await?;
        Ok(lease)
    }

    pub async fn validate_management(
        &self,
        lease: &BridgeManagementLease,
    ) -> Result<(), BridgeError> {
        lease.require_current()?;
        let current = self
            .store
            .bridge(&lease.record.id)
            .await?
            .ok_or(BridgeError::NotFound)?;
        if current != lease.record {
            return Err(BridgeError::Conflict);
        }
        lease.require_current()
    }
}
