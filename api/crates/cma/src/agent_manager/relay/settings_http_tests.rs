use super::*;
use crate::store::MongoSandboxStore;
use layer2_core::sandbox::store::{AgentStore, SandboxStore, WorkspaceStore, WorktreeStore};
use serde_json::{json, Value};

#[path = "settings_tests/fixture.rs"]
mod fixture;
#[path = "settings_tests/runtime.rs"]
mod runtime;
use fixture::*;
#[path = "settings_tests/access.rs"]
mod access;
#[path = "settings_tests/guards.rs"]
mod guards;
#[path = "settings_tests/lifecycle.rs"]
mod lifecycle;

fn changed() -> Value {
    json!({"model":"gpt-6", "reasoning_effort":"medium", "permission_mode":"guarded"})
}

async fn changed_request_uses_owner(direct: bool) {
    let (mongo, name, durable) =
        crate::store::sandbox_maintenance::mongo_integration_tests::database().await;
    seed(&durable).await;
    let replicas = vec![
        Replica::open(&name, true).await,
        Replica::open(&name, true).await,
    ];
    let (owner, authority) = fixture::acquire(&replicas).await;
    let peer = 1 - owner;
    let executing = if direct { owner } else { peer };
    let before = replicas[peer]
        .store
        .get_agent(&AgentId(AGENT.into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        before.settings().reasoning_effort,
        layer2_core::profiles::ThinkingEffort::High
    );
    assert_eq!(
        before.settings().permission_mode,
        layer2_core::profiles::PermissionLevel::Autonomous
    );
    let (status, result) = replicas[executing]
        .request(Method::PATCH, Some("settings-once"), Some(changed()))
        .await;
    let after = replicas[peer]
        .store
        .get_agent(&before.id)
        .await
        .unwrap()
        .unwrap();
    eprintln!("direct={direct} ingress={executing} owner={owner} epoch={} status={status} result={result} owner_configures={} peer_configures={} before={:?} after={:?}",
        authority.token.epoch, replicas[owner].runtime.inputs.lock().unwrap().len(),
        replicas[peer].runtime.inputs.lock().unwrap().len(), before.settings(), after.settings());
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(
        result,
        json!({"model":"gpt-6", "reasoning_effort":"medium", "permission_mode":"guarded", "pending":false})
    );
    assert_eq!(after.profile, before.profile);
    assert_eq!(
        after.settings().system_prompt,
        before.settings().system_prompt
    );
    assert_eq!(replicas[owner].runtime.inputs.lock().unwrap().len(), 1);
    assert!(replicas[peer]
        .runtime
        .resolutions
        .lock()
        .unwrap()
        .is_empty());
    assert!(replicas[peer].runtime.inputs.lock().unwrap().is_empty());
    assert_eq!(
        *replicas[owner].scopes.lock().unwrap(),
        vec![Some(authority.token.clone())]
    );
    assert!(replicas[owner]
        .runtime
        .resolutions
        .lock()
        .unwrap()
        .iter()
        .all(|(_, scope)| scope.as_ref() == Some(&authority.token)));
    for replica in &replicas {
        assert_eq!(
            replica.request(Method::GET, None, None).await,
            (status, result.clone())
        );
        assert_eq!(
            replica
                .request(Method::PATCH, Some("settings-once"), Some(changed()))
                .await,
            (status, result.clone())
        );
    }
    assert_eq!(replicas[owner].runtime.inputs.lock().unwrap().len(), 1);
    assert_eq!(
        replicas[peer]
            .store
            .get_agent(&before.id)
            .await
            .unwrap()
            .unwrap(),
        after
    );
    let mut alternate = changed();
    alternate["model"] = json!("fixture-alternate-model");
    let (status, updated) = replicas[peer]
        .request(Method::PATCH, Some("different-model"), Some(alternate))
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["model"], "fixture-alternate-model");
    assert_eq!(updated["pending"], false);
    assert_eq!(
        replicas[owner].request(Method::GET, None, None).await.1,
        updated
    );
    assert_eq!(replicas[owner].runtime.inputs.lock().unwrap().len(), 2);
    assert!(replicas[peer].runtime.inputs.lock().unwrap().is_empty());
    for replica in replicas {
        replica.close().await;
    }
    mongo.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn settings_patch_from_peer_reaches_only_owner_runtime_and_replays() {
    changed_request_uses_owner(false).await;
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn settings_patch_direct_to_owner_supplies_publication_scope() {
    changed_request_uses_owner(true).await;
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn settings_patch_without_coordination_keeps_local_handler_and_noop_behavior() {
    let (mongo, name, durable) =
        crate::store::sandbox_maintenance::mongo_integration_tests::database().await;
    seed(&durable).await;
    let replica = Replica::open(&name, false).await;
    let (status, result) = replica
        .request(Method::PATCH, Some("local-settings"), Some(changed()))
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["pending"], false);
    assert_eq!(replica.runtime.inputs.lock().unwrap().len(), 1);
    assert_eq!(*replica.scopes.lock().unwrap(), vec![None]);
    assert!(replica
        .runtime
        .resolutions
        .lock()
        .unwrap()
        .iter()
        .all(|(_, scope)| scope.is_none()));
    assert_eq!(
        replica
            .request(Method::PATCH, Some("unchanged-settings"), Some(changed()))
            .await,
        (status, result)
    );
    assert_eq!(replica.runtime.inputs.lock().unwrap().len(), 1);
    replica.close().await;
    mongo.database(&name).drop().await.unwrap();
}
