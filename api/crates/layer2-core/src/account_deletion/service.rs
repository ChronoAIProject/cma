use super::*;
use crate::{Caller, Capability, SubjectId};
use async_trait::async_trait;
use std::{sync::Arc, time::Duration};

#[async_trait]
pub trait AccountDeletionStore: Send + Sync {
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
        key: &str,
        acknowledged: bool,
    ) -> Result<Receipt, AccountDeletionError> {
        require_owner(caller)?;
        if !self.enabled {
            return Err(AccountDeletionError::Unavailable);
        }
        if !acknowledged {
            return Err(AccountDeletionError::AcknowledgementRequired);
        }
        if key.is_empty()
            || key.len() > 255
            || !key.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
        {
            return Err(AccountDeletionError::Invalid);
        }
        let deletion = match self
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
        };
        if let Some(hash) = &deletion.active_subject_hash {
            self.standing.insert_local(hash);
        }
        Ok(deletion.receipt())
    }
    pub async fn status(&self, caller: &Caller) -> Result<Receipt, AccountDeletionError> {
        require_owner(caller)?;
        self.store
            .latest(&caller.subject)
            .await?
            .map(|deletion| deletion.receipt())
            .ok_or(AccountDeletionError::NotFound)
    }
}
// Pending security decision D7: verified human authority is sufficient; recent
// identity reauthentication is not asserted by this contract.
fn require_owner(caller: &Caller) -> Result<(), AccountDeletionError> {
    caller
        .require(Capability::DeleteOwnAccount)
        .map_err(|_| AccountDeletionError::RequiresOwnerSession)
}
