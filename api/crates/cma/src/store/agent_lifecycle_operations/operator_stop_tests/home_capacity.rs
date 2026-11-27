use super::*;
use crate::store::{MongoRegistrationCapacityStore, MongoSandboxStore};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use layer2_core::{
    admission::AdmissionCeilings,
    sandbox::{
        store::{SandboxAdmission, SandboxAdmissionName, SandboxStore},
        SandboxOrigin,
    },
};
use std::sync::Arc;
use tower::ServiceExt;

async fn with_ceiling(client: &Client, name: &str) -> MongoSandboxStore {
    let sandboxes = MongoSandboxStore::open(client, name).await.unwrap();
    MongoRegistrationCapacityStore::open(client, name, true)
        .await
        .unwrap();
    crate::store::settings_barrier::test_support::set_ceilings(
        client,
        name,
        AdmissionCeilings {
            retained_homes: Some(1),
            ..Default::default()
        },
    )
    .await;
    sandboxes
}

fn home(id: &str, owner: &SubjectId, operation: &str) -> SandboxAdmission {
    let mut sandbox =
        crate::store::sandbox_admission_mongo_integration_tests::sandbox(id, "unused");
    sandbox.owner = owner.clone();
    sandbox.origin = SandboxOrigin::Lifecycle {
        operation_id: operation.into(),
    };
    SandboxAdmission::new(
        sandbox,
        SandboxAdmissionName::Allocate("fixture".into()),
        8,
        now_rfc3339(),
    )
    .unwrap()
}

async fn submit(app: &Router, key: &str) -> (StatusCode, Option<String>, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::post(layer1_public::rest::creation_outcomes::Operation::AgentBootstrap.path())
                .header("content-type", "application/json")
                .header("idempotency-key", key)
                .body(Body::from(r#"{"name":"Fixture lifecycle"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let retry = response
        .headers()
        .get("retry-after")
        .map(|v| v.to_str().unwrap().to_owned());
    let body = to_bytes(response.into_body(), 65_536).await.unwrap();
    (status, retry, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn lifecycle_ceiling_one_refuses_second_home_and_preserves_exact_replay() {
    let (client, name, store) = fixture().await;
    let database = client.database(&name);
    let sandboxes = with_ceiling(&client, &name).await;
    let store = Arc::new(store);
    let (app, _slots) =
        crate::agent_lifecycle::creation_test_router(&database, store.clone()).await;
    let (status, _, first) = submit(&app, "first-home").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{first:?}");
    let id = first["id"].as_str().unwrap();
    sandboxes
        .admit_sandbox(home("sbx_first_home", &SubjectId::new("owner"), id))
        .await
        .unwrap();

    for _ in 0..2 {
        let (status, retry, body) = submit(&app, "second-home").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body:?}");
        assert_eq!(body["code"], "sandbox_capacity_unavailable");
        assert_eq!(retry.as_deref(), Some("60"));
    }
    let (status, _, replay) = submit(&app, "first-home").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{replay:?}");
    assert_eq!(replay["id"], id);
    assert_eq!(store.operations.count_documents(doc! {}).await.unwrap(), 1);
    assert_eq!(
        sandboxes.sandboxes.count_documents(doc! {}).await.unwrap(),
        1
    );
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn lifecycle_capacity_race_uses_the_shared_fence_after_operator_stop() {
    let (client, name, first) = fixture().await;
    let sandboxes = with_ceiling(&client, &name).await;
    let peer = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let peer_sandboxes = MongoSandboxStore::open(&peer, &name).await.unwrap();
    let second = MongoAgentLifecycleStore::open(&peer.database(&name))
        .await
        .unwrap();
    let alice = SubjectId::new("alice");
    let bob = SubjectId::new("bob");
    let (a, b) = tokio::join!(
        first.create(&alice, "alice-home", input()),
        second.create(&bob, "bob-home", input())
    );
    let CreateResult::Created(a) = a.unwrap() else {
        panic!("expected a fresh operation")
    };
    let CreateResult::Created(b) = b.unwrap() else {
        panic!("expected a fresh operation")
    };
    stop(&client, &name).await;

    let (a_home, b_home) = tokio::join!(
        sandboxes.admit_sandbox(home("sbx_alice", &alice, &a)),
        peer_sandboxes.admit_sandbox(home("sbx_bob", &bob, &b)),
    );
    let (loser, owner, key, id) = match (a_home, b_home) {
        (Ok(_), Err(SandboxStoreError::GlobalCapacityReached)) => (&second, &bob, "bob-home", b),
        (Err(SandboxStoreError::GlobalCapacityReached), Ok(_)) => (&first, &alice, "alice-home", a),
        other => panic!("expected one home and one capacity refusal, got {other:?}"),
    };
    let CreateResult::Existing(replayed) = loser.create(owner, key, input()).await.unwrap() else {
        panic!("capacity loser must retain the same operation");
    };
    assert_eq!(replayed.id, id);
    assert!(replayed.sandbox_id.is_none());
    assert!(loser.claim(&id).await.unwrap().is_some());
    assert_eq!(
        sandboxes.sandboxes.count_documents(doc! {}).await.unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn lifecycle_counts_pending_trigger_homes_and_refuses_unreadable_capacity() {
    let (client, name, store) = fixture().await;
    let sandboxes = with_ceiling(&client, &name).await;
    let owner = SubjectId::new("owner");
    let fence = crate::store::launch_home_capacity::fence_key();
    for pending in [Bson::Int64(1), Bson::String("unreadable".into())] {
        sandboxes.sandbox_admissions.update_one(
            fence.clone(),
            doc! {"$set": {"document_schema_version": DOCUMENT_SCHEMA_VERSION, "pending_launch_homes": pending}},
        ).upsert(true).await.unwrap();
        assert!(matches!(
            store.create(&owner, "fresh", input()).await,
            Err(LifecycleCreateError::HomeCapacity)
        ));
        assert!(matches!(
            sandboxes
                .admit_sandbox(home("sbx_not_allocated", &owner, "already-admitted"))
                .await,
            Err(SandboxStoreError::GlobalCapacityReached)
        ));
    }
    assert_eq!(store.operations.count_documents(doc! {}).await.unwrap(), 0);
    assert_eq!(
        sandboxes.sandboxes.count_documents(doc! {}).await.unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}
