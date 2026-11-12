use super::*;
use crate::{Caller, SubjectId};
use async_trait::async_trait;
use std::{sync::Arc, time::Duration};

#[async_trait]
pub trait AccountDeletionStore: Send + Sync {
    async fn replay(
        &self,
        subject: &SubjectId,
        key: &str,
    ) -> Result<Option<AccountDeletion>, AccountDeletionError>;
    async fn request(
        &self,
        subject: &SubjectId,
        key: &str,
        at: &str,
    ) -> Result<AccountDeletion, AccountDeletionError>;
    async fn active(
        &self,
        subject: &SubjectId,
    ) -> Result<Option<AccountDeletion>, AccountDeletionError>;
    async fn latest(
        &self,
        subject: &SubjectId,
    ) -> Result<Option<AccountDeletion>, AccountDeletionError>;
    async fn get(
        &self,
        id: &AccountDeletionId,
    ) -> Result<Option<AccountDeletion>, AccountDeletionError>;
    async fn claim_due(
        &self,
        holder: &str,
        now: &str,
        lease: Duration,
    ) -> Result<Option<AccountDeletion>, AccountDeletionError>;
    async fn commit(
        &self,
        deletion: AccountDeletion,
        events: Vec<ErasureEvent>,
        expected_revision: u64,
        expected_lease_token: Option<&str>,
    ) -> Result<(), AccountDeletionError>;
}
#[async_trait]
pub trait AccountErasureEffects: Send + Sync {
    async fn run(
        &self,
        deletion: &AccountDeletion,
        step: ErasureStep,
    ) -> Result<StepOutcome, ErasureFailure>;
}
pub struct AccountDeletionService {
    store: Arc<dyn AccountDeletionStore>,
    standing: Arc<dyn AccountStanding>,
    enabled: bool,
}
impl AccountDeletionService {
    pub fn new(
        store: Arc<dyn AccountDeletionStore>,
        standing: Arc<dyn AccountStanding>,
        enabled: bool,
    ) -> Self {
        Self {
            store,
            standing,
            enabled,
        }
    }
    pub async fn request(
        &self,
        caller: &Caller,
        authority: Option<&OwnerSessionAuthority>,
        key: &str,
        acknowledged: bool,
    ) -> Result<Receipt, AccountDeletionError> {
        require_owner(caller, authority)?;
        if !acknowledged {
            return Err(AccountDeletionError::AcknowledgementRequired);
        }
        if !crate::operation::valid_idempotency_key(key.as_bytes()) {
            return Err(AccountDeletionError::InvalidIdempotencyKey);
        }
        let deletion = if let Some(record) = self.store.replay(&caller.subject, key).await? {
            record
        } else {
            if !self.enabled {
                return Err(AccountDeletionError::Unavailable);
            }
            match self
                .store
                .request(&caller.subject, key, &crate::now_rfc3339())
                .await
            {
                Ok(deletion) => deletion,
                Err(AccountDeletionError::RecoveryRequired) => {
                    self.standing.insert_local(&subject_hash(&caller.subject));
                    return Err(AccountDeletionError::RecoveryRequired);
                }
                Err(error) => return Err(error),
            }
        };
        if let Some(hash) = &deletion.active_subject_hash {
            self.standing.insert_local(hash);
        }
        Ok(deletion.receipt())
    }
    pub async fn status(
        &self,
        caller: &Caller,
        authority: Option<&OwnerSessionAuthority>,
    ) -> Result<Receipt, AccountDeletionError> {
        require_owner(caller, authority)?;
        self.store
            .latest(&caller.subject)
            .await?
            .map(|deletion| deletion.receipt())
            .ok_or(AccountDeletionError::NotFound)
    }
}
// Pending security decision: verified human authority is sufficient; recent
// identity reauthentication is not asserted by this contract.
fn require_owner(
    caller: &Caller,
    authority: Option<&OwnerSessionAuthority>,
) -> Result<(), AccountDeletionError> {
    if authority.is_some_and(|authority| authority.subject() == &caller.subject) {
        Ok(())
    } else {
        Err(AccountDeletionError::RequiresOwnerSession)
    }
}
