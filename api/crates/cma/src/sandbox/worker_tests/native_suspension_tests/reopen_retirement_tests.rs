use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
    Router,
};
use layer2_core::{OperationClaim, OperationReservation, OperationStore};
use layer3_adapter::agent_sandbox::NativePoolControl;
use serde_json::Value;
use tower::ServiceExt;

struct Operations;

#[async_trait::async_trait]
impl OperationStore for Operations {
    async fn reserve(&self, _: &OperationClaim) -> Result<OperationReservation, StoreError> {
        Ok(OperationReservation::Acquired)
    }

    async fn complete(&self, _: &OperationClaim, _: Vec<u8>) -> Result<(), StoreError> {
        Ok(())
    }
}

fn router(fixture: &Fixture) -> Router {
    use layer1_public::rest::{hierarchy_router, HierarchyState};
    use layer2_core::sandbox::{HierarchyReadService, WorkspaceService};
    hierarchy_router(HierarchyState::new(
        SandboxService::new(
            fixture.harness.store.clone(),
            Arc::new(InMemoryProfileStore::default()),
            fixture.harness.policy.clone(),
            Arc::new(layer2_core::sandbox::FixedSandboxCredentialPrerequisite::Ready),
        ),
        WorkspaceService::new(
            fixture.harness.store.clone(),
            Arc::new(InMemoryProfileStore::default()),
        ),
        HierarchyReadService::new(fixture.harness.store.clone()),
        Arc::new(Operations),
    ))
}

async fn public_call(
    app: &Router,
    id: &SandboxId,
    method: Method,
    subject: &str,
    expected: StatusCode,
) -> Value {
    let mut request = Request::builder()
        .method(method)
        .uri(format!("/api/v1/sandboxes/{id}"))
        .header("Idempotency-Key", uuid::Uuid::new_v4().to_string())
        .body(Body::empty())
        .unwrap();
    request
        .extensions_mut()
        .insert(layer1_public::AuthenticatedSubject(SubjectId::new(subject)));
    request.extensions_mut().insert(Authorization::new([
        Capability::UseOwnSandboxes,
        Capability::DeleteOwnSandbox,
    ]));
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    assert_eq!(status, expected, "{}", String::from_utf8_lossy(&bytes));
    serde_json::from_slice(&bytes).unwrap()
}

async fn retire_and_prove(
    fixture: &Fixture,
    before: &Sandbox,
    agent: &layer2_core::sandbox::Agent,
    mut now: SystemTime,
) {
    let owner = caller("owner", [Capability::DeleteOwnSandbox]);
    assert!(fixture
        .harness
        .service
        .delete(&caller("other", [Capability::DeleteOwnSandbox]), &before.id)
        .await
        .is_err());
    let retiring = fixture
        .harness
        .service
        .delete(&owner, &before.id)
        .await
        .unwrap();
    assert_eq!(retiring.status, SandboxStatus::Retiring);
    assert!(retiring.work.lease.is_none());
    assert!(retiring.lifecycle.operation.is_none());
    assert_eq!(
        fixture
            .harness
            .service
            .delete(&owner, &before.id)
            .await
            .unwrap(),
        retiring
    );
    fixture.backend.hold_delete.store(true, Ordering::SeqCst);
    for _ in 0..8 {
        now += Duration::from_secs(6);
        fixture.step(&before.id, now, &runtime_image()).await;
        assert!(
            !fixture
                .harness
                .stored(&before.id)
                .await
                .lifecycle
                .home
                .as_ref()
                .unwrap()
                .released
        );
    }
    fixture.backend.hold_delete.store(false, Ordering::SeqCst);
    fixture.backend.lose_delete.store(true, Ordering::SeqCst);
    let retired = fixture
        .drive(
            &before.id,
            &mut now,
            SandboxStatus::Retired,
            &runtime_image(),
        )
        .await;
    assert!(retired.runtime.is_none() && retired.lifecycle.suspended_runtime.is_none());
    assert!(retired.work.lease.is_none());
    assert!(retired.lifecycle.home.as_ref().unwrap().released);
    assert!(
        retired
            .lifecycle
            .home
            .as_ref()
            .unwrap()
            .native_pool
            .as_ref()
            .unwrap()
            .cleaned
    );
    assert!(fixture
        .backend
        .pools
        .as_ref()
        .unwrap()
        .inventory_claims()
        .await
        .unwrap()
        .is_empty());
    assert!(fixture
        .adapter(&runtime_image())
        .runtime_inventory()
        .await
        .unwrap()
        .is_empty());
    let stopped = fixture
        .harness
        .store
        .get_agent(&agent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stopped.status, AgentStatus::Stopped);
    assert!(stopped.work.lease.is_none());
    assert_eq!(
        stopped.conversation.conversation_id,
        agent.conversation.conversation_id
    );
    assert_eq!(
        fixture
            .harness
            .service
            .delete(&owner, &before.id)
            .await
            .unwrap(),
        retired
    );
    assert_eq!(
        fixture
            .backend
            .calls
            .snapshot()
            .iter()
            .filter(|c| c.as_str() == "delete-effective")
            .count(),
        1
    );
}

#[tokio::test]
async fn conversation_reopen_rejection_then_owner_delete_converges_after_lost_reply() {
    let (fixture, reopening, agent, mut now) = retained_reopen_tests::restored_reopen().await;
    let app = router(&fixture);
    let before = public_call(&app, &reopening.id, Method::GET, "owner", StatusCode::OK).await;
    fixture.backend.reject_reopen.store(true, Ordering::SeqCst);
    now += Duration::from_secs(6);
    fixture.step(&reopening.id, now, &runtime_image()).await;
    let failed = fixture.harness.stored(&reopening.id).await;
    assert_eq!(failed.status, SandboxStatus::Failed);
    assert_eq!(failed.failure.as_ref().unwrap().class, "restore_failed");
    assert_eq!(failed.lifecycle.home, reopening.lifecycle.home);
    let body = public_call(&app, &failed.id, Method::GET, "owner", StatusCode::OK).await;
    public_call(
        &app,
        &failed.id,
        Method::GET,
        "other",
        StatusCode::NOT_FOUND,
    )
    .await;
    assert!(body["lifecycle"].get("failed_operation").is_none());
    let result = &body["lifecycle"]["result"];
    assert_eq!(result["state"], "failed");
    assert_eq!(result["code"], "lifecycle_failed");
    assert_eq!(result["continuing"], false);
    assert_eq!(result["actions"], serde_json::json!(["observe", "retire"]));
    assert!(result.get("retry_after_seconds").is_none());
    for key in ["operation_id", "started_at", "result_due_at"] {
        assert_eq!(result[key], before["lifecycle"]["result"][key]);
    }
    let replica = router(&fixture);
    assert_eq!(
        public_call(&replica, &failed.id, Method::GET, "owner", StatusCode::OK).await,
        body
    );
    assert_eq!(
        fixture
            .harness
            .store
            .get_agent(&agent.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Paused
    );
    retire_and_prove(&fixture, &failed, &agent, now).await;
}

#[tokio::test]
async fn owner_delete_fences_a_silent_inflight_reopen_and_competing_worker() {
    let (fixture, reopening, agent, mut now) = retained_reopen_tests::restored_reopen().await;
    fixture.backend.hold_reopen.store(true, Ordering::SeqCst);
    let worker = fixture
        .harness
        .worker_with_adapter("stalled-reopen", fixture.adapter(&runtime_image()));
    let id = reopening.id.clone();
    now += Duration::from_secs(6);
    let task = tokio::spawn(async move { worker.run_one(&id, now).await });
    tokio::time::timeout(
        Duration::from_secs(3),
        fixture.backend.reopen_entered.notified(),
    )
    .await
    .unwrap();
    let competing = fixture
        .harness
        .worker_with_adapter("competitor", fixture.adapter(&runtime_image()));
    assert!(!competing.run_one(&reopening.id, now).await.unwrap());
    let app = router(&fixture);
    let observing = public_call(&app, &reopening.id, Method::GET, "owner", StatusCode::OK).await;
    assert_eq!(
        observing["lifecycle"]["result"]["actions"],
        serde_json::json!(["observe", "retire"])
    );
    public_call(
        &app,
        &reopening.id,
        Method::DELETE,
        "other",
        StatusCode::NOT_FOUND,
    )
    .await;
    let receipt = tokio::time::timeout(
        Duration::from_secs(3),
        public_call(
            &app,
            &reopening.id,
            Method::DELETE,
            "owner",
            StatusCode::ACCEPTED,
        ),
    )
    .await
    .expect("owner DELETE must not wait for the blocked reopen");
    assert_eq!(receipt["status"], "retiring");
    fixture.backend.reopen_release.notify_one();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap(),
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(
        fixture
            .harness
            .store
            .get_agent(&agent.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Paused
    );
    retire_and_prove(&fixture, &reopening, &agent, now).await;
}
