use super::cross_interface_tests::{authenticated, read, Route};
use super::*;
use axum::body::{to_bytes, Body};
use futures_util::{FutureExt, StreamExt};
use layer2_core::sandbox::agent_io::*;
use layer2_core::sandbox::AgentLogEntry;
use mongodb::{bson::doc, Client};
use tower::ServiceExt;

use crate::store::MongoSandboxStore;

mod proof;

const INPUTS: [(Route, &str, &str); 3] = [
    (Route::Responses, "mongo-legacy-first", "sdk-raw-with-tools"),
    (Route::Agui, "mongo-agui-second", "Second input from AG-UI"),
    (Route::ResponsesV2, "mongo-v2-third", "Third input from v2"),
];

fn request(route: Route, agent: &AgentId, key: &str, input: &str) -> Request<Body> {
    let mut request = route.request(agent, input);
    request
        .headers_mut()
        .insert("idempotency-key", key.parse().unwrap());
    request
}

async fn commands(store: &MongoSandboxStore, count: usize) -> Vec<AdmittedAgentCommand> {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let page = store
                .command_work_page(&PageRequest::default())
                .await
                .unwrap();
            assert!(page.next.is_none());
            if page.items.len() == count {
                let mut items = page.items;
                items.sort_by_key(|item| item.receipt.agent_order);
                return items;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("public handlers persist all admissions in Mongo")
}

fn receipt_headers(response: &Response, command: &AdmittedAgentCommand) {
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["x-cma-command-id"],
        command.receipt.envelope.command_id.as_str()
    );
    assert_eq!(
        response.headers()["x-cma-response-id"],
        command.receipt.response_id.as_ref().unwrap().as_str()
    );
}

async fn exercise(client: &Client, name: &str, uri: &str) {
    let store = Arc::new(MongoSandboxStore::open(client, name).await.unwrap());
    let harness = Harness::with_store(store, Arc::new(EmptyAgentSkillResolver), 1).await;
    let agent = harness.admit("mongo-public-agent", vec![]).await;
    let agent = harness.drive_ready(&agent.id).await;
    let probe = runtime_tests::RuntimeProbe::gated();
    let (router, worker) =
        conversation_router_with_probe(&harness, Some(probe.clone()), "mongo-public-first-worker")
            .await;
    let router = authenticated(router, &harness);
    let mut pending = Vec::new();
    let mut original = Vec::new();

    // Legacy AfterTurn(None) is first on a fresh conversation. The other two
    // protocols append; their ordering fingerprints are intentionally distinct.
    for (index, (route, key, input)) in INPUTS.iter().copied().enumerate() {
        let request = request(route, &agent.id, key, input);
        let handler = router.clone();
        pending.push(tokio::spawn(async move {
            handler.oneshot(request).await.unwrap()
        }));
        original = commands(&harness.store, index + 1).await;
        assert_eq!(original[index].receipt.agent_order, index as u64 + 1);
    }
    assert!(probe.inputs().is_empty());
    for command in &original {
        assert_eq!(command.receipt.envelope.agent_id, agent.id);
        assert_eq!(command.receipt.envelope.source.kind, AgentSourceKind::Api);
        assert!(matches!(
            command.receipt.envelope.actor,
            AgentActor::Api { .. }
        ));
        assert_eq!(command.status, AgentResponseStatus::Queued);
        assert!(command.work.dispatch.is_none());
    }
    proof::reject_changed_inputs(&router, &agent.id).await;
    // A duplicate queued v2 request recovers the same persisted receipt before
    // any command worker has been run, without consuming another queue order.
    let (route, key, input) = INPUTS[2];
    let duplicate = router
        .clone()
        .oneshot(request(route, &agent.id, key, input))
        .await
        .unwrap();
    receipt_headers(&duplicate, &original[2]);
    drop(duplicate);
    assert_eq!(commands(&harness.store, 3).await, original);

    let first_id = &original[0].receipt.envelope.command_id;
    worker.run_one(first_id).await.unwrap();
    let first = tokio::time::timeout(Duration::from_secs(15), pending.remove(0))
        .await
        .unwrap()
        .unwrap();
    receipt_headers(&first, &original[0]);
    let mut stream = first.into_body().into_data_stream();
    let frame = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(std::str::from_utf8(&frame).unwrap().contains("data:"));
    drop(stream);
    for task in pending {
        task.abort();
        let _ = task.await;
    }
    let bound = harness.store.get_command(first_id).await.unwrap().unwrap();
    assert_eq!(bound.receipt, original[0].receipt);
    assert!(bound.work.dispatch.is_some());
    assert_eq!(bound.status, AgentResponseStatus::InProgress);
    assert_eq!(probe.inputs().len(), 1);
    drop(worker);
    drop(router);

    // This is in-process replacement with an independent Mongo client/store,
    // public services and command worker. The fixture runtime remains alive;
    // this does not simulate process death or a deployed runtime reconnect.
    let reopened_client = Client::with_uri_str(uri).await.unwrap();
    let reopened_store = Arc::new(
        MongoSandboxStore::open(&reopened_client, name)
            .await
            .unwrap(),
    );
    assert!(!Arc::ptr_eq(&harness.store, &reopened_store));
    let harness = Harness {
        updates: DurableAgentUpdateRecorder::new(reopened_store.clone()),
        store: reopened_store,
        ..harness
    };
    let (reopened, replacement) = conversation_router_with_probe(
        &harness,
        Some(probe.clone()),
        "mongo-public-replacement-worker",
    )
    .await;
    let reopened = authenticated(reopened, &harness);
    assert_eq!(
        harness.store.get_command(first_id).await.unwrap().unwrap(),
        bound
    );
    replacement.run_one(first_id).await.unwrap();
    replacement
        .run_one(&original[2].receipt.envelope.command_id)
        .await
        .unwrap();
    replacement
        .run_one(&original[1].receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(
        probe.inputs().len(),
        1,
        "an active first turn blocks later public inputs"
    );

    for index in 0..original.len() {
        let command = &original[index];
        let id = &command.receipt.envelope.command_id;
        if index > 0 {
            // Selecting a later command must not skip the admitted head.
            if index == 1 {
                replacement
                    .run_one(&original[2].receipt.envelope.command_id)
                    .await
                    .unwrap();
                assert_eq!(probe.inputs().len(), 1);
            }
            replacement.run_one(id).await.unwrap();
        }
        proof::assert_inputs(&probe, &agent, index + 1);
        probe.release_reply();
        proof::completed(&reopened, &harness.store, &replacement, command).await;
    }
    let done = harness.store.get_command(first_id).await.unwrap().unwrap();
    assert_eq!(done.work.dispatch, bound.work.dispatch);
    proof::assert_inputs(&probe, &agent, 3);
    let final_agent = harness.stored(&agent.id).await;
    assert_eq!(final_agent.id, agent.id);
    assert_eq!(final_agent.sandbox_id, agent.sandbox_id);
    assert_eq!(final_agent.workspace_id, agent.workspace_id);
    assert_eq!(final_agent.worktree_id, agent.worktree_id);
    assert_eq!(
        final_agent.conversation.conversation_id,
        agent.conversation.conversation_id
    );
    assert_eq!(
        final_agent.conversation.runtime_generation,
        agent.conversation.runtime_generation
    );
    let logs = harness.store.agent_log(&agent.id, 0, 100).await.unwrap();
    proof::ordered_history(&logs);
    proof::replay_and_retry(
        &reopened,
        &harness.store,
        &replacement,
        &agent,
        &original,
        &logs,
    )
    .await;
    proof::assert_inputs(&probe, &agent, 3);
    assert_eq!(
        harness.store.agent_log(&agent.id, 0, 100).await.unwrap(),
        logs
    );
    assert_eq!(
        client
            .database(name)
            .collection::<mongodb::bson::Document>("agent_commands")
            .count_documents(doc! {})
            .await
            .unwrap(),
        3
    );
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn public_protocol_fifo_and_retries_survive_mongo_service_and_worker_replacement() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("test Mongo replica URI");
    let client = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_851_public_{}", AgentId::generate());
    let result = std::panic::AssertUnwindSafe(exercise(&client, &name, &uri))
        .catch_unwind()
        .await;
    client.database(&name).drop().await.unwrap();
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}
