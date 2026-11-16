//! Restore admission boundary for account-deletion replay.

use crate::SubjectId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DeletionReplayError {
    #[error("owner was deleted")]
    OwnerErased,
    #[error("deletion state is unavailable")]
    Unavailable,
}

/// Recovery callers implement this port with the deletion ledger or its
/// storage-outside-the-restored-database replica. The D8 ledger format and
/// writer are deliberately outside this port.
#[async_trait::async_trait]
pub trait DeletionReplayPort: Send + Sync {
    async fn owner_is_deleted(&self, owner: &SubjectId) -> Result<bool, DeletionReplayError>;

    async fn ensure_owner_not_deleted(&self, owner: &SubjectId) -> Result<(), DeletionReplayError> {
        if self.owner_is_deleted(owner).await? {
            Err(DeletionReplayError::OwnerErased)
        } else {
            Ok(())
        }
    }
}

pub async fn reject_deleted_owner(
    port: &dyn DeletionReplayPort,
    owner: &SubjectId,
) -> Result<(), DeletionReplayError> {
    port.ensure_owner_not_deleted(owner).await
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Port(bool);
    #[async_trait::async_trait]
    impl DeletionReplayPort for Port {
        async fn owner_is_deleted(&self, _: &SubjectId) -> Result<bool, DeletionReplayError> {
            Ok(self.0)
        }
    }
    #[tokio::test]
    async fn deleted_owner_is_refused() {
        assert_eq!(
            reject_deleted_owner(&Port(true), &SubjectId::new("owner")).await,
            Err(DeletionReplayError::OwnerErased)
        );
    }
    #[tokio::test]
    async fn live_owner_passes() {
        assert_eq!(
            reject_deleted_owner(&Port(false), &SubjectId::new("owner")).await,
            Ok(())
        );
    }
}
