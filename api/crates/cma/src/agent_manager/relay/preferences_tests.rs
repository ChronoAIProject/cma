use super::*;
use crate::store::MongoSandboxStore;
use layer2_core::sandbox::store::{AgentStore, SandboxStore, WorkspaceStore, WorktreeStore};
use serde_json::{json, Value};

#[path = "preferences_tests/fixture.rs"]
mod fixture;
use fixture::*;

#[path = "preferences_tests/settings_receipts.rs"]
mod settings_receipts;

#[path = "preferences_tests/process_recovery.rs"]
mod process_recovery;

#[path = "settings_tests.rs"]
mod settings;

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn preferences_patch_on_either_replica_uses_owner_scope_and_replays_one_title_event() {
    let (mongo, name, durable) =
        crate::store::sandbox_maintenance::mongo_integration_tests::database().await;
    seed(&durable).await;
    let replicas = vec![
        Replica::open(&name, true).await,
        Replica::open(&name, true).await,
    ];
    let (owner, authority) = fixture::acquire(&replicas).await;
    let peer = 1 - owner;
    let body = json!({"title":"Owner revised title", "expected_revision":0});
    let (status, result) = replicas[peer]
        .request(Method::PATCH, Some("rename-once"), Some(body.clone()))
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["title"], "Owner revised title");
    assert_eq!(result["revision"], 1);
    for replica in &replicas {
        assert_eq!(
            replica.request(Method::GET, None, None).await,
            (StatusCode::OK, result.clone())
        );
        assert_eq!(
            replica
                .request(Method::PATCH, Some("rename-once"), Some(body.clone()))
                .await,
            (StatusCode::OK, result.clone())
        );
    }
    assert!(replicas[peer].scopes.lock().unwrap().is_empty());
    assert_eq!(replicas[owner].scopes.lock().unwrap().len(), 3);
    assert!(replicas[owner]
        .scopes
        .lock()
        .unwrap()
        .iter()
        .all(|scope| scope.as_ref() == Some(&authority.token)));
    assert_eq!(title_count(&mongo.database(&name)).await, 1);
    let changed = json!({"title":"Changed request", "expected_revision":1});
    let (status, problem) = replicas[peer]
        .request(Method::PATCH, Some("rename-once"), Some(changed))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert_eq!(problem["code"], "idempotency_key_reused");
    assert_eq!(
        replicas[owner]
            .request(Method::PATCH, Some("stale-title"), Some(body))
            .await
            .0,
        StatusCode::CONFLICT
    );
    let (status, result) = replicas[owner]
        .request(
            Method::PATCH,
            Some("direct-title"),
            Some(json!({"title":"Direct owner title", "expected_revision":1})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(
        replicas[peer].request(Method::GET, None, None).await.1["title"],
        "Direct owner title"
    );
    assert_eq!(title_count(&mongo.database(&name)).await, 2);
    assert!(replicas
        .iter()
        .all(|replica| replica.runtime.inputs.lock().unwrap().is_empty()));
    for replica in replicas {
        replica.close().await;
    }
    mongo.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn preferences_patch_without_coordination_keeps_the_existing_handler() {
    let (mongo, name, durable) =
        crate::store::sandbox_maintenance::mongo_integration_tests::database().await;
    seed(&durable).await;
    let replica = Replica::open(&name, false).await;
    let (status, result) = replica
        .request(
            Method::PATCH,
            Some("uncoordinated-title"),
            Some(json!({"title":"Ordinary title", "expected_revision":0})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["revision"], 1);
    assert_eq!(*replica.scopes.lock().unwrap(), vec![None]);
    assert_eq!(title_count(&mongo.database(&name)).await, 1);
    replica.close().await;
    mongo.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn preferences_owner_handoff_rejects_old_proofs_and_preserves_read_and_replay() {
    let (mongo, name, durable) =
        crate::store::sandbox_maintenance::mongo_integration_tests::database().await;
    seed(&durable).await;
    let replicas = vec![
        Replica::open(&name, true).await,
        Replica::open(&name, true).await,
    ];
    let initial = json!({"title":"Before handoff", "expected_revision":0});
    let (status, problem) = replicas[0]
        .request(
            Method::PATCH,
            Some("handoff-initial"),
            Some(initial.clone()),
        )
        .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{problem}");
    assert_eq!(problem["code"], "agent_owner_unavailable");
    assert_eq!(title_count(&mongo.database(&name)).await, 0);
    let (owner, authority) = fixture::acquire(&replicas).await;
    let peer = 1 - owner;
    let original = replicas[peer]
        .request(
            Method::PATCH,
            Some("handoff-initial"),
            Some(initial.clone()),
        )
        .await;
    assert_eq!(original.0, StatusCode::OK, "{}", original.1);
    let old = replicas[owner].coordinator.as_ref().unwrap();
    for owned in old.begin_drain().await {
        old.handoff(&owned, async { Ok(()) }).await.unwrap();
    }
    assert!(!authority.is_current());
    assert_eq!(
        replicas[owner].request(Method::GET, None, None).await.0,
        StatusCode::OK
    );
    let next = replicas[peer]
        .coordinator
        .as_ref()
        .unwrap()
        .acquire(&AgentId(AGENT.into()))
        .await
        .unwrap()
        .unwrap();
    assert!(next.token.epoch > authority.token.epoch);
    let uri: Uri = format!("/api/v1/agents/{AGENT}/preferences")
        .parse()
        .unwrap();
    let body = br#"{"title":"Stale owner write","expected_revision":1}"#;
    let headers = HeaderMap::from_iter([
        (
            axum::http::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        ),
        (
            axum::http::HeaderName::from_static("idempotency-key"),
            "old-epoch".parse().unwrap(),
        ),
    ]);
    let relay = replicas[peer].relay.as_ref().unwrap();
    let proof = relay
        .authentication
        .sign(
            &next.token.instance_id,
            &authority.token,
            &Method::PATCH,
            &uri,
            &headers,
            body,
        )
        .unwrap();
    let response = client()
        .patch(format!("{}{uri}", replicas[peer].address))
        .headers(headers)
        .header(PROOF, proof)
        .body(body.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(title_count(&mongo.database(&name)).await, 1);
    assert_eq!(
        replicas[peer]
            .request(Method::PATCH, Some("handoff-initial"), Some(initial))
            .await,
        original
    );
    let (status, result) = replicas[peer]
        .request(
            Method::PATCH,
            Some("new-epoch"),
            Some(json!({"title":"After handoff", "expected_revision":1})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(
        replicas[owner].request(Method::GET, None, None).await.1["title"],
        "After handoff"
    );
    assert_eq!(
        replicas[peer].scopes.lock().unwrap().last(),
        Some(&Some(next.token.clone()))
    );
    assert_eq!(title_count(&mongo.database(&name)).await, 2);
    for replica in replicas {
        replica.close().await;
    }
    mongo.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn preferences_system_prompt_uses_the_same_owner_and_durable_replay() {
    let (mongo, name, durable) =
        crate::store::sandbox_maintenance::mongo_integration_tests::database().await;
    seed(&durable).await;
    let replicas = vec![
        Replica::open(&name, true).await,
        Replica::open(&name, true).await,
    ];
    let (owner, authority) = fixture::acquire(&replicas).await;
    let peer = 1 - owner;
    let body = json!({"system_prompt":"Owner revised instructions", "expected_revision":0});
    let result = replicas[peer]
        .request(Method::PATCH, Some("prompt-once"), Some(body.clone()))
        .await;
    assert_eq!(result.0, StatusCode::OK, "{}", result.1);
    assert_eq!(result.1["revision"], 1);
    assert_eq!(result.1["system_prompt"], "Owner revised instructions");
    assert_eq!(result.1["prompt_pending"], false);
    assert_eq!(
        replicas[owner]
            .request(Method::PATCH, Some("prompt-once"), Some(body))
            .await,
        result
    );
    assert!(replicas[peer].runtime.inputs.lock().unwrap().is_empty());
    let inputs = replicas[owner].runtime.inputs.lock().unwrap().clone();
    assert_eq!(inputs.len(), 1);
    assert!(
        matches!(&inputs[0], layer3_adapter::ConversationInput::Configure {system_prompt:Some(prompt),..} if prompt == "Owner revised instructions")
    );
    assert!(replicas[owner]
        .scopes
        .lock()
        .unwrap()
        .iter()
        .all(|scope| scope.as_ref() == Some(&authority.token)));
    assert_eq!(
        replicas[peer].request(Method::GET, None, None).await,
        result
    );
    for replica in replicas {
        replica.close().await;
    }
    mongo.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn preferences_system_prompt_retains_idle_runtime_and_parent_guards() {
    use layer2_core::sandbox::{AgentStatus, SandboxStatus};
    use std::sync::atomic::Ordering;
    for condition in ["unsupported", "busy", "paused-parent"] {
        let (mongo, name, durable) =
            crate::store::sandbox_maintenance::mongo_integration_tests::database().await;
        seed(&durable).await;
        let replicas = vec![
            Replica::open(&name, true).await,
            Replica::open(&name, true).await,
        ];
        let (owner, authority) = fixture::acquire(&replicas).await;
        let store = &replicas[owner].store;
        authority
            .run(async {
                match condition {
                    "unsupported" => replicas[owner]
                        .runtime
                        .supported
                        .store(false, Ordering::SeqCst),
                    "busy" => {
                        let mut agent = store
                            .get_agent(&AgentId(AGENT.into()))
                            .await
                            .unwrap()
                            .unwrap();
                        let revision = agent.revision;
                        agent.revision += 1;
                        agent.status = AgentStatus::Working;
                        agent.conversation.active_turn = Some(1);
                        store.save_agent(revision, agent).await.unwrap();
                    }
                    _ => {
                        let mut parent = store
                            .get_sandbox(&layer2_core::SandboxId("sbx-preferences".into()))
                            .await
                            .unwrap()
                            .unwrap();
                        let revision = parent.revision;
                        parent.revision += 1;
                        parent.status = SandboxStatus::Paused;
                        parent.runtime = None;
                        parent.lifecycle.checkpoints.latest = Some(
                            layer2_core::sandbox::SandboxCheckpointRef::seal(
                                &layer3_adapter::CheckpointReceipt {
                                    epoch: 1,
                                    object_reference: "preferences-fixture-checkpoint".into(),
                                    sha256: "a".repeat(64),
                                    writers: layer3_adapter::WriterState::Resumed,
                                },
                                &[],
                                parent.created_at.clone(),
                            )
                            .unwrap(),
                        );
                        store.save_sandbox(revision, parent).await.unwrap();
                    }
                }
            })
            .await
            .unwrap();
        let before = store
            .get_agent(&AgentId(AGENT.into()))
            .await
            .unwrap()
            .unwrap();
        let (status, problem) = replicas[1 - owner]
            .request(
                Method::PATCH,
                Some("refused-prompt"),
                Some(json!({"system_prompt":"Refused instructions", "expected_revision":0})),
            )
            .await;
        assert_eq!(
            status,
            if condition == "unsupported" {
                StatusCode::UNPROCESSABLE_ENTITY
            } else {
                StatusCode::CONFLICT
            },
            "{condition}: {problem}"
        );
        if condition == "unsupported" {
            assert_eq!(problem["code"], "invalid_agent_request");
        }
        assert_eq!(store.get_agent(&before.id).await.unwrap().unwrap(), before);
        assert!(replicas
            .iter()
            .all(|replica| replica.runtime.inputs.lock().unwrap().is_empty()));
        for replica in replicas {
            replica.close().await;
        }
        mongo.database(&name).drop().await.unwrap();
    }
}
