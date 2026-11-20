use super::*;
use std::sync::Arc;
impl TriggerService {
    pub fn with_account_standing(
        mut self: Arc<Self>,
        standing: Arc<dyn crate::account_deletion::AccountStanding>,
    ) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("publisher standing is configured before sharing")
            .standing = standing;
        self
    }
    pub(super) async fn require_public_owner(
        &self,
        subject: &crate::SubjectId,
    ) -> Result<(), TriggerServiceError> {
        if self.standing.erasing(subject).await.map_err(|_| {
            TriggerServiceError::Unavailable("publisher standing is unavailable".into())
        })? {
            return Err(TriggerServiceError::Unavailable(
                "publisher is unavailable".into(),
            ));
        }
        Ok(())
    }
}
impl LaunchService {
    pub(super) async fn require_public_owner(
        &self,
        subject: &crate::SubjectId,
    ) -> Result<(), LaunchServiceError> {
        if self.standing.erasing(subject).await.map_err(|_| {
            LaunchServiceError::Unavailable("publisher standing is unavailable".into())
        })? {
            return Err(LaunchServiceError::Unavailable(
                "publisher is unavailable".into(),
            ));
        }
        Ok(())
    }

    pub fn with_account_standing(
        mut self: Arc<Self>,
        standing: Arc<dyn crate::account_deletion::AccountStanding>,
    ) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("launch standing is configured before sharing")
            .standing = standing;
        self
    }
}
