use super::*;
use crate::{Authorization, Caller, Capability, SubjectId};
use std::{sync::Arc, time::Duration};
fn owner() -> Caller {
    Caller::new(
        SubjectId::new("owner"),
        Authorization::new([Capability::DeleteOwnAccount]),
    )
}
fn service() -> AccountDeletionService {
    AccountDeletionService::new(
        Arc::new(MemoryAccountDeletionStore::default()),
        Arc::new(AvailableAccounts),
        true,
    )
}
#[tokio::test]
async fn request_is_idempotent_per_key() {
    let service = service();
    let first = service.request(&owner(), "first", true).await.unwrap();
    assert_eq!(
        service.request(&owner(), "first", true).await.unwrap(),
        first
    );
}
#[tokio::test]
async fn second_request_while_active_returns_existing() {
    let service = service();
    let first = service.request(&owner(), "first", true).await.unwrap();
    assert_eq!(
        service.request(&owner(), "second", true).await.unwrap(),
        first
    );
}
#[tokio::test]
async fn request_requires_acknowledgement() {
    assert_eq!(
        service().request(&owner(), "first", false).await,
        Err(AccountDeletionError::AcknowledgementRequired)
    );
}
#[tokio::test]
async fn non_human_actor_is_refused() {
    for capability in [
        Capability::UseBindingConversations,
        Capability::UseOwnSandboxes,
        Capability::LaunchTriggers,
    ] {
        let caller = Caller::new(SubjectId::new("actor"), Authorization::new([capability]));
        assert_eq!(
            service().request(&caller, "first", true).await,
            Err(AccountDeletionError::RequiresOwnerSession)
        );
    }
}
#[tokio::test]
async fn disabled_request_and_absent_status_are_typed() {
    let service = AccountDeletionService::new(
        Arc::new(MemoryAccountDeletionStore::default()),
        Arc::new(AvailableAccounts),
        false,
    );
    assert_eq!(
        service.request(&owner(), "first", true).await,
        Err(AccountDeletionError::Unavailable)
    );
    assert_eq!(
        service.status(&owner()).await,
        Err(AccountDeletionError::NotFound)
    );
}
#[derive(Default)]
struct LocalStanding(std::sync::Mutex<std::collections::BTreeSet<String>>);
#[async_trait::async_trait]
impl AccountStanding for LocalStanding {
    async fn erasing(&self, subject: &SubjectId) -> Result<bool, AccountDeletionError> {
        Ok(self.0.lock().unwrap().contains(&subject_hash(subject)))
    }
    fn insert_local(&self, hash: &str) {
        self.0.lock().unwrap().insert(hash.into());
    }
}
#[tokio::test]
async fn accepted_request_updates_local_standing_after_commit() {
    let store = Arc::new(MemoryAccountDeletionStore::default());
    let standing = Arc::new(LocalStanding::default());
    let service = AccountDeletionService::new(store.clone(), standing.clone(), true);
    let receipt = service.request(&owner(), "first", true).await.unwrap();
    assert!(standing.erasing(&owner().subject).await.unwrap());
    assert_eq!(
        store.active(&owner().subject).await.unwrap().unwrap().id,
        receipt.id
    );
}
#[tokio::test]
async fn rejected_request_never_updates_local_standing() {
    let store = Arc::new(MemoryAccountDeletionStore::default());
    let standing = Arc::new(LocalStanding::default());
    let service = AccountDeletionService::new(store.clone(), standing.clone(), true);
    assert!(service.request(&owner(), "first", false).await.is_err());
    assert!(!standing.erasing(&owner().subject).await.unwrap());
    assert!(store.active(&owner().subject).await.unwrap().is_none());
}

struct FailingRequestStore {
    inner: Arc<MemoryAccountDeletionStore>,
    commit_before_error: bool,
    error: AccountDeletionError,
}

#[async_trait::async_trait]
impl AccountDeletionStore for FailingRequestStore {
    async fn request(
        &self,
        subject: &SubjectId,
        key: &str,
        at: &str,
    ) -> Result<AccountDeletion, AccountDeletionError> {
        if self.commit_before_error {
            self.inner.request(subject, key, at).await?;
        }
        Err(self.error.clone())
    }

    async fn active(
        &self,
        subject: &SubjectId,
    ) -> Result<Option<AccountDeletion>, AccountDeletionError> {
        self.inner.active(subject).await
    }

    async fn latest(
        &self,
        subject: &SubjectId,
    ) -> Result<Option<AccountDeletion>, AccountDeletionError> {
        self.inner.latest(subject).await
    }

    async fn get(
        &self,
        id: &AccountDeletionId,
    ) -> Result<Option<AccountDeletion>, AccountDeletionError> {
        self.inner.get(id).await
    }

    async fn claim_due(
        &self,
        holder: &str,
        now: &str,
        lease: Duration,
    ) -> Result<Option<AccountDeletion>, AccountDeletionError> {
        self.inner.claim_due(holder, now, lease).await
    }

    async fn commit(
        &self,
        deletion: AccountDeletion,
        events: Vec<ErasureEvent>,
        expected_revision: u64,
    ) -> Result<(), AccountDeletionError> {
        self.inner.commit(deletion, events, expected_revision).await
    }
}

async fn assert_failed_request_standing(
    error: AccountDeletionError,
    committed: bool,
    expected_erasing: bool,
) {
    let inner = Arc::new(MemoryAccountDeletionStore::default());
    let standing = Arc::new(LocalStanding::default());
    let store = Arc::new(FailingRequestStore {
        inner: inner.clone(),
        commit_before_error: committed,
        error: error.clone(),
    });
    let service = AccountDeletionService::new(store, standing.clone(), true);
    assert_eq!(service.request(&owner(), "first", true).await, Err(error));
    assert_eq!(
        standing.erasing(&owner().subject).await.unwrap(),
        expected_erasing
    );
    assert!(!standing.erasing(&SubjectId::new("other")).await.unwrap());
    assert_eq!(
        inner.active(&owner().subject).await.unwrap().is_some(),
        committed
    );
}

#[tokio::test]
async fn unknown_committed_request_blocks_locally_without_claiming_success() {
    assert_failed_request_standing(AccountDeletionError::RecoveryRequired, true, true).await;
}

#[tokio::test]
async fn unknown_uncommitted_request_conservatively_blocks_locally() {
    assert_failed_request_standing(AccountDeletionError::RecoveryRequired, false, true).await;
}

#[tokio::test]
async fn definite_store_failure_does_not_update_local_standing() {
    assert_failed_request_standing(
        AccountDeletionError::Store("request failed before commit".into()),
        false,
        false,
    )
    .await;
}
