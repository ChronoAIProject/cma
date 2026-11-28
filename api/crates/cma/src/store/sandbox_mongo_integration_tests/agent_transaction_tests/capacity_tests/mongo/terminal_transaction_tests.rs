use super::*;
use crate::store::agent_log_integrity::NORMAL_AGENT_LOG_BYTES;
use crate::store::sandbox_mongo_integration_tests::capacity_fixtures::{
    fill_to, PRIVATE_PREFIX_HISTORY_BYTES,
};
use layer2_core::sandbox::{
    AgentConversationError, AgentMessageAssembly, AgentUpdateRecorder, DurableAgentUpdateRecorder,
};
use layer3_adapter::{
    ConversationUpdate, PromptIdentity, PromptResponse, RuntimeOutputReceipt, RuntimeOutputSource,
    TurnStopReason,
};
use std::time::{Duration, Instant};

struct Fixture {
    store: Arc<MongoSandboxStore>,
    client: Client,
    admin: Client,
    name: String,
    app: String,
    full: Agent,
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn exhausted_terminal_reserve_preserves_prefix_and_never_acknowledges() {
    let mut f = Fixture::new().await;
    f.full = crate::store::sandbox_mongo_integration_tests::capacity_fixtures::fill_to(
        &f.store,
        f.full.clone(),
        Some(10_064),
        None,
    )
    .await;
    let recorder = DurableAgentUpdateRecorder::new(f.store.clone());
    let mut assembly = AgentMessageAssembly::default();
    recorder
        .record_update(
            &f.full.id,
            1,
            &mut assembly,
            ConversationUpdate::Said(layer3_adapter::SessionUpdate::AgentMessageChunk(
                layer3_adapter::ContentChunk::new(layer3_adapter::ContentBlock::from(
                    "Retained private prefix",
                )),
            )),
            MINUTE.into(),
        )
        .await
        .unwrap();
    let before = f.store.agent_message_checkpoint(&f.full.id).await.unwrap();
    let (acks, mut acknowledged) = tokio::sync::mpsc::channel(2);
    let result = crate::sandbox::record_update_for_test(
        recorder.as_ref(),
        &f.full.id,
        1,
        &mut assembly,
        f.update(acks),
    )
    .await;
    assert!(
        matches!(result, Err(AgentConversationError::CapacityExceeded)),
        "{result:?}"
    );
    assert!(acknowledged.try_recv().is_err());
    assert_eq!(
        f.store.get_agent(&f.full.id).await.unwrap(),
        Some(f.full.clone())
    );
    assert_eq!(
        f.store.agent_message_checkpoint(&f.full.id).await.unwrap(),
        before
    );
    assert!(!before.unwrap().assembly.is_empty());
    f.cleanup().await;
}
impl Fixture {
    async fn new() -> Self {
        Self::with_byte_limit(None).await
    }
    async fn with_byte_limit(byte_limit: Option<usize>) -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
        let app = format!("capacity-terminal-{}", uuid::Uuid::new_v4());
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app.clone());
        let client = Client::with_options(options).unwrap();
        let admin = Client::with_uri_str(uri).await.unwrap();
        let name = format!("cma_terminal_capacity_{}", uuid::Uuid::new_v4().simple());
        let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
        let ready = ready_conversation_fixture(&store, "terminal_transaction").await;
        let turn = begin_stored_conversation_turn(&store, ready).await;
        store.commit_agent_conversation(turn.clone()).await.unwrap();
        let full = match byte_limit {
            Some(limit) => fill_to(&store, turn.agent, None, Some(limit)).await,
            None => fill_normal_capacity(&store, turn.agent, false).await,
        };
        Self {
            store,
            client,
            admin,
            name,
            app,
            full,
        }
    }
    fn update(&self, acks: tokio::sync::mpsc::Sender<RuntimeOutputSource>) -> ConversationUpdate {
        ConversationUpdate::RuntimeOutput {
            receipt: RuntimeOutputReceipt::new(
                RuntimeOutputSource {
                    stream_id: "94111111-1111-4111-8111-111111111111".into(),
                    cursor: 1,
                    native_conversation_id: self.full.conversation.conversation_id.clone().unwrap(),
                    payload_sha256: "d".repeat(64),
                },
                acks,
            ),
            update: Some(
                Box::new(ConversationUpdate::TurnEndedForPrompt {
                    prompt_id: PromptIdentity::new(
                        self.full.conversation.conversation_id.as_deref().unwrap(),
                        1,
                        self.full.conversation.active_turn.unwrap(),
                    ),
                    response: PromptResponse::new(TurnStopReason::EndTurn),
                })
                .into(),
            ),
        }
    }
    async fn failpoint(&self, mode: Bson, mut data: Document) -> i64 {
        data.insert("failCommands", vec!["commitTransaction"]);
        data.insert("appName", &self.app);
        let result = self
            .admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand", "mode":mode, "data":data,
            })
            .await
            .unwrap();
        result
            .get_i64("count")
            .or_else(|_| result.get_i32("count").map(i64::from))
            .unwrap()
    }
    async fn off(&self) {
        self.admin
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
            .await
            .unwrap();
    }
    async fn one_terminal(&self) {
        let saved = self.store.get_agent(&self.full.id).await.unwrap().unwrap();
        assert_eq!(saved.status, AgentStatus::Failed);
        assert!(saved.work.lease.is_none());
        let tail = self
            .store
            .agent_log(&saved.id, self.full.conversation.last_seq, 20)
            .await
            .unwrap();
        assert_eq!(
            tail.iter()
                .filter(|e| matches!(e.kind, AgentLogKind::TurnCompleted { .. }))
                .count(),
            1
        );
        assert_eq!(tail.len(), 2);
    }
    async fn cleanup(self) {
        self.off().await;
        self.client.database(&self.name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn native_terminal_ack_waits_for_the_reserved_transaction_commit() {
    let f = Fixture::new().await;
    let entered = f
        .failpoint(
            Bson::Document(doc! {"times":1}),
            doc! {"blockConnection":true, "blockTimeMS":2000},
        )
        .await;
    let recorder = DurableAgentUpdateRecorder::new(f.store.clone());
    let (acks, mut acknowledged) = tokio::sync::mpsc::channel(2);
    let update = f.update(acks);
    let id = f.full.id.clone();
    let task = tokio::spawn(async move {
        crate::sandbox::record_update_for_test(
            recorder.as_ref(),
            &id,
            1,
            &mut AgentMessageAssembly::default(),
            update,
        )
        .await
    });
    f.admin
        .database("admin")
        .run_command(
            doc! {"waitForFailPoint":"failCommand", "timesEntered":entered+1, "maxTimeMS":10_000},
        )
        .await
        .unwrap();
    assert!(
        acknowledged.try_recv().is_err(),
        "runtime receipt escaped before commit"
    );
    assert_eq!(
        f.store.get_agent(&f.full.id).await.unwrap(),
        Some(f.full.clone())
    );
    tokio::time::timeout(Duration::from_secs(120), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(acknowledged.recv().await.unwrap().cursor, 1);
    f.one_terminal().await;
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn committed_unknown_native_terminal_is_proved_and_replayed_once() {
    let f = Fixture::new().await;
    f.failpoint(Bson::Document(doc! {"times":5}), doc! {"writeConcernError":{"code":50, "codeName":"MaxTimeMSExpired", "errmsg":"fixture uncertain commit"}}).await;
    let recorder = DurableAgentUpdateRecorder::new(f.store.clone());
    let (acks, _) = tokio::sync::mpsc::channel(2);
    let update = f.update(acks);
    let started = Instant::now();
    let result = recorder
        .record_update(
            &f.full.id,
            1,
            &mut AgentMessageAssembly::default(),
            update.clone(),
            MINUTE.into(),
        )
        .await;
    f.off().await;
    result.unwrap();
    assert!(started.elapsed() < Duration::from_secs(120));
    let peer = Arc::new(MongoSandboxStore::open(&f.client, &f.name).await.unwrap());
    DurableAgentUpdateRecorder::new(peer)
        .record_update(
            &f.full.id,
            1,
            &mut AgentMessageAssembly::default(),
            update,
            MINUTE.into(),
        )
        .await
        .unwrap();
    f.one_terminal().await;
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn unproved_native_terminal_commit_returns_typed_unknown_without_assuming_abort() {
    let f = Fixture::new().await;
    f.failpoint(
        Bson::String("alwaysOn".into()),
        doc! {"errorCode":1, "errorLabels":["UnknownTransactionCommitResult"]},
    )
    .await;
    let recorder = DurableAgentUpdateRecorder::new(f.store.clone());
    let (acks, mut acknowledged) = tokio::sync::mpsc::channel(2);
    let update = f.update(acks);
    let mut assembly = AgentMessageAssembly::default();
    let started = Instant::now();
    let result = recorder
        .record_update(&f.full.id, 1, &mut assembly, update.clone(), MINUTE.into())
        .await;
    f.off().await;
    assert!(started.elapsed() < Duration::from_secs(120));
    assert!(
        matches!(result, Err(AgentConversationError::Unavailable(_))),
        "{result:?}"
    );
    assert!(acknowledged.try_recv().is_err());
    assert_eq!(
        f.store.get_agent(&f.full.id).await.unwrap(),
        Some(f.full.clone())
    );
    recorder
        .record_update(&f.full.id, 1, &mut assembly, update, MINUTE.into())
        .await
        .unwrap();
    f.one_terminal().await;
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn compact_capacity_failure_preserves_prefix_through_both_unknown_commit_outcomes() {
    for committed in [false, true] {
        prefix_failure_unknown(committed, true).await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn visible_capacity_failure_replays_prefix_once_through_both_unknown_commit_outcomes() {
    for committed in [false, true] {
        prefix_failure_unknown(committed, false).await;
    }
}

async fn prefix_failure_unknown(committed: bool, retained: bool) {
    let byte_limit = if retained {
        PRIVATE_PREFIX_HISTORY_BYTES
    } else {
        NORMAL_AGENT_LOG_BYTES
    };
    let f = Fixture::with_byte_limit(Some(byte_limit)).await;
    let recorder = DurableAgentUpdateRecorder::new(f.store.clone());
    let mut assembly = AgentMessageAssembly::default();
    let text = if retained {
        "p".repeat(1024 * 1024)
    } else {
        "Visible partial reply before failure".to_owned()
    };
    recorder
        .record_update(
            &f.full.id,
            1,
            &mut assembly,
            ConversationUpdate::Said(layer3_adapter::SessionUpdate::AgentMessageChunk(
                layer3_adapter::ContentChunk::new(layer3_adapter::ContentBlock::from(text.clone())),
            )),
            MINUTE.into(),
        )
        .await
        .unwrap();
    let current = f.full.clone();
    let prefix = f
        .store
        .agent_message_checkpoint(&current.id)
        .await
        .unwrap()
        .unwrap();
    if committed {
        f.failpoint(Bson::Document(doc! {"times":5}), doc! {"writeConcernError":{"code":50,"codeName":"MaxTimeMSExpired","errmsg":"fixture uncertain compact commit"}}).await;
    } else {
        f.failpoint(
            Bson::String("alwaysOn".into()),
            doc! {"errorCode":1,"errorLabels":["UnknownTransactionCommitResult"]},
        )
        .await;
    }
    let (acks, mut acknowledged) = tokio::sync::mpsc::channel(2);
    let mut update = f.update(acks);
    if !retained {
        let ConversationUpdate::RuntimeOutput { update: inner, .. } = &mut update else {
            unreachable!()
        };
        *inner = Some(
            Box::new(ConversationUpdate::Said(
                layer3_adapter::SessionUpdate::ToolCallUpdate(layer3_adapter::ToolCallUpdate::new(
                    "refused",
                    layer3_adapter::ToolCallUpdateFields::new().title("Unrecorded tool output"),
                )),
            ))
            .into(),
        );
    }
    let started = Instant::now();
    let result = recorder
        .record_update(&current.id, 1, &mut assembly, update.clone(), MINUTE.into())
        .await;
    f.off().await;
    assert!(started.elapsed() < Duration::from_secs(120));
    if committed {
        let outcome = result.unwrap();
        assert!(outcome.turn_closed);
        assert!(outcome.appended.iter().any(|e|matches!(&e.kind,AgentLogKind::TurnFailed {failure,..} if failure.class == "agent_log_capacity_exceeded")));
    } else {
        assert!(matches!(
            result,
            Err(AgentConversationError::Unavailable(_))
        ));
        assert_eq!(
            f.store.get_agent(&current.id).await.unwrap(),
            Some(current.clone())
        );
        assert_eq!(
            crate::sandbox::record_update_for_test(
                recorder.as_ref(),
                &current.id,
                1,
                &mut assembly,
                update.clone()
            )
            .await,
            Err(AgentConversationError::CapacityExceeded)
        );
    }
    assert!(acknowledged.try_recv().is_err());
    let row = f
        .store
        .agents
        .find_one(doc! {"_id":current.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    if retained {
        assert_eq!(
            row.get("agent_capacity_prefix_v1"),
            Some(&mongodb::bson::serialize_to_bson(&prefix).unwrap())
        );
    } else {
        assert!(!row.contains_key("agent_capacity_prefix_v1"));
    }
    let tail = f
        .store
        .agent_log(&current.id, current.conversation.last_seq, 20)
        .await
        .unwrap();
    assert_eq!(tail.iter().filter(|entry| matches!(&entry.kind, AgentLogKind::AgentMessage { content } if content == &text)).count(), usize::from(!retained));
    if !retained {
        assert!(matches!(tail[0].kind, AgentLogKind::AgentMessage { .. }));
    }
    let settled = f.store.get_agent(&current.id).await.unwrap().unwrap();
    let peer = Arc::new(MongoSandboxStore::open(&f.client, &f.name).await.unwrap());
    assert!(DurableAgentUpdateRecorder::new(peer)
        .record_update(
            &current.id,
            1,
            &mut AgentMessageAssembly::default(),
            update,
            MINUTE.into()
        )
        .await
        .is_err());
    assert_eq!(f.store.get_agent(&current.id).await.unwrap(), Some(settled));
    assert_eq!(
        tail.iter()
            .filter(|e| matches!(e.kind, AgentLogKind::TurnFailed { .. }))
            .count(),
        1
    );
    assert_eq!(
        f.store
            .get_agent(&current.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Failed
    );
    f.cleanup().await;
}
