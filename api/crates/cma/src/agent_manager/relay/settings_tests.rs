//! Live settings use the exact execution owner through the authenticated HTTP relay.
use super::*;

const BODY: &str =
    r#"{"model":"fixture-settings-model","reasoning_effort":"high","permission_mode":"guarded"}"#;

fn path() -> String {
    format!("/api/v1/agents/{AGENT}/settings")
}

fn patch(replica: &Replica, key: &str) -> reqwest::RequestBuilder {
    client()
        .patch(format!("{}{}", replica.address, path()))
        .header("content-type", "application/json")
        .header("idempotency-key", key)
        .body(BODY)
}

async fn view(response: reqwest::Response) -> (StatusCode, Value) {
    let status = response.status();
    (status, response.json().await.unwrap())
}

async fn read(replica: &Replica) -> (StatusCode, Value) {
    view(
        client()
            .get(format!("{}{}", replica.address, path()))
            .send()
            .await
            .unwrap(),
    )
    .await
}

fn enable(replica: &Replica) {
    replica
        .relay
        .as_ref()
        .unwrap()
        .enable_settings_owner_routing();
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn settings_patch_on_peer_dispatches_on_owner_once_and_replays_after_handoff() {
    let (mongo, name, durable) =
        crate::store::sandbox_maintenance::mongo_integration_tests::database().await;
    seed(&durable).await;
    let replicas = [
        Replica::open(&name, true).await,
        Replica::open(&name, true).await,
    ];
    for replica in &replicas {
        enable(replica);
    }
    assert_eq!(read(&replicas[0]).await.0, StatusCode::OK);
    let (status, problem) = view(patch(&replicas[0], "settings-once").send().await.unwrap()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(problem["code"], "agent_owner_unavailable");

    let (owner, authority) = fixture::acquire(&replicas).await;
    let peer = 1 - owner;
    let response = patch(&replicas[peer], "settings-once")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    drop(response);
    let expected = json!({
        "model":"fixture-settings-model", "reasoning_effort":"high",
        "permission_mode":"guarded", "pending":false
    });
    for replica in &replicas {
        assert_eq!(read(replica).await, (StatusCode::OK, expected.clone()));
        assert_eq!(
            view(patch(replica, "settings-once").send().await.unwrap()).await,
            (StatusCode::OK, expected.clone())
        );
    }
    assert_eq!(
        replicas[owner].scopes.lock().unwrap().as_slice(),
        &[
            Some(authority.token.clone()),
            Some(authority.token.clone()),
            Some(authority.token.clone()),
        ]
    );
    assert!(replicas[peer].scopes.lock().unwrap().is_empty());
    assert!(matches!(
        replicas[owner].runtime.inputs.lock().unwrap().as_slice(),
        [layer3_adapter::ConversationInput::Configure { model, .. }] if model == "fixture-settings-model"
    ));
    assert!(replicas[peer].runtime.inputs.lock().unwrap().is_empty());

    let old = replicas[owner].coordinator.as_ref().unwrap();
    for owned in old.begin_drain().await {
        old.handoff(&owned, async { Ok(()) }).await.unwrap();
    }
    let next = replicas[peer]
        .coordinator
        .as_ref()
        .unwrap()
        .acquire(&AgentId(AGENT.into()))
        .await
        .unwrap()
        .unwrap();
    assert!(next.token.epoch > authority.token.epoch);
    let headers = HeaderMap::from_iter([
        (
            axum::http::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        ),
        (
            axum::http::HeaderName::from_static("idempotency-key"),
            "stale-epoch".parse().unwrap(),
        ),
    ]);
    let proof = replicas[peer]
        .relay
        .as_ref()
        .unwrap()
        .authentication
        .sign(
            &next.token.instance_id,
            &authority.token,
            &Method::PATCH,
            &path().parse().unwrap(),
            &headers,
            BODY.as_bytes(),
        )
        .unwrap();
    let stale = patch(&replicas[owner], "stale-epoch")
        .header(PROOF, proof)
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        read(&replicas[owner]).await,
        (StatusCode::OK, expected.clone())
    );
    assert_eq!(
        view(
            patch(&replicas[peer], "settings-once")
                .send()
                .await
                .unwrap()
        )
        .await,
        (StatusCode::OK, expected)
    );
    assert_eq!(replicas[owner].runtime.inputs.lock().unwrap().len(), 1);
    assert!(replicas[peer].runtime.inputs.lock().unwrap().is_empty());
    for replica in replicas {
        replica.close().await;
    }
    mongo.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn settings_patch_refuses_foreign_principal_and_mixed_rollout() {
    let (mongo, name, durable) =
        crate::store::sandbox_maintenance::mongo_integration_tests::database().await;
    seed(&durable).await;
    let replicas = [
        Replica::open(&name, true).await,
        Replica::open(&name, true).await,
    ];
    let (owner, _) = fixture::acquire(&replicas).await;
    let peer = 1 - owner;
    let off = patch(&replicas[peer], "settings-disabled")
        .send()
        .await
        .unwrap();
    assert!(!off.status().is_success());
    assert_eq!(replicas[peer].scopes.lock().unwrap().as_slice(), &[None]);
    assert!(replicas[owner].scopes.lock().unwrap().is_empty());

    enable(&replicas[peer]);
    let mixed = patch(&replicas[peer], "settings-mixed")
        .send()
        .await
        .unwrap();
    assert_eq!(mixed.status(), StatusCode::FORBIDDEN);
    assert_eq!(replicas[peer].scopes.lock().unwrap().as_slice(), &[None]);
    assert!(replicas[owner].scopes.lock().unwrap().is_empty());
    enable(&replicas[owner]);

    let foreign = Replica::open_as(&name, true, "outside-settings-owner").await;
    enable(&foreign);
    let denied = patch(&foreign, "settings-denied").send().await.unwrap();
    assert!(matches!(
        denied.status(),
        StatusCode::NOT_FOUND | StatusCode::FORBIDDEN
    ));
    assert!(foreign.scopes.lock().unwrap().is_empty());
    assert!(replicas[owner].scopes.lock().unwrap().is_empty());
    assert!(replicas
        .iter()
        .all(|replica| replica.runtime.inputs.lock().unwrap().is_empty()));
    foreign.close().await;
    for replica in replicas {
        replica.close().await;
    }
    mongo.database(&name).drop().await.unwrap();
}
