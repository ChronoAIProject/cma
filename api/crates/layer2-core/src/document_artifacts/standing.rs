use super::*;
use std::sync::Arc;
impl DocumentService {
    pub fn with_account_standing(
        mut self: Arc<Self>,
        standing: Arc<dyn crate::account_deletion::AccountStanding>,
    ) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("document standing is configured before sharing")
            .standing = standing;
        self
    }
    pub(super) async fn require_account_available(
        &self,
        subject: &crate::SubjectId,
    ) -> Result<(), DocumentError> {
        if self
            .standing
            .erasing(subject)
            .await
            .map_err(|_| DocumentError::Unavailable)?
        {
            return Err(DocumentError::AccountDeletionInProgress);
        }
        Ok(())
    }
}
