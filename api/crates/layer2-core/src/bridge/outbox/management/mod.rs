//! Current owner/admin inspection and explicit recovery of original deliveries.
//! Public views exclude text, chat/thread routing and credential references.
mod service;
mod views;

use super::{OutboxError, OutboxItem};
use crate::bridge::{BridgeError, BridgeManagementLease, BridgeValidation};
pub use service::*;
pub use views::*;

#[derive(Debug, thiserror::Error)]
pub enum OutboxManagementError {
    #[error(transparent)]
    Bridge(#[from] BridgeError),
    #[error(transparent)]
    Delivery(#[from] OutboxError),
    #[error("delivery was not found")]
    NotFound,
    #[error("invalid delivery recovery request")]
    Invalid,
    #[error("duplicate delivery risk must be explicitly accepted")]
    DuplicateRiskRequired,
}

/// Only the service constructs a write after current management authorization.
pub struct OutboxRecoveryWrite {
    management: BridgeManagementLease,
    request: OutboxRecoveryRequest,
    receipt_id: String,
    request_sha256: String,
    validation: Option<BridgeValidation>,
}
impl OutboxRecoveryWrite {
    pub fn management(&self) -> &BridgeManagementLease {
        &self.management
    }
    pub fn request(&self) -> &OutboxRecoveryRequest {
        &self.request
    }
    pub fn receipt_id(&self) -> &str {
        &self.receipt_id
    }
    pub fn request_sha256(&self) -> &str {
        &self.request_sha256
    }
    pub fn validation(&self) -> Option<&BridgeValidation> {
        self.validation.as_ref()
    }
    pub fn require_current(&self) -> Result<(), OutboxManagementError> {
        self.management.require_current()?;
        if let Some(validation) = &self.validation {
            if tokio::time::Instant::now() >= validation.deadline() {
                return Err(OutboxError::AuthorityUnavailable.into());
            }
        }
        Ok(())
    }
    pub fn apply(&self, item: &mut OutboxItem, now_ms: i64) -> Result<(), OutboxManagementError> {
        self.require_current()?;
        require_source(&self.management, item)?;
        if item.id != self.request.item_id
            || item.revision != self.request.expected_revision
            || self.management.record().revision != self.request.expected_bridge_revision
        {
            return Err(OutboxError::Conflict.into());
        }
        match self.request.action {
            OutboxRecoveryAction::Retry {
                accept_duplicate_risk,
            } => {
                let validation = self
                    .validation
                    .as_ref()
                    .ok_or(OutboxError::AuthorityUnavailable)?;
                validation.require(item.source.owner(), item.source.config())?;
                item.source.require_bridge(self.management.record())?;
                let custody = validation.custody_pin();
                let credential = super::DeliveryCredentialRef::RetainedKey {
                    reference: custody.reference().clone(),
                    revision: custody.revision(),
                    key_state_version: custody.key_state_version(),
                };
                if item.attempts.len() >= 16 {
                    return Err(OutboxError::Bounds.into());
                }
                match item.state {
                    super::DeliveryState::Unknown { .. } => {
                        if !accept_duplicate_risk {
                            return Err(OutboxManagementError::DuplicateRiskRequired);
                        }
                        item.retry_unknown(
                            &self.management,
                            credential,
                            super::DuplicateRiskAccepted,
                            now_ms,
                        )?;
                    }
                    super::DeliveryState::Rejected { .. }
                    | super::DeliveryState::RetryWait { .. } => {
                        credential.require_source(&item.source, now_ms)?;
                        if now_ms < item.updated_at_ms {
                            return Err(OutboxError::Conflict.into());
                        }
                        item.credential = credential;
                        item.state = super::DeliveryState::Pending;
                        item.updated_at_ms = now_ms;
                        item.revision = item.revision.checked_add(1).ok_or(OutboxError::Corrupt)?;
                        item.validate()?;
                    }
                    _ => return Err(OutboxError::Conflict.into()),
                }
            }
            OutboxRecoveryAction::Abandon {} => item.abandon(&self.management, now_ms)?,
            OutboxRecoveryAction::Reconcile {} => {
                // No upstream reconciliation proof exists. Only an expired
                // local worker lease establishes a safe state transition.
                if matches!(item.state, super::DeliveryState::Claimed { expires_at_ms, .. }
                    | super::DeliveryState::Sending { expires_at_ms, .. } if expires_at_ms <= now_ms)
                {
                    item.recover_expired(now_ms)?;
                }
            }
        }
        Ok(())
    }
}

pub fn require_source(
    manager: &BridgeManagementLease,
    item: &OutboxItem,
) -> Result<(), OutboxManagementError> {
    manager.require_current()?;
    item.validate()?;
    let bridge = manager.record();
    if item.source.owner() != &bridge.owner
        || item.source.pin().bridge_id != bridge.id
        || bridge.pinned(item.source.pin())?.config != *item.source.config()
    {
        return Err(OutboxManagementError::NotFound);
    }
    Ok(())
}

#[async_trait::async_trait]
pub trait BridgeOutboxManagementStore: Send + Sync {
    async fn list(
        &self,
        manager: &BridgeManagementLease,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<OutboxItem>, OutboxManagementError>;
    async fn item(
        &self,
        manager: &BridgeManagementLease,
        id: &str,
    ) -> Result<Option<OutboxItem>, OutboxManagementError>;
    async fn recover(
        &self,
        manager: &BridgeManagementLease,
        id: &str,
        request_sha256: &str,
    ) -> Result<Option<OutboxRecoveryReceipt>, OutboxManagementError>;
    async fn apply(
        &self,
        write: OutboxRecoveryWrite,
    ) -> Result<OutboxRecoveryReceipt, OutboxManagementError>;
}
