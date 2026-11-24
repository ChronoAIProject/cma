use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
    Router,
};
use layer2_core::{OperationClaim, OperationReservation, OperationStore};
use layer3_adapter::agent_sandbox::NativePoolControl;
use layer3_adapter::HomeVolumeStore;
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
    let pools = fixture.backend.pools.as_ref().unwrap();
    let original_home = before.lifecycle.home.as_ref().unwrap();
    let attachment = original_home.attachment().unwrap();
    let original_pool = original_home.native_pool.as_ref().unwrap();
    let claim = original_pool.claim.as_ref().unwrap();
    let before_loss = fixture.harness.stored(&before.id).await;
    let attempts_before_loss = fixture.backend.calls.snapshot().len();
    fixture.backend.lose_delete.store(true, Ordering::SeqCst);
    now = (now + Duration::from_secs(6)).max(stamp_time(&before_loss.work.ready_at));
    fixture.step(&before.id, now, &runtime_image()).await;
    assert!(!fixture.backend.lose_delete.load(Ordering::SeqCst));
    let calls_after_loss = fixture.backend.calls.snapshot();
    assert_eq!(
        deletion_counts(&calls_after_loss[attempts_before_loss..]),
        (1, 1),
        "the lost acknowledgement followed one effective deletion"
    );

    let pending = fixture.harness.stored(&before.id).await;
    assert_eq!(pending.status, SandboxStatus::Retiring);
    assert_eq!(pending.owner, before.owner);
    assert_eq!(pending.runtime, before_loss.runtime);
    assert_eq!(
        pending.lifecycle.suspended_runtime,
        before_loss.lifecycle.suspended_runtime
    );
    assert_eq!(pending.lifecycle.home, before_loss.lifecycle.home);
    assert!(fixture
        .adapter(&runtime_image())
        .runtime_inventory()
        .await
        .unwrap()
        .is_empty());
    assert_eq!(pools.inventory_claims().await.unwrap(), vec![claim.clone()]);
    assert_eq!(
        HomeVolumeStore::inventory(pools.as_ref()).await.unwrap(),
        vec![attachment]
    );

    // Drive constructs fresh workers and adapters and reads the retained retirement state.
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
    assert_eq!(retired.id, before.id);
    assert_eq!(retired.owner, before.owner);
    let retired_home = retired.lifecycle.home.as_ref().unwrap();
    assert!(retired_home.released);
    assert_eq!(retired_home.request, original_home.request);
    assert_eq!(retired_home.receipt, original_home.receipt);
    let retired_pool = retired_home.native_pool.as_ref().unwrap();
    assert!(retired_pool.cleaned);
    assert_eq!(retired_pool.request, original_pool.request);
    assert_eq!(retired_pool.claim, original_pool.claim);
    assert!(pools.inventory_claims().await.unwrap().is_empty());
    assert!(HomeVolumeStore::inventory(pools.as_ref())
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
    let calls = fixture.backend.calls.snapshot();
    let (attempts, effects) = deletion_counts(&calls[calls_after_loss.len()..]);
    assert_eq!(effects, 0, "the successor must not delete another runtime");
    assert_eq!(deletion_counts(&calls).1, 1);
    eprintln!("retirement attempts across worker lifetimes: first=1, successor={attempts}");
}

fn deletion_counts(calls: &[String]) -> (usize, usize) {
    let attempts = calls
        .iter()
        .filter(|call| call.as_str() == "delete-retained")
        .count();
    let effects = calls
        .iter()
        .filter(|call| call.as_str() == "delete-effective")
        .count();
    (attempts, effects)
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

mod local_lifecycle_tests;
