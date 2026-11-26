mod terminal_event_tests;
use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
    Router,
};
use layer2_core::profiles::InMemoryProfileStore;
use layer2_core::sandbox::store::{
    SandboxLifecycleAgentScope, SandboxLifecycleExecutionStore, SandboxLifecycleMutation,
    SandboxMutationGuard,
};
use layer2_core::sandbox::{
    FixedSandboxCredentialPrerequisite, HierarchyReadService, SandboxLifecycleResult,
    SandboxPolicy, SandboxReviveCause, SandboxRotationIdentity, SandboxService,
};
use layer2_core::{OperationClaim, OperationReservation, OperationStore, StoreError};
use serde_json::Value;
use std::time::{Duration, SystemTime};
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

fn router(store: Arc<MongoSandboxStore>) -> Router {
    let profiles = Arc::new(InMemoryProfileStore::default());
    layer1_public::rest::hierarchy_router(layer1_public::rest::HierarchyState::new(
        SandboxService::new(
            store.clone(),
            profiles.clone(),
            SandboxPolicy::default(),
            Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        ),
        WorkspaceService::new(store.clone(), profiles),
        HierarchyReadService::new(store),
        Arc::new(Operations),
    ))
}

async fn call(
    app: &Router,
    id: &SandboxId,
    method: Method,
    owner: &str,
    expected: StatusCode,
) -> Value {
    call_path(
        app,
        &format!("/api/v1/sandboxes/{id}"),
        method,
        owner,
        expected,
    )
    .await
}

async fn call_path(
    app: &Router,
    path: &str,
    method: Method,
    owner: &str,
    expected: StatusCode,
) -> Value {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("Idempotency-Key", uuid::Uuid::new_v4().to_string())
        .body(Body::empty())
        .unwrap();
    request
        .extensions_mut()
        .insert(layer1_public::AuthenticatedSubject(SubjectId(owner.into())));
    request.extensions_mut().insert(Authorization::new([
        Capability::UseOwnSandboxes,
        Capability::DeleteOwnSandbox,
    ]));
    let response = tokio::time::timeout(Duration::from_secs(3), app.clone().oneshot(request))
        .await
        .unwrap()
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    assert_eq!(status, expected, "{}", String::from_utf8_lossy(&bytes));
    serde_json::from_slice(&bytes).unwrap()
}

pub(in crate::store::sandbox_mongo_integration_tests) async fn fail(
    store: &MongoSandboxStore,
    id: &SandboxId,
    at: &str,
) -> Sandbox {
    let claimed = store
        .claim_sandbox_work(id, "worker", at.into(), work_lease_deadline())
        .await
        .unwrap()
        .unwrap();
    let mut failed = claimed.clone();
    let failure = failed
        .fail_lifecycle(
            "runtime rejected the lifecycle".into(),
            "worker",
            claimed.work.lease.as_ref().unwrap().fence,
            at.into(),
        )
        .unwrap();
    store
        .commit_sandbox_lifecycle(SandboxLifecycleMutation {
            guard: SandboxMutationGuard::capture(&claimed),
            sandbox: failed.clone(),
            agents: vec![],
            agent_scope: SandboxLifecycleAgentScope::Touched,
            appended_at: at.into(),
            events: vec![SandboxEventKind::Failed { failure }],
        })
        .await
        .unwrap();
    failed
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn failed_lifecycle_history_survives_fresh_readers_and_owner_retirement() {
    for status in [
        SandboxStatus::Pausing,
        SandboxStatus::Reviving,
        SandboxStatus::Rotating,
    ] {
        let (client, name) = database("failed_lifecycle").await;
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let mut original = sandbox("sbx_failed_lifecycle");
        original.work.step = SandboxWorkStep::None;
        if status == SandboxStatus::Reviving {
            original.status = SandboxStatus::Paused;
            original.lifecycle.checkpoints.latest = Some(super::checkpoint_for_agents(&[], 1));
        }
        store.create_sandbox(original.clone()).await.unwrap();
        let mut admitted = original.clone();
        match status {
            SandboxStatus::Pausing => admitted.begin_pause(MINUTE.into(), None).unwrap(),
            SandboxStatus::Reviving => admitted
                .begin_revive(SandboxReviveCause::ExplicitRequest, MINUTE.into())
                .unwrap(),
            SandboxStatus::Rotating => {
                let identity = SandboxRotationIdentity::capture(&admitted, &[]).unwrap();
                admitted
                    .begin_image_rotation("runtime-next".into(), identity, MINUTE.into())
                    .unwrap();
            }
            _ => unreachable!(),
        }
        store
            .commit(SandboxTransaction::SaveSandboxAndAppend {
                command_revival: None,
                released_runtime: None,
                expected_revision: original.revision,
                sandbox: Box::new(admitted.clone()),
                event_at: MINUTE.into(),
                event: SandboxEventKind::StatusChanged {
                    from: original.status,
                    to: status,
                },
            })
            .await
            .unwrap();
        let before =
            SandboxLifecycleResult::observe(&admitted, None, SystemTime::now(), true).unwrap();
        let failed = fail(&store, &admitted.id, TWO_MINUTES).await;
        let row = store
            .sandboxes
            .find_one(doc! {"_id": failed.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        assert!(!row
            .get_document("record")
            .unwrap()
            .get_document("lifecycle")
            .unwrap()
            .contains_key("failed_operation"));
        let mut observed = None;
        for _ in 0..2 {
            let replica = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
            let retained = replica.get_sandbox(&failed.id).await.unwrap().unwrap();
            assert_eq!(retained, failed);
            let app = router(replica);
            let body = call(&app, &failed.id, Method::GET, "owner", StatusCode::OK).await;
            terminal_event_tests::assert_public_terminal(&app, &failed).await;
            let result = &body["lifecycle"]["result"];
            assert_eq!(result["state"], "failed");
            assert_eq!(result["code"], "lifecycle_failed");
            assert_eq!(result["continuing"], false);
            assert!(result.get("retry_after_seconds").is_none());
            let before = serde_json::to_value(&before).unwrap();
            for key in ["operation_id", "started_at", "result_due_at"] {
                assert_eq!(result[key], before[key]);
            }
            if let Some(previous) = observed.replace(body.clone()) {
                assert_eq!(previous, body);
            }
            call(
                &app,
                &failed.id,
                Method::GET,
                "other",
                StatusCode::NOT_FOUND,
            )
            .await;
            call(
                &app,
                &failed.id,
                Method::DELETE,
                "other",
                StatusCode::NOT_FOUND,
            )
            .await;
        }
        assert_eq!(
            store
                .sandboxes
                .find_one(doc! {"_id": failed.id.as_str()})
                .await
                .unwrap()
                .unwrap(),
            row
        );
        // A predecessor-shaped writer still uses the same CAS; a fresh observer
        // cannot attach a successor's history to the failed snapshot it was given.
        let replica = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
        let app = router(replica.clone());
        let (observed, retired) = tokio::join!(
            store.failed_lifecycle_admission(&failed),
            call(
                &app,
                &failed.id,
                Method::DELETE,
                "owner",
                StatusCode::ACCEPTED
            ),
        );
        if let Some(admission) = observed.unwrap() {
            assert_eq!(Some(admission.operation_id), before.operation_id);
            assert_eq!(Some(admission.requested_at), before.started_at);
        }
        assert_eq!(retired["status"], "retiring");
        assert_eq!(
            store.failed_lifecycle_admission(&failed).await.unwrap(),
            None
        );
        let mut stale = failed.clone();
        stale.revision += 1;
        assert_eq!(
            store.save_sandbox(failed.revision, stale).await,
            Err(SandboxStoreError::Conflict)
        );
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retained_failure_without_history_is_explicit_stable_and_deletable() {
    let (client, name) = database("failed_legacy").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut legacy = sandbox("sbx_legacy_failure");
    legacy.work.step = SandboxWorkStep::None;
    legacy.begin_pause(MINUTE.into(), None).unwrap();
    legacy
        .work
        .claim("worker".into(), TWO_MINUTES, work_lease_deadline())
        .unwrap();
    legacy
        .fail_lifecycle(
            "historical failure".into(),
            "worker",
            legacy.work.lease.as_ref().unwrap().fence,
            TWO_MINUTES.into(),
        )
        .unwrap();
    // A retained predecessor row whose event history cannot establish admission.
    legacy.revision = 1;
    store.create_sandbox(legacy.clone()).await.unwrap();
    let mut previous = None;
    for _ in 0..2 {
        let app = router(Arc::new(
            MongoSandboxStore::open(&client, &name).await.unwrap(),
        ));
        let body = call(&app, &legacy.id, Method::GET, "owner", StatusCode::OK).await;
        let result = &body["lifecycle"]["result"];
        assert_eq!(result["state"], "failed");
        assert_eq!(result["code"], "lifecycle_failed_admission_unavailable");
        assert_eq!(result["continuing"], false);
        assert_eq!(result["actions"], serde_json::json!(["observe", "retire"]));
        for field in [
            "operation_id",
            "started_at",
            "result_due_at",
            "retry_after_seconds",
        ] {
            assert!(result.get(field).is_none());
        }
        if let Some(previous) = previous.replace(body.clone()) {
            assert_eq!(previous, body);
        }
        call(
            &app,
            &legacy.id,
            Method::GET,
            "other",
            StatusCode::NOT_FOUND,
        )
        .await;
    }
    let app = router(Arc::new(
        MongoSandboxStore::open(&client, &name).await.unwrap(),
    ));
    call(
        &app,
        &legacy.id,
        Method::DELETE,
        "owner",
        StatusCode::ACCEPTED,
    )
    .await;
    client.database(&name).drop().await.unwrap();
}
