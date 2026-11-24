use super::*;
use crate::connections::subscription_store::RefreshLease;

#[tokio::test]
async fn usable_cache_and_previous_account_preserve_refresh_decisions() {
    let (manager, store, native, owner) = fixture(cache("account", 7200)).await;
    let mut current = store.get(&owner).await.unwrap().unwrap();
    current.cache = Some(cache("account", 3600));
    store.0.lock().unwrap().insert(owner.clone(), current);
    assert_eq!(
        manager
            .tokens(&owner, false, None)
            .await
            .unwrap()
            .account_id,
        "account"
    );
    assert_eq!(native.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        manager
            .tokens(&owner, false, Some("foreign"))
            .await
            .unwrap_err(),
        SubscriptionError::SignInRequired
    );
    assert_eq!(native.calls.load(Ordering::SeqCst), 0);
    native.release.add_permits(1);
    manager.tokens(&owner, true, Some("account")).await.unwrap();
    assert_eq!(native.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn forced_refresh_accepts_a_different_cache_published_while_waiting() {
    let (manager, store, native, owner) = fixture(cache("account", 7200)).await;
    let mut current = store.get(&owner).await.unwrap().unwrap();
    current.cache = Some(cache("account", 3600));
    current.refresh_lease = Some(RefreshLease {
        holder: "other-refresher".into(),
        expires_at: SystemTime::now() + Duration::from_secs(30),
    });
    store.0.lock().unwrap().insert(owner.clone(), current);
    let mut waiting = Box::pin(manager.tokens(&owner, true, Some("account")));
    assert!(futures_util::poll!(&mut waiting).is_pending());
    let current = store.get(&owner).await.unwrap().unwrap();
    let fresh = cache("account", 7200);
    let mut next = current.clone();
    next.revision += 1;
    next.cache = Some(fresh.clone());
    next.refresh_lease = None;
    assert!(store.replace(&current, next).await.unwrap());
    assert_eq!(
        waiting.await.unwrap().access_token,
        SubscriptionTokens::from_native_cache(&fresh)
            .unwrap()
            .access_token
    );
    assert_eq!(native.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn changed_incarnation_or_lease_holder_defeats_refresh_completion() {
    for incarnation in [true, false] {
        let (manager, store, native, owner) = fixture(cache("account", 3600)).await;
        let refreshing = {
            let manager = manager.clone();
            let owner = owner.clone();
            tokio::spawn(async move { manager.tokens(&owner, false, None).await })
        };
        native.entered.notified().await;
        let mut newer = store.get(&owner).await.unwrap().unwrap();
        newer.revision += 1;
        if incarnation {
            newer.incarnation = uuid::Uuid::new_v4().to_string();
        } else {
            newer.refresh_lease.as_mut().unwrap().holder = "replacement-refresher".into();
        }
        let original = newer.cache.clone();
        store.0.lock().unwrap().insert(owner.clone(), newer);
        native.release.add_permits(1);
        assert_eq!(
            refreshing.await.unwrap().unwrap_err(),
            SubscriptionError::Changed
        );
        let retained = store.get(&owner).await.unwrap().unwrap();
        assert_eq!(retained.cache, original);
        assert!(retained.refresh_lease.is_some());
    }
}

struct FailingNative(SubscriptionError);

#[async_trait::async_trait]
impl NativeSubscriptionAuth for FailingNative {
    async fn start(&self) -> Result<NativeLogin, SubscriptionError> {
        panic!("refresh must not start device login")
    }
    async fn refresh(&self, _: &SecretValue) -> Result<SecretValue, SubscriptionError> {
        Err(self.0)
    }
}

#[tokio::test]
async fn native_domain_errors_clear_only_the_original_lease_and_required_cache() {
    for error in [
        SubscriptionError::SignInRequired,
        SubscriptionError::Expired,
        SubscriptionError::Cancelled,
        SubscriptionError::Unavailable,
        SubscriptionError::InvalidAccount,
        SubscriptionError::Changed,
    ] {
        let (_, store, _, owner) = fixture(cache("account", 3600)).await;
        let original = store.get(&owner).await.unwrap().unwrap().cache;
        let manager = CodexSubscriptions {
            store: store.clone(),
            native: Arc::new(FailingNative(error)),
            worker: "synthetic-worker".into(),
            login_capacity: Arc::new(Semaphore::new(1)),
        };
        assert_eq!(
            manager.tokens(&owner, false, None).await.unwrap_err(),
            error
        );
        let retained = store.get(&owner).await.unwrap().unwrap();
        assert!(retained.refresh_lease.is_none());
        assert_eq!(
            retained.cache,
            if error == SubscriptionError::SignInRequired {
                None
            } else {
                original
            }
        );
    }
}

struct ConflictedStore {
    inner: Arc<MemoryStore>,
    completion_calls: AtomicUsize,
    uncertain: bool,
}

#[async_trait::async_trait]
impl SubscriptionStore for ConflictedStore {
    async fn get(
        &self,
        owner: &SubjectId,
    ) -> Result<Option<SubscriptionRecord>, SubscriptionError> {
        self.inner.get(owner).await
    }
    async fn insert(&self, _: SubscriptionRecord) -> Result<bool, SubscriptionError> {
        panic!("refresh cannot insert a new credential record")
    }
    async fn replace(
        &self,
        expected: &SubscriptionRecord,
        next: SubscriptionRecord,
    ) -> Result<bool, SubscriptionError> {
        if next.refresh_lease.is_none() {
            self.completion_calls.fetch_add(1, Ordering::SeqCst);
            if self.uncertain {
                Err(SubscriptionError::Unavailable)
            } else {
                Ok(false)
            }
        } else {
            self.inner.replace(expected, next).await
        }
    }
}

#[tokio::test]
async fn completion_cas_is_bounded_and_unknown_write_is_never_retried() {
    for uncertain in [false, true] {
        let (_, store, native, owner) = fixture(cache("account", 3600)).await;
        let original = store.get(&owner).await.unwrap().unwrap().cache;
        let conflicted = Arc::new(ConflictedStore {
            inner: store.clone(),
            completion_calls: AtomicUsize::new(0),
            uncertain,
        });
        let manager = CodexSubscriptions {
            store: conflicted.clone(),
            native: native.clone(),
            worker: "synthetic-worker".into(),
            login_capacity: Arc::new(Semaphore::new(1)),
        };
        native.release.add_permits(1);
        let error = manager.tokens(&owner, false, None).await.unwrap_err();
        assert_eq!(
            error,
            if uncertain {
                SubscriptionError::Unavailable
            } else {
                SubscriptionError::Changed
            }
        );
        assert_eq!(
            conflicted.completion_calls.load(Ordering::SeqCst),
            if uncertain { 1 } else { CAS_ATTEMPTS }
        );
        let retained = store.get(&owner).await.unwrap().unwrap();
        assert_eq!(retained.cache, original);
        assert!(retained.refresh_lease.is_some());
        assert_eq!(native.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn cancelled_refresher_keeps_the_lease_and_never_publishes_a_cache() {
    let (manager, store, native, owner) = fixture(cache("account", 3600)).await;
    let original = store.get(&owner).await.unwrap().unwrap().cache;
    let mut refreshing = Box::pin(manager.tokens(&owner, false, None));
    assert!(futures_util::poll!(&mut refreshing).is_pending());
    assert_eq!(native.calls.load(Ordering::SeqCst), 1);
    drop(refreshing);
    let retained = store.get(&owner).await.unwrap().unwrap();
    assert_eq!(retained.cache, original);
    assert!(retained.refresh_lease.is_some());
}

#[tokio::test]
async fn missing_or_foreign_record_is_rejected_before_native_refresh() {
    for foreign in [false, true] {
        let (manager, store, native, owner) = fixture(cache("account", 3600)).await;
        if foreign {
            store.0.lock().unwrap().get_mut(&owner).unwrap().owner =
                SubjectId::new("different-owner");
        } else {
            store.0.lock().unwrap().remove(&owner);
        }
        assert_eq!(
            manager.tokens(&owner, false, None).await.unwrap_err(),
            SubscriptionError::SignInRequired
        );
        assert_eq!(native.calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn refresh_deadline_cancels_native_wait_without_releasing_authority() {
    let (manager, store, native, owner) = fixture(cache("account", 3600)).await;
    let original = store.get(&owner).await.unwrap().unwrap().cache;
    let start = std::time::Instant::now();
    assert_eq!(
        manager.tokens(&owner, false, None).await.unwrap_err(),
        SubscriptionError::Unavailable
    );
    assert!(start.elapsed() >= REFRESH_BUDGET);
    assert!(start.elapsed() < REFRESH_BUDGET + Duration::from_secs(10));
    assert_eq!(native.calls.load(Ordering::SeqCst), 1);
    let retained = store.get(&owner).await.unwrap().unwrap();
    assert_eq!(retained.cache, original);
    assert!(retained.refresh_lease.is_some());
}
