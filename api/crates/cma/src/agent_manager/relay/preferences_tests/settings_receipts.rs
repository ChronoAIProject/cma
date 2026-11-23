use super::*;
use layer2_core::sandbox::AgentStatus;

const REASON: &str = "cma-settings-reasons";

fn path() -> String {
    format!("/api/v1/agents/{AGENT}/settings")
}

fn body(model: &str) -> Value {
    json!({"model":model, "reasoning_effort":"high", "permission_mode":"guarded"})
}

fn patch(replica: &Replica, key: &str, model: &str) -> reqwest::RequestBuilder {
    client()
        .patch(format!("{}{}", replica.address, path()))
        .header("idempotency-key", key)
        .header("prefer", REASON)
        .json(&body(model))
}

async fn response(request: reqwest::RequestBuilder) -> (StatusCode, Value) {
    let response = request.send().await.unwrap();
    let status = response.status();
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    (status, response.json().await.unwrap())
}

async fn read(replica: &Replica) -> (StatusCode, Value) {
    response(client().get(format!("{}{}", replica.address, path()))).await
}

async fn busy(replica: &Replica, authority: &Arc<Authority>, busy: bool) {
    authority
        .run(async {
            let mut agent = replica
                .store
                .get_agent(&AgentId(AGENT.into()))
                .await
                .unwrap()
                .unwrap();
            let revision = agent.revision;
            agent.revision += 1;
            agent.status = if busy {
                AgentStatus::Working
            } else {
                AgentStatus::Ready
            };
            agent.conversation.active_turn = busy.then_some(1);
            replica.store.save_agent(revision, agent).await.unwrap();
        })
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn coordinated_settings_apply_on_both_ingresses_replays_the_original_outcome() {
    let (mongo, name, store) =
        crate::store::sandbox_maintenance::mongo_integration_tests::database().await;
    seed(&store).await;
    let replicas = [
        Replica::open(&name, true).await,
        Replica::open(&name, true).await,
    ];
    for replica in &replicas {
        replica
            .relay
            .as_ref()
            .unwrap()
            .enable_settings_owner_routing();
    }
    let (owner, authority) = fixture::acquire(&replicas).await;
    let peer = 1 - owner;
    let original = read(&replicas[peer]).await;
    assert_eq!(original.0, StatusCode::OK);
    busy(&replicas[owner], &authority, true).await;
    let refused = response(patch(&replicas[peer], "settings-busy", "fixture-first")).await;
    assert_eq!(refused.0, StatusCode::CONFLICT, "{}", refused.1);
    assert_eq!(refused.1["settings_update_reason"], "agent_not_idle");
    busy(&replicas[owner], &authority, false).await;
    assert_eq!(read(&replicas[peer]).await, original);
    assert_eq!(
        response(patch(&replicas[owner], "settings-busy", "fixture-first")).await,
        refused
    );
    assert!(replicas[owner].runtime.inputs.lock().unwrap().is_empty());

    for (ingress, key, model) in [
        (owner, "settings-owner", "fixture-first"),
        (peer, "settings-peer", "fixture-second"),
    ] {
        let accepted = response(patch(&replicas[ingress], key, model)).await;
        assert_eq!(accepted.0, StatusCode::OK, "{}", accepted.1);
        assert_eq!(accepted.1["model"], model);
        assert_eq!(accepted.1["pending"], false);
        for replica in &replicas {
            assert_eq!(read(replica).await, accepted);
            assert_eq!(response(patch(replica, key, model)).await, accepted);
        }
    }
    {
        let inputs = replicas[owner].runtime.inputs.lock().unwrap();
        assert!(matches!(
            inputs.as_slice(),
            [layer3_adapter::ConversationInput::Configure { model: first, .. },
             layer3_adapter::ConversationInput::Configure { model: second, .. }]
                if first == "fixture-first" && second == "fixture-second"
        ));
    }
    assert!(replicas[peer].runtime.inputs.lock().unwrap().is_empty());
    assert!(replicas[peer].scopes.lock().unwrap().is_empty());
    let scopes = replicas[owner].scopes.lock().unwrap().clone();
    assert_eq!(scopes.len(), 8);
    assert!(scopes
        .iter()
        .all(|scope| scope.as_ref() == Some(&authority.token)));
    for replica in replicas {
        replica.close().await;
    }
    mongo.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn uncoordinated_settings_keeps_the_existing_handler() {
    let (mongo, name, store) =
        crate::store::sandbox_maintenance::mongo_integration_tests::database().await;
    seed(&store).await;
    let replica = Replica::open(&name, false).await;
    let accepted = response(patch(&replica, "legacy-settings", "fixture-legacy")).await;
    assert_eq!(accepted.0, StatusCode::OK, "{}", accepted.1);
    assert_eq!(read(&replica).await, accepted);
    assert_eq!(replica.scopes.lock().unwrap().as_slice(), &[None]);
    assert_eq!(replica.runtime.inputs.lock().unwrap().len(), 1);
    replica.close().await;
    mongo.database(&name).drop().await.unwrap();
}
