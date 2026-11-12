use super::*;
use crate::{Authorization, Caller, Capability, SubjectId};
use std::{sync::Arc, time::Duration};
fn owner() -> Caller {
    Caller::new(SubjectId::new("owner"), Authorization::none())
}
fn authority() -> OwnerSessionAuthority {
    OwnerSessionAuthority::verified_personal(owner().subject)
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
    let first = service
        .request(&owner(), Some(&authority()), "first", true)
        .await
        .unwrap();
    assert_eq!(
        service
            .request(&owner(), Some(&authority()), "first", true)
            .await
            .unwrap(),
        first
    );
}
#[tokio::test]
async fn second_request_while_active_returns_existing() {
    let service = service();
    let first = service
        .request(&owner(), Some(&authority()), "first", true)
        .await
        .unwrap();
    assert_eq!(
        service
            .request(&owner(), Some(&authority()), "second", true)
            .await
            .unwrap(),
        first
    );
}
#[tokio::test]
async fn request_requires_acknowledgement() {
    assert_eq!(
        service()
            .request(&owner(), Some(&authority()), "first", false)
            .await,
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
            service().request(&caller, None, "first", true).await,
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
        service
            .request(&owner(), Some(&authority()), "first", true)
            .await,
        Err(AccountDeletionError::Unavailable)
    );
    assert_eq!(
        service.status(&owner(), Some(&authority())).await,
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
    let receipt = service
        .request(&owner(), Some(&authority()), "first", true)
        .await
        .unwrap();
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
    assert!(service
        .request(&owner(), Some(&authority()), "first", false)
        .await
        .is_err());
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

    async fn replay(
        &self,
        subject: &SubjectId,
        key: &str,
    ) -> Result<Option<AccountDeletion>, AccountDeletionError> {
        self.inner.replay(subject, key).await
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
        expected_lease_token: Option<&str>,
    ) -> Result<(), AccountDeletionError> {
        self.inner
            .commit(deletion, events, expected_revision, expected_lease_token)
            .await
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
    assert_eq!(
        service
            .request(&owner(), Some(&authority()), "first", true)
            .await,
        Err(error)
    );
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

async fn assert_unknown_commit_reconciles_through_the_original_key(committed: bool) {
    let inner = Arc::new(MemoryAccountDeletionStore::default());
    let standing = Arc::new(LocalStanding::default());
    let uncertain = AccountDeletionService::new(
        Arc::new(FailingRequestStore {
            inner: inner.clone(),
            commit_before_error: committed,
            error: AccountDeletionError::RecoveryRequired,
        }),
        standing.clone(),
        true,
    );
    assert_eq!(
        uncertain
            .request(&owner(), Some(&authority()), "original", true)
            .await,
        Err(AccountDeletionError::RecoveryRequired)
    );
    assert!(standing.erasing(&owner().subject).await.unwrap());
    let persisted = inner.replay(&owner().subject, "original").await.unwrap();
    if let Some(record) = &persisted {
        assert!(committed);
        assert_eq!(
            uncertain
                .status(&owner(), Some(&authority()))
                .await
                .unwrap(),
            record.receipt()
        );
    } else {
        assert!(!committed);
        assert_eq!(
            uncertain.status(&owner(), Some(&authority())).await,
            Err(AccountDeletionError::NotFound)
        );
    }

    let restored = AccountDeletionService::new(inner.clone(), standing.clone(), true);
    let receipt = restored
        .request(&owner(), Some(&authority()), "original", true)
        .await
        .unwrap();
    if let Some(record) = persisted {
        assert_eq!(receipt, record.receipt());
    }
    assert_eq!(
        restored
            .request(&owner(), Some(&authority()), "original", true)
            .await
            .unwrap(),
        receipt
    );
    assert_eq!(
        restored.status(&owner(), Some(&authority())).await.unwrap(),
        receipt
    );
    assert!(standing.erasing(&owner().subject).await.unwrap());
    let record = inner
        .replay(&owner().subject, "original")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.id, receipt.id);
    assert_eq!((record.revision, record.event_seq), (1, 1));
    assert_eq!(
        inner
            .claim_due(
                "first-worker",
                &record.requested_at,
                Duration::from_secs(90)
            )
            .await
            .unwrap()
            .unwrap()
            .id,
        receipt.id
    );
    assert!(inner
        .claim_due(
            "second-worker",
            &record.requested_at,
            Duration::from_secs(90)
        )
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn unknown_committed_request_replays_the_authoritative_receipt_without_duplicate_effects() {
    assert_unknown_commit_reconciles_through_the_original_key(true).await;
}

#[tokio::test]
async fn unknown_aborted_request_retries_the_original_key_into_one_durable_deletion() {
    assert_unknown_commit_reconciles_through_the_original_key(false).await;
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

#[tokio::test]
async fn owner_session_authority_is_required_and_subject_bound() {
    let service = service();
    assert_eq!(
        service.request(&owner(), None, "first", true).await,
        Err(AccountDeletionError::RequiresOwnerSession)
    );
    let other = OwnerSessionAuthority::verified_personal(SubjectId::new("other"));
    assert_eq!(
        service.request(&owner(), Some(&other), "first", true).await,
        Err(AccountDeletionError::RequiresOwnerSession)
    );
    assert_eq!(
        service.status(&owner(), Some(&other)).await,
        Err(AccountDeletionError::RequiresOwnerSession)
    );
    assert!(service
        .request(&owner(), Some(&authority()), "first", true)
        .await
        .is_ok());
}

#[tokio::test]
async fn disabled_writer_replays_only_the_original_persisted_key() {
    let store = Arc::new(MemoryAccountDeletionStore::default());
    let first = store
        .request(&owner().subject, "first", "2026-11-14T00:00:00Z")
        .await
        .unwrap();
    let service = AccountDeletionService::new(store.clone(), Arc::new(AvailableAccounts), false);
    assert_eq!(
        service
            .request(&owner(), Some(&authority()), "first", true)
            .await
            .unwrap(),
        first.receipt()
    );
    assert_eq!(
        service
            .request(&owner(), Some(&authority()), "second", true)
            .await,
        Err(AccountDeletionError::Unavailable)
    );
    assert!(store
        .replay(&owner().subject, "second")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn active_new_keys_return_the_same_receipt_without_becoming_replay_identities() {
    let store = Arc::new(MemoryAccountDeletionStore::default());
    let service = AccountDeletionService::new(store.clone(), Arc::new(AvailableAccounts), true);
    let first = service
        .request(&owner(), Some(&authority()), "first", true)
        .await
        .unwrap();
    for index in 0..32 {
        let key = format!("later-{index}");
        assert_eq!(
            service
                .request(&owner(), Some(&authority()), &key, true)
                .await
                .unwrap(),
            first
        );
        assert!(store
            .replay(&owner().subject, &key)
            .await
            .unwrap()
            .is_none());
    }
    assert!(store
        .replay(&owner().subject, "first")
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn request_keys_use_the_canonical_visible_ascii_boundary() {
    let longest = "a".repeat(255);
    assert!(service()
        .request(&owner(), Some(&authority()), &longest, true)
        .await
        .is_ok());
    for key in [
        "".into(),
        " ".into(),
        "a\nb".into(),
        "\u{7f}".into(),
        "é".into(),
        "a".repeat(256),
    ] {
        assert_eq!(
            service()
                .request(&owner(), Some(&authority()), &key, true)
                .await,
            Err(AccountDeletionError::InvalidIdempotencyKey)
        );
    }
}
