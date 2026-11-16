use super::*;
use std::sync::Arc;
impl ResourceAccessService {
    pub fn with_account_standing(
        mut self,
        standing: Arc<dyn crate::account_deletion::AccountStanding>,
    ) -> Self {
        self.standing = standing;
        self
    }
    pub(super) async fn require_account_available(
        &self,
        subject: &crate::SubjectId,
        authoritative: bool,
    ) -> Result<(), ResourceAccessError> {
        let erasing = if authoritative {
            self.standing.authoritative(subject).await
        } else {
            self.standing.erasing(subject).await
        };
        if erasing.map_err(|_| ResourceAccessStoreError::Unavailable)? {
            return Err(ResourceAccessError::AccountDeletionInProgress);
        }
        Ok(())
    }
    pub(super) async fn require_owner_available(
        &self,
        owner: &ResourceOwner,
        authoritative: bool,
    ) -> Result<(), ResourceAccessError> {
        if let ResourceOwner::Person { subject } = owner {
            self.require_account_available(subject, authoritative)
                .await?;
        }
        Ok(())
    }
    pub(super) async fn require_recipient_available(
        &self,
        subject: &crate::SubjectId,
    ) -> Result<(), ResourceAccessError> {
        if self
            .standing
            .authoritative(subject)
            .await
            .map_err(|_| ResourceAccessStoreError::Unavailable)?
        {
            return Err(ResourceAccessStoreError::ShareRecipientUnavailable.into());
        }
        Ok(())
    }
}
