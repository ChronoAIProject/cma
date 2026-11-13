//! Delegation carries verified user authority into the actual hierarchy handlers.
use super::*;
use layer2_core::{
    profiles::InMemoryProfileStore,
    sandbox::{
        store::SandboxStore, FixedSandboxCredentialPrerequisite, HierarchyReadService,
        SandboxPolicy, SandboxService, WorkspaceService,
    },
    OperationClaim, OperationReservation, OperationStore, StoreError,
};

struct DenialReceipts;

#[async_trait::async_trait]
impl OperationStore for DenialReceipts {
    async fn reserve(&self, claim: &OperationClaim) -> Result<OperationReservation, StoreError> {
        assert_eq!(claim.subject().as_str(), "visitor");
        Ok(OperationReservation::Acquired)
    }

    async fn complete(&self, claim: &OperationClaim, outcome: Vec<u8>) -> Result<(), StoreError> {
        // A caller may retain its own denial receipt, never a successful foreign effect.
        assert_eq!(claim.subject().as_str(), "visitor");
        assert_eq!(
            serde_json::from_slice::<Value>(&outcome).unwrap()["status"],
            404
        );
        Ok(())
    }
}

#[tokio::test]
async fn tenant_isolation_delegated_targets_and_revoked_grants_are_checked_before_dispatch() {
    use crate::permissions::{capabilities_from_permissions, permission};
    let f = Fixture::with_permissions(&[
        permission::SANDBOX_CREATE,
        permission::SANDBOX_USE,
        permission::SANDBOX_DELETE,
    ])
    .await;
    let authority = capabilities_from_permissions(&f.peer.permissions);
    assert!(!authority.allows(layer2_core::Capability::ReadAllSandboxes));
    assert!(!authority.allows(layer2_core::Capability::ControlAnySandbox));
    let notification = f.activate().await;
    let own = crate::auth::test_resources::sandbox("sbx_visitor", "visitor");
    f.store.create_sandbox(own.clone()).await.unwrap();
    let profiles = Arc::new(InMemoryProfileStore::default());
    let access = Arc::new(layer2_core::resource_access::ResourceAccessService::new(
        f.store.clone(),
        f.authority.auth.clone(),
    ));
    let state = layer1_public::rest::HierarchyState::new(
        SandboxService::new(
            f.store.clone(),
            profiles.clone(),
            SandboxPolicy::default(),
            Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        ),
        WorkspaceService::new(f.store.clone(), profiles),
        HierarchyReadService::new(f.store.clone()).with_resource_access(access),
        Arc::new(DenialReceipts),
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let resources = layer1_public::rest::hierarchy_router(state).layer(axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            observed.fetch_add(1, Ordering::SeqCst);
            async move { next.run(request).await }
        },
    ));
    let original = f.store.get_sandbox(&f.binding.sandbox_id).await.unwrap();
    for (method, suffix) in [
        ("GET", ""),
        ("GET", "/workspaces"),
        ("POST", "/pause"),
        ("DELETE", ""),
    ] {
        let mut denied = None;
        for id in [f.binding.sandbox_id.as_str(), "sbx_absent"] {
            let response = request(
                &f,
                resources.clone(),
                "/api/v1/agent-delegations/operations",
                json!({
                    "event_ref":capability(&notification), "method":method,
                    "path":format!("/api/v1/sandboxes/{id}{suffix}"),
                    "idempotency_key":format!("isolation-{method}-{id}"), "body":{}
                }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let result = json_body(response).await;
            assert_eq!(result["status"], 404, "{method} {suffix}");
            assert_eq!(result["body"]["code"], "sandbox_not_found");
            if let Some(previous) = &denied {
                assert_eq!(previous, &result);
            }
            denied = Some(result);
        }
    }
    assert_eq!(
        f.store.get_sandbox(&f.binding.sandbox_id).await.unwrap(),
        original
    );
    for path in [
        "/api/v1/sandboxes/sbx_visitor",
        "/api/v1/sandboxes/sbx_visitor/workspaces",
        "/api/v1/sandboxes",
    ] {
        let response = request(
            &f,
            resources.clone(),
            "/api/v1/agent-delegations/operations",
            json!({
                "event_ref":capability(&notification), "method":"GET", "path":path
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let result = json_body(response).await;
        assert_eq!(result["status"], 200);
        assert!(!result.to_string().contains(f.binding.sandbox_id.as_str()));
    }
    // A warmed grant cannot keep dispatching after revocation, including writes.
    f.authority
        .auth
        .revoke_actor_link(&notification.actor, &notification.grant.grant_id)
        .await
        .unwrap();
    let before = calls.load(Ordering::SeqCst);
    for method in ["GET", "DELETE"] {
        let response = request(
            &f,
            resources.clone(),
            "/api/v1/agent-delegations/operations",
            json!({
                "event_ref":capability(&notification), "method":method,
                "path":"/api/v1/sandboxes/sbx_visitor", "idempotency_key":"stale-grant"
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        json_body(response).await;
    }
    assert_eq!(calls.load(Ordering::SeqCst), before);
    assert_eq!(f.store.get_sandbox(&own.id).await.unwrap(), Some(own));
}
