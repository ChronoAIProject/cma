//! A failed custody read after real policy work is unavailable, not revoked.
use super::*;
use crate::auth::persistence::{AuthStore, MemoryAuthStore, SecretValue};
use axum::{body::Body, http::Request};
use layer2_core::resource_access::*;
use layer2_core::sandbox::{store::InMemorySandboxStore, AgentListQuery, OwnerSandboxQuery};
use layer2_core::SubjectId;
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

#[derive(Clone, Copy)]
enum CustodyChange {
    Outage,
    Revoke,
    Replace,
    Expire,
}

struct PolicyStore {
    inner: Arc<InMemorySandboxStore>,
    auth: Arc<MemoryAuthStore>,
    reads: AtomicUsize,
    change: CustodyChange,
}

#[async_trait::async_trait]
impl ResourceAccessStore for PolicyStore {
    async fn resource_access_snapshot(
        &self,
        target: &ResourceTarget,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
        let snapshot = self.inner.resource_access_snapshot(target).await?;
        // The serving policy performs an admission snapshot and its final
        // current-snapshot check before RelayCaller::current reads custody.
        if self.reads.fetch_add(1, Ordering::SeqCst) == 1 {
            if !matches!(self.change, CustodyChange::Outage) {
                let mut state = self
                    .auth
                    .subject(&SubjectId::new("owner"))
                    .await
                    .unwrap()
                    .unwrap();
                match self.change {
                    CustodyChange::Revoke => state.tokens = None,
                    CustodyChange::Replace => {
                        state.tokens.as_mut().unwrap().access_token =
                            SecretValue::new("replacement".into())
                    }
                    CustodyChange::Expire => {
                        state.tokens.as_mut().unwrap().expires_at =
                            SystemTime::now() - std::time::Duration::from_secs(1)
                    }
                    CustodyChange::Outage => unreachable!(),
                }
                self.auth.put_subject(state).await.unwrap();
            } else {
                self.auth.set_unavailable(true);
            }
        }
        Ok(snapshot)
    }
    async fn organization_sandboxes(
        &self,
        _: &SubjectId,
        _: &OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        panic!("personal policy cannot read organization inventory")
    }
    async fn shared_sandboxes(
        &self,
        _: &SubjectId,
        _: &[SubjectId],
        _: &OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        panic!("personal policy cannot read shared inventory")
    }
    async fn organization_agents(
        &self,
        _: &SubjectId,
        _: &AgentListQuery,
        _: &layer2_core::sandbox::store::PageRequest,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        panic!("personal policy cannot read organization inventory")
    }
}

#[async_trait::async_trait]
impl ResourceShareStore for PolicyStore {
    async fn share_mutation_receipt(
        &self,
        _: &str,
    ) -> Result<Option<ShareMutationReceipt>, ResourceAccessStoreError> {
        panic!("read policy cannot mutate sharing")
    }
    async fn mutate_share(
        &self,
        _: ShareMutation,
    ) -> Result<ShareMutationReceipt, ResourceAccessStoreError> {
        panic!("read policy cannot mutate sharing")
    }
    async fn resource_share(
        &self,
        _: &str,
    ) -> Result<Option<ResourceShare>, ResourceAccessStoreError> {
        panic!("personal policy cannot read share management records")
    }
}

#[tokio::test]
async fn post_policy_custody_outage_is_503_without_ownership_or_effects() {
    custody_change("GET", "environment", CustodyChange::Outage).await;
}

#[tokio::test]
async fn preferences_policy_io_cannot_outlive_credential_custody() {
    custody_change("PATCH", "preferences", CustodyChange::Outage).await;
    custody_change("PATCH", "preferences", CustodyChange::Revoke).await;
}

#[tokio::test]
async fn settings_policy_io_cannot_outlive_credential_custody() {
    for change in [
        CustodyChange::Outage,
        CustodyChange::Revoke,
        CustodyChange::Replace,
        CustodyChange::Expire,
    ] {
        custody_change("PATCH", "settings", change).await;
    }
}

async fn custody_change(method: &str, suffix: &str, change: CustodyChange) {
    let browser = crate::auth::relay::tests::Fixture::new(true).await;
    let mut f = fixture::fixture(browser.auth.clone(), "agt-public", "owner").await;
    let policy_store = Arc::new(PolicyStore {
        inner: f.store.clone(),
        auth: browser.store.clone(),
        reads: AtomicUsize::new(0),
        change,
    });
    let access = Arc::new(ResourceAccessService::new(
        policy_store.clone(),
        browser.auth.clone(),
    ));
    let (adapter, _) = layer3_adapter::codex::CodexAdapter::in_memory(Default::default());
    f.policy = PublicAccess::new(
        browser.auth.clone(),
        f.policy.reads.clone(),
        access,
        Arc::new(adapter),
    );
    let (router, ownership) = fixture::router(&f).await;
    let mut request = Request::builder()
        .method(method)
        .uri(format!("/api/v1/agents/agt-public/{suffix}"))
        .body(Body::empty())
        .unwrap();
    *request.headers_mut() = browser.headers.clone();
    request
        .extensions_mut()
        .insert(layer1_public::AuthenticatedSubject(
            browser.context.caller.subject.clone(),
        ));
    request.extensions_mut().insert(browser.context.clone());
    request
        .extensions_mut()
        .insert(layer1_public::DelegatedCredential::new(
            "test-access".into(),
        ));
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(
        response.status(),
        if !matches!(change, CustodyChange::Outage) {
            StatusCode::UNAUTHORIZED
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        }
    );
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    assert_eq!(
        response.headers()["content-type"],
        "application/problem+json"
    );
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
        if !matches!(change, CustodyChange::Outage) {
            "authentication_required"
        } else {
            "resource_access_unavailable"
        }
    );
    assert_eq!(policy_store.reads.load(Ordering::SeqCst), 2);
    assert_eq!(ownership.calls.load(Ordering::SeqCst), 0);
    // The fixture handler panics if any denied request reaches an effect.
}
