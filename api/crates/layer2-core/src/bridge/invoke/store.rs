use crate::resource_access::ResourceAccessSnapshot;

use super::super::{BridgeCustodyPin, BridgeId, BridgeManagementLease};
use super::{BridgeInvokeError, BridgeInvokeGrant, BridgeInvokeGrantId, BridgeInvokeReceipt};

/// Store-bound management proof. Grant writes consume actual current management
/// evidence; historical audit and JSON records cannot construct this type.
pub struct BridgeInvokeWrite {
    pub(super) management: BridgeManagementLease,
    pub(super) previous: Option<BridgeInvokeGrant>,
    pub(super) record: BridgeInvokeGrant,
    pub(super) receipt: BridgeInvokeReceipt,
    pub(super) resources: Option<ResourceAccessSnapshot>,
    pub(super) custody: Option<BridgeCustodyPin>,
    pub(super) deadline: tokio::time::Instant,
    pub(super) channel_authority: Option<crate::bridge::allocation::AllocationAuthority>,
    pub(super) channel_control_plane: Option<std::sync::Arc<dyn super::ChannelInvokeAuthority>>,
}

impl BridgeInvokeWrite {
    pub fn management(&self) -> &BridgeManagementLease {
        &self.management
    }
    pub fn previous(&self) -> Option<&BridgeInvokeGrant> {
        self.previous.as_ref()
    }
    pub fn record(&self) -> &BridgeInvokeGrant {
        &self.record
    }
    pub fn receipt(&self) -> &BridgeInvokeReceipt {
        &self.receipt
    }
    pub fn resources(&self) -> Option<&ResourceAccessSnapshot> {
        self.resources.as_ref()
    }
    pub fn custody_pin(&self) -> Option<&BridgeCustodyPin> {
        self.custody.as_ref()
    }
    pub fn channel_authority(&self) -> Option<&crate::bridge::allocation::AllocationAuthority> {
        self.channel_authority.as_ref()
    }
    pub fn channel_control_plane(&self) -> Option<&dyn super::ChannelInvokeAuthority> {
        self.channel_control_plane.as_deref()
    }

    pub fn require_current(&self) -> Result<(), BridgeInvokeError> {
        self.management.require_current()?;
        if tokio::time::Instant::now() >= self.deadline {
            return Err(BridgeInvokeError::Unavailable);
        }
        self.record.validate()?;
        self.receipt.validate()?;
        if self.record.owner != self.management.record().owner
            || self.record.pin.bridge_id != self.management.record().id
            || self.receipt.actor != *self.management.actor()
            || self.receipt.result != self.record.view()
        {
            return Err(BridgeInvokeError::Corrupt);
        }
        match &self.previous {
            None => {
                let resources = self.resources.as_ref().ok_or(BridgeInvokeError::Corrupt)?;
                let custody = self.custody.as_ref().ok_or(BridgeInvokeError::Corrupt)?;
                self.record.require_active()?;
                self.record.target.require(&self.record.owner, resources)?;
                if self.record.issued_by != *self.management.actor()
                    || custody.reference() != &self.record.config.connector.custody
                {
                    return Err(BridgeInvokeError::Corrupt);
                }
                match (&self.record.channel, &self.channel_authority) {
                    (None, None)
                        if self.record.config == self.management.record().current()?.config
                            && self.record.pin.config_revision
                                == self.management.record().config_revision
                            && self.channel_control_plane.is_none() => {}
                    (Some(basis), Some(authority)) => {
                        let control_plane = self
                            .channel_control_plane()
                            .ok_or(BridgeInvokeError::Corrupt)?;
                        control_plane.require_current()?;
                        if &control_plane.actor_context().actor != self.management.actor()
                            || self.deadline > control_plane.deadline()
                        {
                            return Err(BridgeInvokeError::Corrupt);
                        }
                        authority
                            .require_record(&basis.allocation)
                            .map_err(|_| BridgeInvokeError::Unavailable)?;
                        if authority.bridge() != self.management.record() {
                            return Err(BridgeInvokeError::Conflict);
                        }
                        basis.require_resources(resources)?;
                        basis.require_bridge(
                            self.management.record(),
                            &self.record.pin,
                            &self.record.config,
                        )?;
                    }
                    _ => return Err(BridgeInvokeError::Corrupt),
                }
            }
            Some(previous) => {
                previous.require_active()?;
                let mut revoked = previous.clone();
                revoked.revision = 2;
                revoked.state = self.record.state.clone();
                if revoked != self.record
                    || self.record.revision != 2
                    || !matches!(&self.record.state,
                        super::BridgeInvokeGrantState::Revoked { actor, .. }
                        if actor == self.management.actor())
                    || self.resources.is_some()
                    || self.custody.is_some()
                    || self.channel_authority.is_some()
                    || self.channel_control_plane.is_some()
                {
                    return Err(BridgeInvokeError::Corrupt);
                }
            }
        }
        Ok(())
    }
}

#[async_trait::async_trait]
pub trait BridgeInvokeStore: Send + Sync {
    /// Replay is still a background use of the renewable human grant. Fence
    /// its current exact binding, including replacement at the same key revision.
    async fn validate_channel_replay(
        &self,
        _grant: &BridgeInvokeGrant,
        _management: &BridgeManagementLease,
        _control_plane: &dyn super::ChannelInvokeAuthority,
    ) -> Result<(), BridgeInvokeError> {
        Err(BridgeInvokeError::Unavailable)
    }

    /// Trusted exact ready allocation/provenance read; unsupported stores cannot
    /// issue channel-created grants from caller-supplied attribution.
    async fn channel_target(
        &self,
        _allocation_id: &str,
    ) -> Result<super::ChannelInvokeSnapshot, BridgeInvokeError> {
        Err(BridgeInvokeError::Unavailable)
    }
    async fn grant(
        &self,
        id: &BridgeInvokeGrantId,
    ) -> Result<Option<BridgeInvokeGrant>, BridgeInvokeError>;
    async fn receipt(&self, id: &str) -> Result<Option<BridgeInvokeReceipt>, BridgeInvokeError>;
    /// Filter exact bridge before pagination; callers authorize its current owner.
    async fn list(
        &self,
        bridge: &BridgeId,
        after: Option<&BridgeInvokeGrantId>,
        limit: u32,
    ) -> Result<Vec<BridgeInvokeGrant>, BridgeInvokeError>;
    /// Persist grant CAS, immutable audit and idempotent receipt atomically with
    /// bridge/custody/resource fences. Never delete retained revocation history.
    async fn apply(
        &self,
        write: BridgeInvokeWrite,
    ) -> Result<BridgeInvokeReceipt, BridgeInvokeError>;
}

pub fn match_invoke_replay(
    stored: &BridgeInvokeReceipt,
    requested: &BridgeInvokeReceipt,
) -> Result<BridgeInvokeReceipt, BridgeInvokeError> {
    stored.validate()?;
    if stored.id != requested.id
        || stored.request_digest != requested.request_digest
        || stored.bridge_id != requested.bridge_id
        || stored.actor != requested.actor
        || stored.result.id != requested.result.id
        || stored.result.owner != requested.result.owner
        || stored.result.pin != requested.result.pin
        || stored.result.target != requested.result.target
        || stored.result.consent != requested.result.consent
        || stored.result.issued_by != requested.result.issued_by
        || stored.result.revision != requested.result.revision
    {
        return Err(BridgeInvokeError::IdempotencyConflict);
    }
    Ok(stored.clone())
}
