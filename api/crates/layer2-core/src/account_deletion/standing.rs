use super::AccountDeletionError;
use crate::SubjectId;
use async_trait::async_trait;

/// Cached read authority; delayed effects use the authoritative check.
#[async_trait]
pub trait AccountStanding: Send + Sync {
    async fn erasing(&self, subject: &SubjectId) -> Result<bool, AccountDeletionError>;
    async fn authoritative(&self, subject: &SubjectId) -> Result<bool, AccountDeletionError> {
        self.erasing(subject).await
    }
    /// Block locally after an accepted or uncertain commit until authoritative reconciliation.
    fn insert_local(&self, _hash: &str) {}
}
/// An explicitly ungated account source, useful for independent domain fixtures.
pub struct AvailableAccounts;
#[async_trait]
impl AccountStanding for AvailableAccounts {
    async fn erasing(&self, _: &SubjectId) -> Result<bool, AccountDeletionError> {
        Ok(false)
    }
}
