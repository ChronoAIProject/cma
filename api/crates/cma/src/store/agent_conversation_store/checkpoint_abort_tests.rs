use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Weak,
};
use std::time::Duration;

use layer2_core::sandbox::{
    AgentConversationError, AgentOutputCommit, AgentUpdateRecorder, DurableAgentUpdateRecorder,
};
use layer3_adapter::{
    ContentBlock, ContentChunk, ConversationUpdate, MessageId, PromptResponse,
    RuntimeOutputReceipt, RuntimeOutputSource, SessionUpdate, TurnStopReason,
};
use mongodb::{event::command::CommandEvent, event::EventHandler, options::ClientOptions, Client};
use tokio::sync::{mpsc, Notify};

use super::*;

// Hold only the first returned error so an independent, valid publication can
// advance the anchor before the live consumer retries its unchanged assembly.
struct RetryBarrier {
    recorder: Arc<DurableAgentUpdateRecorder>,
    first: AtomicBool,
    errors: mpsc::UnboundedSender<String>,
    release: Notify,
}

#[async_trait::async_trait]
impl AgentUpdateRecorder for RetryBarrier {
    async fn restore_assembly(
        &self,
        id: &AgentId,
        generation: u64,
        assembly: &mut AgentMessageAssembly,
    ) -> Result<(), AgentConversationError> {
        self.recorder
            .restore_assembly(id, generation, assembly)
            .await
    }

    async fn record_update(
        &self,
        id: &AgentId,
        generation: u64,
        assembly: &mut AgentMessageAssembly,
        update: ConversationUpdate,
        at: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        self.recorder
            .record_update(id, generation, assembly, update, at)
            .await
    }

    async fn record_output(
        &self,
        id: &AgentId,
        generation: u64,
        assembly: &mut AgentMessageAssembly,
        update: ConversationUpdate,
        at: String,
    ) -> Result<AgentOutputCommit, AgentConversationError> {
        let result = self
            .recorder
            .record_output(id, generation, assembly, update, at)
            .await;
        if let Err(error) = &result {
            self.errors.send(error.to_string()).unwrap();
            if self.first.swap(false, Ordering::SeqCst) {
                self.release.notified().await;
            }
        }
        result
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn exhausted_message_abort_retries_after_anchor_advance_and_acks_once() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let name = format!("cma_checkpoint_abort_{suffix}");
    let app_name = format!("cma-checkpoint-abort-{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let failures = Arc::new(AtomicUsize::new(0));
    let observed = failures.clone();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if matches!(event, CommandEvent::Failed(ref failed) if failed.command_name == "update") {
            observed.fetch_add(1, Ordering::SeqCst);
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let agent = super::anchor::working_with_worktree(&store, "sbx_checkpoint_abort").await;
    let (errors, mut failed) = mpsc::unbounded_channel();
    let barrier = Arc::new(RetryBarrier {
        recorder: DurableAgentUpdateRecorder::new(Arc::new(store.clone())),
        first: AtomicBool::new(true),
        errors,
        release: Notify::new(),
    });
    let (activate, activated) = tokio::sync::oneshot::channel();
    let (send, updates) = mpsc::channel(8);
    let (ack, mut acknowledged) = mpsc::channel(8);
    let task = crate::sandbox::binding_lifecycle_test_support::spawn_update_task(
        Weak::new(),
        activated,
        updates,
        barrier.clone(),
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
    let stream = uuid::Uuid::new_v4().to_string();
    let retained = |cursor: u64, update: ConversationUpdate| ConversationUpdate::RuntimeOutput {
        receipt: RuntimeOutputReceipt::new(
            RuntimeOutputSource {
                stream_id: stream.clone(),
                cursor,
                native_conversation_id: agent.conversation.conversation_id.clone().unwrap(),
                payload_sha256: format!("{cursor:064x}"),
            },
            ack.clone(),
        ),
        update: Some(Box::new(update).into()),
    };
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": "alwaysOn",
            "data": {"failCommands": ["update"], "appName": &app_name,
                "errorCode": 112_i32, "errorLabels": ["TransientTransactionError"]},
        })
        .await
        .unwrap();
    send.send(retained(
        1,
        ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(
            ContentChunk::new(ContentBlock::from("retained answer"))
                .message_id(MessageId::new("reply")),
        )),
    ))
    .await
    .unwrap();
    let first_failure = tokio::time::timeout(Duration::from_secs(10), failed.recv()).await;
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": "off",
        })
        .await
        .unwrap();
    assert!(
        first_failure.unwrap().is_some(),
        "must exhaust the recorder attempt budget"
    );
    assert!(failures.load(Ordering::SeqCst) >= 5);
    assert!(
        acknowledged.try_recv().is_err(),
        "aborted output must not be ACKed"
    );
    assert!(store
        .agent_message_checkpoint(&agent.id)
        .await
        .unwrap()
        .is_none());
    super::anchor::publish_connection(&store, &agent.id).await;
    let advanced = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert!(advanced.conversation.last_seq > agent.conversation.last_seq);
    assert_eq!(
        advanced.conversation.active_turn,
        agent.conversation.active_turn
    );
    barrier.release.notify_one();
    let recovery = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::select! {
            receipt = acknowledged.recv() => Ok(receipt.unwrap()),
            error = failed.recv() => Err(error.unwrap()),
        }
    })
    .await
    .unwrap();
    // Always stop the consumer before asserting RED, so no retry task survives it.
    if recovery.is_err() {
        task.abort();
        client.database(&name).drop().await.unwrap();
    }
    let receipt = recovery.expect("known-aborted output must recover after checkpoint advancement");
    assert_eq!(receipt.cursor, 1);
    assert!(
        acknowledged.try_recv().is_err(),
        "one receipt ACK for one output"
    );
    send.send(retained(
        2,
        ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::EndTurn)),
    ))
    .await
    .unwrap();
    let terminal = tokio::time::timeout(Duration::from_secs(5), acknowledged.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(terminal.cursor, 2);
    assert!(acknowledged.try_recv().is_err());
    let logs = store.agent_log(&agent.id, 0, 100).await.unwrap();
    let messages: Vec<_> = logs
        .iter()
        .filter_map(|entry| match &entry.kind {
            AgentLogKind::AgentMessage { content } => Some(content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(messages, ["retained answer"]);
    assert_eq!(
        logs.iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::TurnCompleted { .. }))
            .count(),
        1
    );
    let finished = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(finished.status, AgentStatus::Ready);
    assert_eq!(finished.conversation.active_turn, None);
    let checkpoint = store
        .agent_message_checkpoint(&agent.id)
        .await
        .unwrap()
        .unwrap();
    assert!(checkpoint.matches(&finished));
    assert_eq!(checkpoint.output.unwrap().cursor, 2);
    task.abort();
    client.database(&name).drop().await.unwrap();
}
