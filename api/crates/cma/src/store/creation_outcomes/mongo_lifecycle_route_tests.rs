use std::sync::Arc;

use axum::{
    body::{to_bytes, Body},
    http::Request,
    Extension, Router,
};
use layer1_public::rest::AgentLifecycleFailure;
use layer2_core::sandbox::{
    store::{SandboxStore, SandboxStoreError},
    SandboxOrigin, SandboxStatus,
};
use layer2_core::SandboxId;
use serde_json::Value;
use tower::ServiceExt;

use super::*;
use crate::store::{MongoAgentLifecycleStore, MongoSandboxStore};

const BODY: &str = r#"{"name":"Fixture lifecycle"}"#;

pub(super) async fn routes(
    store: &CreationOutcomes,
) -> (
    Router,
    Arc<MongoAgentLifecycleStore>,
    tokio::sync::OwnedSemaphorePermit,
) {
    let mut operations = MongoAgentLifecycleStore::open(&store.database)
        .await
        .unwrap();
    operations.creation_writer_epoch = Some(store.config.epoch.clone());
    operations.creation_terminal_settlement_enabled = store.config.terminal_settlement;
    let operations = Arc::new(operations);
    let (routes, slots) =
        crate::agent_lifecycle::creation_test_router(&store.database, operations.clone()).await;
    let backend: Arc<dyn Backend> = Arc::new(store.clone());
    (routes.layer(Extension(backend)), operations, slots)
}

fn lifecycle_query(
    store: &CreationOutcomes,
    owner: &SubjectId,
    day: i64,
    nonce: &str,
) -> LookupRequest {
    let mut request = query(store, owner, Operation::AgentBootstrap, day, nonce);
    request.fingerprint.digest = contract::wire_digest(BODY.as_bytes());
    let binding = contract::hash_fields(&[
        "cma-creation-key-v1",
        &store.config.epoch,
        &day.to_string(),
        nonce,
        owner.as_str(),
        store.deployment(),
        "POST",
        Operation::AgentBootstrap.path(),
        &request.fingerprint.digest,
    ]);
    request.idempotency_key = format!("cma1.{}.{day}.{nonce}.{binding}", store.config.epoch);
    request
}

pub(super) async fn call(app: &Router, request: Request<Body>, expected: StatusCode) -> Value {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 65_536).await.unwrap();
    assert_eq!(status, expected, "{}", String::from_utf8_lossy(&bytes));
    serde_json::from_slice(&bytes).unwrap()
}

async fn submit(app: &Router, request: &LookupRequest) -> Value {
    call(
        app,
        Request::post(Operation::AgentBootstrap.path())
            .header("content-type", "application/json")
            .header("Idempotency-Key", &request.idempotency_key)
            .body(Body::from(BODY))
            .unwrap(),
        StatusCode::ACCEPTED,
    )
    .await
}

async fn read_operation(app: &Router, id: &str) -> Value {
    call(
        app,
        Request::get(format!("/api/v1/agent-lifecycle/{id}"))
            .body(Body::empty())
            .unwrap(),
        StatusCode::OK,
    )
    .await
}

async fn sandbox_store(store: &CreationOutcomes) -> MongoSandboxStore {
    let mut sandboxes = MongoSandboxStore::open(store.database.client(), store.database.name())
        .await
        .unwrap();
    sandboxes.creation_writer_epoch = Some(store.config.epoch.clone());
    sandboxes
}

fn admission(id: &str, origin: SandboxOrigin, limit: u16) -> SandboxAdmission {
    let mut candidate =
        super::super::super::sandbox_admission_mongo_integration_tests::sandbox(id, "unused");
    candidate.origin = origin;
    let name = SandboxAdmissionName::Allocate(candidate.name.clone());
    SandboxAdmission::new(candidate, name, limit, layer2_core::now_rfc3339()).unwrap()
}

#[tokio::test]
#[ignore = "requires disposable CHRONO_TEST_MONGODB_URI"]
async fn original_create_replay_resumes_a_waiting_operation_without_allocating_again() {
    let (store, reader, today) = fixture().await;
    let owner = SubjectId::new("owner");
    let (app, operations, _slots) = routes(&store).await;
    let sandboxes = sandbox_store(&store).await;
    let request = lifecycle_query(&store, &owner, today, &"c".repeat(32));
    let first = submit(&app, &request).await;
    let id = first["id"].as_str().unwrap();
    let operation = operations.claim(id).await.unwrap().unwrap();
    let lease = operation.lease().unwrap();
    let mut fence = operation.creation_fence.unwrap();
    fence.lifecycle_lease = Some((id.to_owned(), lease.token.clone()));
    let mut effect = admission(
        "sbx_replay_fixture",
        SandboxOrigin::Lifecycle {
            operation_id: id.to_owned(),
        },
        10,
    );
    effect.creation_fence = Some(fence);
    sandboxes.admit_sandbox(effect).await.unwrap();
    operations
        .mark_sandbox(&lease, "sbx_replay_fixture")
        .await
        .unwrap();
    operations
        .wait_for_connection(
            &lease,
            vec!["fixture-service".into()],
            serde_json::json!({}),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        read_operation(&app, id).await["status"],
        "waiting_for_connection"
    );
    call(
        &app,
        Request::post(format!("/api/v1/agent-lifecycle/{id}/cancel"))
            .body(Body::empty())
            .unwrap(),
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(submit(&app, &request).await, first);
    assert_eq!(read_operation(&app, id).await["status"], "queued");
    assert_eq!(
        sandboxes
            .sandboxes
            .count_documents(doc! {"owner":owner.as_str()})
            .await
            .unwrap(),
        1
    );
    assert!(
        matches!(reader.lookup(&identity(&store, &owner, &request), &request).await.unwrap(),
        Outcome::Created { sandbox_id, operation_id: Some(operation_id), .. }
        if sandbox_id == "sbx_replay_fixture" && operation_id == id)
    );
    // Later retirement must preserve the exact created association, including
    // the original operation from which a consumer can reconcile retirement.
    let sandbox_id = SandboxId("sbx_replay_fixture".into());
    let mut retired = sandboxes.get_sandbox(&sandbox_id).await.unwrap().unwrap();
    retired.status = SandboxStatus::Retiring;
    retired.revision += 1;
    retired.updated_at = layer2_core::now_rfc3339();
    sandboxes
        .save_sandbox(retired.revision - 1, retired.clone())
        .await
        .unwrap();
    retired.status = SandboxStatus::Retired;
    retired.retired_at = Some(layer2_core::now_rfc3339());
    retired.revision += 1;
    sandboxes
        .save_sandbox(retired.revision - 1, retired)
        .await
        .unwrap();
    assert_eq!(
        sandboxes
            .get_sandbox(&sandbox_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SandboxStatus::Retired
    );
    assert!(
        matches!(reader.lookup(&identity(&store, &owner, &request), &request).await.unwrap(),
        Outcome::Created { sandbox_id, operation_id: Some(operation_id), .. }
        if sandbox_id == "sbx_replay_fixture" && operation_id == id)
    );
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable CHRONO_TEST_MONGODB_URI"]
async fn fenced_worker_quota_refusal_preserves_flagged_lifecycle_failure_metadata() {
    let (store, reader, today) = fixture().await;
    let owner = SubjectId::new("owner");
    let (app, operations, _slots) = routes(&store).await;
    let sandboxes = sandbox_store(&store).await;
    sandboxes
        .admit_sandbox(admission("sbx_fills_allowance", SandboxOrigin::Manual, 1))
        .await
        .unwrap();
    for (enabled, nonce) in [(false, "d".repeat(32)), (true, "e".repeat(32))] {
        let request = lifecycle_query(&store, &owner, today, &nonce);
        let receipt = submit(&app, &request).await;
        let id = receipt["id"].as_str().unwrap();
        let operation = operations.claim(id).await.unwrap().unwrap();
        let lease = operation.lease().unwrap();
        let mut fence = operation.creation_fence.unwrap();
        fence.lifecycle_lease = Some((id.to_owned(), lease.token.clone()));
        let mut effect = admission(
            "sbx_must_not_publish",
            SandboxOrigin::Lifecycle {
                operation_id: id.to_owned(),
            },
            1,
        );
        effect.creation_fence = Some(fence.clone());
        let error = sandboxes.admit_sandbox(effect.clone()).await.unwrap_err();
        let SandboxStoreError::LimitReached { limit, live, usage } = error else {
            panic!("expected typed quota refusal, got {error:?}")
        };
        let failure = enabled.then_some(AgentLifecycleFailure::SandboxLimitReached {
            limit,
            live: u64::from(live),
            quota_usage: Some(usage),
        });
        assert!(operations
            .fail_fenced_creation(
                &owner,
                &lease,
                &fence,
                &error.to_string(),
                failure,
                RefusalCode::SandboxLimitReached
            )
            .await
            .unwrap());
        let view = read_operation(&app, id).await;
        assert_eq!(view["status"], "failed");
        assert_eq!(
            view["failure"],
            if enabled {
                serde_json::json!({"code":"sandbox_limit_reached","limit":limit,"live":live,"quota_usage":usage})
            } else {
                Value::Null
            }
        );
        assert!(
            matches!(reader.lookup(&identity(&store, &owner, &request), &request).await.unwrap(),
            Outcome::Refused { code: RefusalCode::SandboxLimitReached, operation_id: Some(operation_id), .. } if operation_id == id)
        );
        effect.live_limit = 10;
        assert!(sandboxes.admit_sandbox(effect).await.is_err());
    }
    assert_eq!(
        sandboxes
            .sandboxes
            .count_documents(doc! {"owner":owner.as_str()})
            .await
            .unwrap(),
        1
    );
    store.database.drop().await.unwrap();
}

async fn cancel(app: &Router, id: &str, expected: StatusCode) -> Value {
    call(
        app,
        Request::post(format!("/api/v1/agent-lifecycle/{id}/cancel"))
            .body(Body::empty())
            .unwrap(),
        expected,
    )
    .await
}

#[tokio::test]
#[ignore = "requires disposable CHRONO_TEST_MONGODB_URI"]
async fn covered_connection_wait_cancellation_is_terminal_and_replayable() {
    let (store, reader, today) = fixture().await;
    let owner = SubjectId::new("owner");
    let (app, operations, _slots) = routes(&store).await;
    let request = lifecycle_query(&store, &owner, today, &"a".repeat(32));
    let receipt = submit(&app, &request).await;
    let id = receipt["id"].as_str().unwrap();
    let lease = operations
        .claim(id)
        .await
        .unwrap()
        .unwrap()
        .lease()
        .unwrap();
    operations
        .wait_for_connection(
            &lease,
            vec!["fixture-service".into()],
            serde_json::json!({}),
            None,
        )
        .await
        .unwrap();
    assert!(matches!(
        reader
            .lookup(&identity(&store, &owner, &request), &request)
            .await
            .unwrap(),
        Outcome::InProgress { .. }
    ));
    let (cancelled, resumed) = tokio::join!(
        cancel(&app, id, StatusCode::OK),
        operations.resume_connection(&owner, id)
    );
    resumed.unwrap();
    assert_eq!(cancelled["status"], "failed");
    assert_eq!(cancel(&app, id, StatusCode::OK).await["status"], "failed");
    assert_eq!(submit(&app, &request).await, receipt);
    assert!(operations.claim(id).await.unwrap().is_none());
    assert!(
        matches!(reader.lookup(&identity(&store, &owner, &request), &request).await.unwrap(), Outcome::Refused { code: RefusalCode::CreationCancelled, operation_id: Some(operation_id), .. } if operation_id==id)
    );
    assert_eq!(
        store
            .database
            .collection::<Document>("sandboxes")
            .count_documents(doc! {"owner":owner.as_str()})
            .await
            .unwrap(),
        0
    );
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable CHRONO_TEST_MONGODB_URI"]
async fn covered_cancellation_and_allocation_have_one_authoritative_outcome() {
    let (store, reader, today) = fixture().await;
    let owner = SubjectId::new("owner");
    let (app, operations, _slots) = routes(&store).await;
    let sandboxes = sandbox_store(&store).await;
    let request = lifecycle_query(&store, &owner, today, &"b".repeat(32));
    let receipt = submit(&app, &request).await;
    let id = receipt["id"].as_str().unwrap();
    let operation = operations.claim(id).await.unwrap().unwrap();
    let lease = operation.lease().unwrap();
    let mut fence = operation.creation_fence.clone().unwrap();
    fence.lifecycle_lease = Some((id.to_owned(), lease.token.clone()));
    let mut effect = admission(
        "sbx_cancel_race",
        SandboxOrigin::Lifecycle {
            operation_id: id.into(),
        },
        10,
    );
    effect.creation_fence = Some(fence);
    let (allocated, cancelled) = tokio::join!(
        sandboxes.admit_sandbox(effect),
        operations.cancel_fenced_creation(&owner, &operation)
    );
    let proof = reader
        .lookup(&identity(&store, &owner, &request), &request)
        .await
        .unwrap();
    match (allocated.is_ok(), cancelled.unwrap(), proof) {
        (true, false, Outcome::Created { sandbox_id, .. }) => {
            assert_eq!(sandbox_id, "sbx_cancel_race")
        }
        (
            false,
            true,
            Outcome::Refused {
                code: RefusalCode::CreationCancelled,
                ..
            },
        ) => assert!(operations.claim(id).await.unwrap().is_none()),
        _ => panic!("allocation and cancellation must have one committed outcome"),
    }
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable CHRONO_TEST_MONGODB_URI"]
async fn covered_nonquota_failure_emits_terminal_no_allocation_proof() {
    let (store, reader, today) = fixture().await;
    let owner = SubjectId::new("owner");
    let (app, operations, _slots) = routes(&store).await;
    let request = lifecycle_query(&store, &owner, today, &"f".repeat(32));
    let receipt = submit(&app, &request).await;
    let id = receipt["id"].as_str().unwrap();
    let operation = operations.claim(id).await.unwrap().unwrap();
    let lease = operation.lease().unwrap();
    let fence = operation.creation_fence.unwrap();
    assert!(operations
        .fail_fenced_creation(
            &owner,
            &lease,
            &fence,
            "Fixture terminal failure",
            None,
            RefusalCode::CreationFailed
        )
        .await
        .unwrap());
    assert_eq!(read_operation(&app, id).await["status"], "failed");
    assert!(
        matches!(reader.lookup(&identity(&store,&owner,&request),&request).await.unwrap(),Outcome::Refused { code:RefusalCode::CreationFailed, operation_id:Some(operation_id),.. } if operation_id==id)
    );
    // Treat the first successful write as an acknowledgement the caller lost.
    // A retry must neither allocate nor replace the durable refusal.
    assert!(!operations
        .fail_fenced_creation(
            &owner,
            &lease,
            &fence,
            "Repeated failure",
            None,
            RefusalCode::CreationFailed
        )
        .await
        .unwrap());
    assert_eq!(submit(&app, &request).await, receipt);
    assert!(operations.claim(id).await.unwrap().is_none());
    assert!(
        matches!(reader.lookup(&identity(&store,&owner,&request),&request).await.unwrap(),
        Outcome::Refused { code:RefusalCode::CreationFailed, operation_id:Some(operation_id),.. } if operation_id==id)
    );
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable CHRONO_TEST_MONGODB_URI"]
async fn covered_terminal_failure_from_previous_writer_is_reconciled() {
    let (store, reader, today) = fixture().await;
    let owner = SubjectId::new("owner");
    let (app, operations, _slots) = routes(&store).await;
    let request = lifecycle_query(&store, &owner, today, &"e".repeat(32));
    let receipt = submit(&app, &request).await;
    let id = receipt["id"].as_str().unwrap();
    let operation = operations.claim(id).await.unwrap().unwrap();
    // Waiting operations are excluded by the Failed-state index before pagination.
    for i in 0..40 {
        let waiting = lifecycle_query(&store, &owner, today, &format!("{i:032x}"));
        let waiting_receipt = submit(&app, &waiting).await;
        let waiting_id = waiting_receipt["id"].as_str().unwrap();
        let waiting_lease = operations
            .claim(waiting_id)
            .await
            .unwrap()
            .unwrap()
            .lease()
            .unwrap();
        operations
            .wait_for_connection(
                &waiting_lease,
                vec!["fixture-service".into()],
                serde_json::json!({}),
                None,
            )
            .await
            .unwrap();
    }
    operations
        .fail(&operation.lease().unwrap(), "Previous writer failure")
        .await
        .unwrap();
    assert!(matches!(
        reader
            .lookup(&identity(&store, &owner, &request), &request)
            .await
            .unwrap(),
        Outcome::Unknown { .. }
    ));
    store.settle_failed_linked(&mut None).await.unwrap();
    assert!(
        matches!(reader.lookup(&identity(&store,&owner,&request),&request).await.unwrap(),Outcome::Refused { code:RefusalCode::CreationFailed, operation_id:Some(operation_id),.. } if operation_id==id)
    );
    assert_eq!(
        read_operation(&app, id).await["error"],
        "Previous writer failure"
    );
    store.database.drop().await.unwrap();
}
