//! Replica preflight preserves the account gate's typed refusal before ownership IO.
use super::{fixture::*, *};
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use layer2_core::{
    account_deletion::{AccountDeletionError, AccountStanding},
    SubjectId,
};
use std::sync::atomic::Ordering;
use tower::ServiceExt;

struct DeletingOwner;
#[async_trait::async_trait]
impl AccountStanding for DeletingOwner {
    async fn erasing(&self, subject: &SubjectId) -> Result<bool, AccountDeletionError> {
        Ok(subject.as_str() == "owner")
    }
}

#[tokio::test]
async fn deletion_preflight_returns_typed_problem_without_owner_lookup() {
    let auth = auth("owner");
    let mut f = fixture(auth.clone(), "agt-public", "owner").await;
    let access = Arc::new(
        ResourceAccessService::new(f.store.clone(), auth.clone())
            .with_account_standing(Arc::new(DeletingOwner)),
    );
    let reads = f.policy.reads.with_resource_access(access.clone());
    let (adapter, _) = layer3_adapter::codex::CodexAdapter::in_memory(Default::default());
    f.policy = PublicAccess::new(auth, reads, access, Arc::new(adapter));
    let (router, ownership) = router(&f).await;
    for (method, suffix) in [
        ("GET", "environment"),
        ("GET", "models"),
        ("POST", "files"),
        ("POST", "attachments"),
        ("PATCH", "preferences"),
        ("PATCH", "settings"),
    ] {
        let mut request = Request::builder()
            .method(method)
            .uri(format!("/api/v1/agents/agt-public/{suffix}"))
            .body(Body::empty())
            .unwrap();
        let context = context("owner");
        request
            .extensions_mut()
            .insert(layer1_public::AuthenticatedSubject(
                context.caller.subject.clone(),
            ));
        request
            .extensions_mut()
            .insert(context.caller.authorization.clone());
        request.extensions_mut().insert(context);
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            response.headers()["content-type"],
            "application/problem+json"
        );
        assert_eq!(response.headers()["cache-control"], "private, no-store");
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(
            body["code"], "account_deletion_in_progress",
            "{method} {suffix}"
        );
    }
    assert_eq!(ownership.calls.load(Ordering::SeqCst), 0);
}
