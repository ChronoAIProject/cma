use super::*;
use layer2_core::sandbox::{
    AgentOutputCoalescePolicy, AgentUpdateRecorder, DurableAgentUpdateRecorder,
};
use layer3_adapter::{ConversationUpdate, RuntimeOutputReceipt, RuntimeOutputSource};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Weak,
    },
    time::Duration,
};

fn retained(
    agent: &Agent,
    cursor: u64,
    ack: tokio::sync::mpsc::Sender<RuntimeOutputSource>,
) -> ConversationUpdate {
    ConversationUpdate::RuntimeOutput {
        receipt: RuntimeOutputReceipt::new(
            RuntimeOutputSource {
                stream_id: uuid::Uuid::nil().to_string(),
                cursor,
                native_conversation_id: agent.conversation.conversation_id.clone().unwrap(),
                payload_sha256: format!("{cursor:064x}"),
            },
            ack,
        ),
        update: None,
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn quiet_hundred_frames_reduce_real_transactions_and_keep_version_one() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let mut options = mongodb::options::ClientOptions::parse(uri).await.unwrap();
    let commits = Arc::new(AtomicU64::new(0));
    let observed = commits.clone();
    options.command_event_handler = Some(mongodb::event::EventHandler::callback(move |event| {
        if matches!(event, mongodb::event::command::CommandEvent::Started(ref started) if started.command_name == "commitTransaction")
        {
            observed.fetch_add(1, Ordering::SeqCst);
        }
    }));
    let client = mongodb::Client::with_options(options).unwrap();
    let name = format!("cma_1807_coalesce_{}", uuid::Uuid::new_v4().simple());
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let agent = working(&store, "sbx_coalesced").await;
    let recorder = DurableAgentUpdateRecorder::with_output_policy(
        store.clone(),
        Arc::default(),
        None,
        false,
        false,
        Some(AgentOutputCoalescePolicy::new(Duration::from_millis(250), 64).unwrap()),
    );
    let mut assembly = AgentMessageAssembly::default();
    let (ack, _) = tokio::sync::mpsc::channel(1);
    commits.store(0, Ordering::SeqCst);
    let started = std::time::Instant::now();
    let mut committed = 0;
    for cursor in 1..=100 {
        let result = recorder
            .record_output(
                &agent.id,
                1,
                &mut assembly,
                retained(&agent, cursor, ack.clone()),
                NOW.into(),
            )
            .await
            .unwrap();
        if result.committed.is_some() {
            committed += 1;
        }
    }
    if recorder
        .flush_output(&agent.id, 1, &mut assembly, NOW.into())
        .await
        .unwrap()
        .is_some()
    {
        committed += 1;
    }
    let elapsed = started.elapsed();
    assert_eq!(commits.load(Ordering::SeqCst), committed);
    let upper_bound = 100_u64.div_ceil(64) + elapsed.as_millis() as u64 / 250;
    assert!(
        committed <= upper_bound,
        "{committed} transactions exceed count/time bound {upper_bound}"
    );
    assert!(committed < 100);
    let checkpoint = store
        .agent_message_checkpoint(&agent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint.output.as_ref().unwrap().cursor, 100);
    assert_eq!(checkpoint.version, 1);
    // A disabled recorder reads the exact same serialized checkpoint and accepts replay.
    let disabled = DurableAgentUpdateRecorder::new(store.clone());
    disabled
        .restore_assembly(&agent.id, 1, &mut AgentMessageAssembly::default())
        .await
        .unwrap();
    assert_eq!(
        disabled
            .record_output(
                &agent.id,
                1,
                &mut assembly,
                retained(&agent, 100, ack),
                NOW.into()
            )
            .await
            .unwrap()
            .committed
            .unwrap()
            .cursor,
        100
    );
    println!(
        "QUIET_BATCH frames=100 commits={committed} elapsed_ms={} max_bound={upper_bound}",
        elapsed.as_millis()
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn live_consumer_timer_commits_and_acks_without_another_frame() {
    let (client, name, store) = database().await;
    let agent = working(&store, "sbx_timer").await;
    let store = Arc::new(store);
    let recorder = DurableAgentUpdateRecorder::with_output_policy(
        store.clone(),
        Arc::default(),
        None,
        false,
        false,
        Some(AgentOutputCoalescePolicy::new(Duration::from_millis(250), 64).unwrap()),
    );
    let (activate, activated) = tokio::sync::oneshot::channel();
    let (send, updates) = tokio::sync::mpsc::channel(8);
    let (ack, mut received) = tokio::sync::mpsc::channel(8);
    let task = crate::sandbox::binding_lifecycle_test_support::spawn_update_task(
        Weak::new(),
        activated,
        updates,
        recorder,
    );
    activate
        .send(
            crate::sandbox::binding_lifecycle_test_support::LiveAgentAttachmentIdentity {
                agent_id: agent.id.clone(),
                conversation_id: agent.conversation.conversation_id.clone().unwrap(),
                runtime_generation: 1,
                authority_fence: 1,
                nonce: 1,
            },
        )
        .unwrap();
    send.send(retained(&agent, 1, ack)).await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), received.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.cursor, 1);
    assert_eq!(
        store
            .agent_message_checkpoint(&agent.id)
            .await
            .unwrap()
            .unwrap()
            .output
            .unwrap(),
        result
    );
    assert!(received.try_recv().is_err());
    task.abort();
    client.database(&name).drop().await.unwrap();
}
