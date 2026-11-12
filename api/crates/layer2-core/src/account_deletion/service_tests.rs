use super::*;
use crate::{Authorization, Caller, Capability, SubjectId};
use std::sync::Arc;
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
