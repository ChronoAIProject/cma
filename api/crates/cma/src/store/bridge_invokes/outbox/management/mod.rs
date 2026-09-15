use super::*;
use layer2_core::bridge::{outbox::management::*, BridgeError, BridgeManagementLease};
mod reads;
mod write;

#[derive(thiserror::Error, Debug)]
enum ManagementMutationError {
    #[error(transparent)]
    Domain(#[from] OutboxManagementError),
    #[error(transparent)]
    Driver(#[from] mongodb::error::Error),
}
impl From<OutboxError> for ManagementMutationError {
    fn from(error: OutboxError) -> Self {
        Self::Domain(error.into())
    }
}
impl From<BridgeError> for ManagementMutationError {
    fn from(error: BridgeError) -> Self {
        Self::Domain(error.into())
    }
}
impl From<MutationError> for ManagementMutationError {
    fn from(error: MutationError) -> Self {
        match error {
            MutationError::Core(e) => e.into(),
            MutationError::Driver(e) => Self::Driver(e),
        }
    }
}
impl ManagementMutationError {
    fn retryable(&self) -> bool {
        matches!(self, Self::Driver(e) if is_transient(e) || is_duplicate_key(e))
    }
    fn into_core(self) -> OutboxManagementError {
        match self {
            Self::Domain(error) => error,
            Self::Driver(_) => OutboxError::Unavailable.into(),
        }
    }
}
impl MongoBridgeReplyToolStore {
    async fn fence_manager(
        &self,
        manager: &BridgeManagementLease,
        session: &mut ClientSession,
    ) -> Result<(), ManagementMutationError> {
        manager.require_current()?;
        let document = self
            .invokes
            .bridges
            .definitions
            .find_one(doc! {"_id":manager.record().id.as_str()})
            .session(&mut *session)
            .await?
            .ok_or(BridgeError::NotFound)?;
        let bridge = crate::store::bridge_definitions::decode_bridge(&document)?;
        if bridge != *manager.record() {
            return Err(OutboxError::Conflict.into());
        }
        let changed = self
            .invokes
            .bridges
            .definitions
            .update_one(
                doc! {"_id":bridge.id.as_str(),"revision":bridge.revision as i64},
                doc! {"$inc":{"outbox_management_fence":1_i64}},
            )
            .session(session)
            .await?;
        if changed.matched_count != 1 {
            return Err(OutboxError::Conflict.into());
        }
        manager.require_current()?;
        Ok(())
    }
    async fn recovery_in(
        &self,
        manager: &BridgeManagementLease,
        id: &str,
        digest: &str,
        session: &mut ClientSession,
    ) -> Result<Option<OutboxRecoveryReceipt>, ManagementMutationError> {
        let Some(document) = self
            .recoveries
            .find_one(doc! {"_id":id})
            .session(session)
            .await?
        else {
            return Ok(None);
        };
        let receipt: OutboxRecoveryReceipt =
            read_record(&document).map_err(|_| OutboxError::Corrupt)?;
        if receipt.id != id
            || receipt.bridge_id != manager.record().id
            || receipt.actor != *manager.actor()
            || receipt.result.bridge_id != receipt.bridge_id
            || document.get_str("bridge_id").ok() != Some(receipt.bridge_id.as_str())
            || document.get_str("request_sha256").ok() != Some(&receipt.request_sha256)
        {
            return Err(OutboxError::Corrupt.into());
        }
        if receipt.request_sha256 != digest {
            return Err(BridgeError::IdempotencyConflict.into());
        }
        manager.require_current()?;
        Ok(Some(receipt))
    }
}
