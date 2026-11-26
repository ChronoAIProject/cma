use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use tokio::sync::Notify;

use super::*;

#[path = "subscriptions_refresh_tests.rs"]
mod refresh_cases;

#[derive(Default)]
struct MemoryStore(Mutex<HashMap<SubjectId, SubscriptionRecord>>);

#[async_trait::async_trait]
impl SubscriptionStore for MemoryStore {
    async fn get(
        &self,
        owner: &SubjectId,
    ) -> Result<Option<SubscriptionRecord>, SubscriptionError> {
        Ok(self.0.lock().unwrap().get(owner).cloned())
    }
    async fn insert(&self, record: SubscriptionRecord) -> Result<bool, SubscriptionError> {
        let mut records = self.0.lock().unwrap();
        if records.contains_key(&record.owner) {
            return Ok(false);
        }
        records.insert(record.owner.clone(), record);
        Ok(true)
    }
    async fn replace(
        &self,
        expected: &SubscriptionRecord,
        next: SubscriptionRecord,
    ) -> Result<bool, SubscriptionError> {
        let mut records = self.0.lock().unwrap();
        let Some(current) = records.get(&expected.owner) else {
            return Ok(false);
        };
        if current.incarnation != expected.incarnation || current.revision != expected.revision {
            return Ok(false);
        }
        assert_eq!(expected.owner, next.owner);
        assert_eq!(expected.incarnation, next.incarnation);
        assert_eq!(expected.revision + 1, next.revision);
        records.insert(next.owner.clone(), next);
        Ok(true)
    }
}

fn cache(account: &str, lifetime: u64) -> SecretValue {
    let claims = serde_json::json!({
        "exp": (SystemTime::now() + Duration::from_secs(lifetime)).duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs(),
        "https://api.openai.com/auth": {"chatgpt_account_id": account, "chatgpt_plan_type":"plus"},
    });
    SecretValue::new(serde_json::json!({"tokens": {
        "access_token":format!("fixture.{}.fixture", URL_SAFE_NO_PAD.encode(claims.to_string())),
        "refresh_token":"renewable-secret", "id_token":"id-secret", "account_id":account,
    }}).to_string())
}

struct Native {
    calls: AtomicUsize,
    entered: Notify,
    release: Semaphore,
    renewed: SecretValue,
    completion: Mutex<Option<oneshot::Sender<Result<SecretValue, SubscriptionError>>>>,
}

#[async_trait::async_trait]
impl NativeSubscriptionAuth for Native {
    async fn start(&self) -> Result<NativeLogin, SubscriptionError> {
        let (sender, completion) = oneshot::channel();
        *self.completion.lock().unwrap() = Some(sender);
        let (cancel, _cancelled) = oneshot::channel();
        Ok(NativeLogin {
            device: DeviceLogin {
                login_id: uuid::Uuid::new_v4().to_string(),
                verification_url: "https://auth.openai.com/codex/device".into(),
                user_code: "ABCD-1234".into(),
            },
            completion,
            cancel: Some(cancel),
        })
    }
    async fn refresh(&self, _cache: &SecretValue) -> Result<SecretValue, SubscriptionError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        let permit = self.release.acquire().await.unwrap();
        permit.forget();
        Ok(self.renewed.clone())
    }
}

async fn fixture(
    renewed: SecretValue,
) -> (
    Arc<CodexSubscriptions>,
    Arc<MemoryStore>,
    Arc<Native>,
    SubjectId,
) {
    let store = Arc::new(MemoryStore::default());
    let owner = SubjectId::new("owner");
    let mut record = SubscriptionRecord::empty(owner.clone());
    record.cache = Some(cache("account", 1));
    store.insert(record).await.unwrap();
    let native = Arc::new(Native {
        calls: AtomicUsize::new(0),
        entered: Notify::new(),
        release: Semaphore::new(0),
        renewed,
        completion: Mutex::new(None),
    });
    let manager = Arc::new(CodexSubscriptions {
        store: store.clone(),
        native: native.clone(),
        worker: "worker".into(),
        login_capacity: Arc::new(Semaphore::new(4)),
    });
    (manager, store, native, owner)
}

#[tokio::test]
async fn concurrent_agents_refresh_one_native_credential_and_share_the_result() {
    let (manager, store, native, owner) = fixture(cache("account", 3600)).await;
    let first = {
        let manager = manager.clone();
        let owner = owner.clone();
        tokio::spawn(async move { manager.tokens(&owner, false, None).await })
    };
    native.entered.notified().await;
    let second = {
        let manager = manager.clone();
        let owner = owner.clone();
        tokio::spawn(async move { manager.tokens(&owner, false, None).await })
    };
    native.release.add_permits(1);
    let first = first.await.unwrap().unwrap();
    let second = second.await.unwrap().unwrap();
    assert_eq!(first.access_token, second.access_token);
    assert_eq!(native.calls.load(Ordering::SeqCst), 1);
    assert!(store
        .get(&owner)
        .await
        .unwrap()
        .unwrap()
        .refresh_lease
        .is_none());
}

#[tokio::test]
async fn newer_login_wins_over_an_inflight_refresh() {
    let (manager, store, native, owner) = fixture(cache("account", 3600)).await;
    let refreshing = {
        let manager = manager.clone();
        let owner = owner.clone();
        tokio::spawn(async move { manager.tokens(&owner, false, None).await })
    };
    native.entered.notified().await;
    let current = store.get(&owner).await.unwrap().unwrap();
    let mut newer = current.clone();
    newer.revision += 1;
    newer.cache = Some(cache("new-account", 3600));
    newer.refresh_lease = None;
    assert!(store.replace(&current, newer.clone()).await.unwrap());
    native.release.add_permits(1);
    assert_eq!(
        refreshing.await.unwrap().unwrap_err(),
        SubscriptionError::Changed
    );
    assert_eq!(store.get(&owner).await.unwrap().unwrap().cache, newer.cache);
    assert_eq!(
        manager
            .tokens(&owner, false, Some("account"))
            .await
            .unwrap_err(),
        SubscriptionError::SignInRequired
    );
}

#[tokio::test]
async fn invalid_refresh_never_publishes_another_accounts_credentials() {
    let (manager, store, native, owner) = fixture(cache("foreign-account", 3600)).await;
    let original = store.get(&owner).await.unwrap().unwrap().cache;
    native.release.add_permits(1);
    assert_eq!(
        manager.tokens(&owner, false, None).await.unwrap_err(),
        SubscriptionError::InvalidAccount
    );
    let retained = store.get(&owner).await.unwrap().unwrap();
    assert_eq!(retained.cache, original);
    assert!(retained.refresh_lease.is_none());
}

#[tokio::test]
async fn login_is_owner_bound_resumable_and_cancellation_fences_completion() {
    let (manager, store, native, owner) = fixture(cache("account", 3600)).await;
    let pending = manager.start(&owner, "login-one").await.unwrap();
    assert_eq!(
        manager
            .start(&owner, "login-one")
            .await
            .unwrap()
            .device
            .login_id,
        pending.device.login_id
    );
    assert_eq!(
        manager
            .status(&SubjectId::new("foreign"), "login-one")
            .await
            .unwrap_err(),
        SubscriptionError::SignInRequired
    );
    let original = store.get(&owner).await.unwrap().unwrap().cache;
    manager.cancel(&owner, "login-one").await.unwrap();
    let completed_cache = cache("different-account", 3600);
    assert_eq!(
        manager
            .update_login(
                &owner,
                "login-one",
                Some(completed_cache.clone()),
                Some(BrokerHandoffState::Satisfied)
            )
            .await
            .unwrap_err(),
        SubscriptionError::Changed
    );
    let _ = native
        .completion
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Ok(completed_cache));
    assert_eq!(
        manager.status(&owner, "login-one").await.unwrap().state,
        BrokerHandoffState::Cancelled
    );
    assert_eq!(store.get(&owner).await.unwrap().unwrap().cache, original);
}

#[tokio::test]
async fn finished_login_publishes_renewable_authority_before_it_becomes_satisfied() {
    let (manager, store, native, owner) = fixture(cache("account", 3600)).await;
    manager.start(&owner, "login-two").await.unwrap();
    let fresh = cache("subscribed-account", 3600);
    native
        .completion
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Ok(fresh.clone()))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while manager.status(&owner, "login-two").await.unwrap().state
            == BrokerHandoffState::Pending
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        manager.status(&owner, "login-two").await.unwrap().state,
        BrokerHandoffState::Satisfied
    );
    assert_eq!(store.get(&owner).await.unwrap().unwrap().cache, Some(fresh));
    assert_eq!(
        manager
            .tokens(&owner, false, None)
            .await
            .unwrap()
            .account_id,
        "subscribed-account"
    );
    assert_eq!(native.calls.load(Ordering::SeqCst), 0);
}
