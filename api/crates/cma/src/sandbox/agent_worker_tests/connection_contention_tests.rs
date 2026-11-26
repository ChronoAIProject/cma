//! Exercise normal worker reconnection after a definitively aborted publication.
use super::*;
use crate::store::MongoSandboxStore;
use mongodb::bson::doc;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at disposable replica set"]
async fn connection_contention_recovers_through_normal_worker_and_sends_input_once() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let app = format!("cma-connection-contention-{suffix}");
    let database = format!("cma_connection_contention_{suffix}");
    let admin = mongodb::Client::with_uri_str(&uri).await.unwrap();
    let mut options = mongodb::options::ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app.clone());
    let client = mongodb::Client::with_options(options).unwrap();
    let store = Arc::new(MongoSandboxStore::open(&client, &database).await.unwrap());
    let mut harness =
        Harness::with_store(store.clone(), Arc::new(EmptyAgentSkillResolver), 1).await;
    harness.now = SystemTime::now();
    let agent = harness
        .admit_with_first_message("connection-conflict", vec![], Some("Run exactly once"))
        .await;
    for _ in 0..4 {
        // Each step publishes ready_at using its execution time, which can pass
        // harness.now on a slow claim. Match the scheduler's fresh claim clock
        // (api/crates/cma/src/sandbox/agent_scheduler.rs:296).
        // 250 polls allow 5 s of 20 ms sleeps per step before asserting failure.
        for attempt in 0..250 {
            if harness
                .worker
                .run_one(&agent.id, SystemTime::now())
                .await
                .unwrap()
            {
                break;
            }
            assert!(attempt < 249, "startup work did not become claimable");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    let pending = harness.stored(&agent.id).await;
    assert_eq!(pending.work.step, AgentWorkStep::PublishConnection);
    let now = SystemTime::now();
    let claimed = store
        .claim_agent_work(
            &agent.id,
            "agent-worker",
            rfc3339_at(now),
            rfc3339_at(now + Duration::from_secs(18 * 60)),
        )
        .await
        .unwrap()
        .unwrap();
    let target = Harness::target(&claimed);
    let attachment = harness
        .registry
        .staged_attachment(&target, claimed.work.lease.as_ref().unwrap().fence)
        .unwrap()
        .unwrap();
    harness.registry.publish_and_activate(&attachment).unwrap();
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": {"times": 40_i32},
            "data": {"failCommands": ["update"], "appName": app,
                "errorCode": 112_i32, "errorLabels": ["TransientTransactionError"]},
        })
        .await
        .unwrap();
    let outcome = harness
        .worker
        .finish_connection_publication(claimed.clone(), &attachment, None, now)
        .await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    outcome.unwrap();
    let retrying = harness.stored(&agent.id).await;
    assert_eq!(retrying.work.step, AgentWorkStep::PublishConnection);
    assert!(retrying.work.lease.is_none());
    assert_eq!(
        retrying.conversation.first_message,
        claimed.conversation.first_message
    );
    let retry_at = SystemTime::UNIX_EPOCH
        + Duration::from_secs(layer2_core::seconds_from_rfc3339(&retrying.work.ready_at).unwrap());
    // This enters publish_connection, performs runtime adoption/reconnect,
    // transfers the live channel to fresh authority and commits the next step.
    assert!(harness.worker.run_one(&agent.id, retry_at).await.unwrap());
    let connected = harness.stored(&agent.id).await;
    assert_eq!(connected.work.step, AgentWorkStep::DispatchFirstMessage);
    assert_eq!(
        connected.conversation.first_message,
        claimed.conversation.first_message
    );
    let rebound = harness
        .registry
        .attachment(&target, connected.work.next_fence - 1)
        .unwrap()
        .unwrap();
    assert_eq!(
        rebound.identity().authority_fence,
        connected.work.next_fence - 1
    );
    assert!(rebound.identity().authority_fence > attachment.identity().authority_fence);
    // Connection publication advances ready_at beyond retry_at by its elapsed
    // execution time. Keep the simulated retry clock, but let it advance while
    // polling, with the same 250-poll / 20 ms bound as startup.
    let dispatch_started = tokio::time::Instant::now();
    for attempt in 0..250 {
        if harness
            .worker
            .run_one(&agent.id, retry_at + dispatch_started.elapsed())
            .await
            .unwrap()
        {
            break;
        }
        assert!(attempt < 249, "first message work did not become claimable");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let done = harness.stored(&agent.id).await;
    assert_eq!(done.work.step, AgentWorkStep::None);
    assert_eq!(
        done.conversation.conversation_id,
        claimed.conversation.conversation_id
    );
    assert_eq!(
        done.conversation.runtime_generation,
        claimed.conversation.runtime_generation
    );
    assert_eq!(
        done.conversation.first_message.as_ref().unwrap().state,
        FirstMessageDelivery::WriteConfirmed
    );
    assert_eq!(done.conversation.first_message.as_ref().unwrap().attempt, 1);
    assert_eq!(
        harness
            .store
            .agent_log(&agent.id, 0, 100)
            .await
            .unwrap()
            .iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::OwnerMessage { .. }))
            .count(),
        1
    );
    assert!(!harness
        .worker
        .run_one(&agent.id, retry_at + Duration::from_secs(1))
        .await
        .unwrap());
    client.database(&database).drop().await.unwrap();
}
