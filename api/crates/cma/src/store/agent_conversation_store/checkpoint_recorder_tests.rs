use std::sync::Arc;

use crate::store::agent_log_segments as segments;
use crate::store::sandbox_maintenance::mongo_integration_tests::{
    active_sandbox, insert_agent, ready_agent,
};
use layer2_core::sandbox::store::SandboxStore;
use layer2_core::sandbox::{
    AgentConversationError, AgentUpdateRecorder, DurableAgentUpdateRecorder, SandboxOrigin,
    SandboxRuntimeImageOrigin,
};
use layer3_adapter::{
    ContentBlock, ContentChunk, ConversationUpdate, MessageId, PromptResponse, SessionUpdate,
    TurnStopReason,
};

use super::*;

fn chunk(content: &str) -> ConversationUpdate {
    ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(
        ContentChunk::new(ContentBlock::from(content)).message_id(MessageId::new("reply")),
    ))
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn replacement_mongo_recorder_recovers_prefix_then_commits_one_complete_message() {
    let (client, name, store) = database().await;
    let current = working(&store, "sbx_checkpoint_recorder").await;
    let recorder = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    recorder
        .record_update(
            &current.id,
            1,
            &mut AgentMessageAssembly::default(),
            chunk("prefix "),
            NOW.into(),
        )
        .await
        .unwrap();
    assert_eq!(
        store.get_agent(&current.id).await.unwrap(),
        Some(current.clone())
    );
    drop(recorder);

    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    let replacement = DurableAgentUpdateRecorder::new(Arc::new(reopened.clone()));
    replacement
        .record_update(
            &current.id,
            1,
            &mut AgentMessageAssembly::default(),
            chunk("suffix"),
            NOW.into(),
        )
        .await
        .unwrap();
    drop(replacement);
    let terminal = DurableAgentUpdateRecorder::new(Arc::new(reopened.clone()));
    let outcome = terminal
        .record_update(
            &current.id,
            1,
            &mut AgentMessageAssembly::default(),
            ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::EndTurn)),
            NOW.into(),
        )
        .await
        .unwrap();
    assert!(outcome.turn_closed);
    let logs = reopened.agent_log(&current.id, 0, 100).await.unwrap();
    let messages: Vec<_> = logs
        .iter()
        .filter_map(|entry| match &entry.kind {
            AgentLogKind::AgentMessage { content } => Some(content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(messages, ["prefix suffix"]);
    let checkpoint = reopened
        .agent_message_checkpoint(&current.id)
        .await
        .unwrap()
        .unwrap();
    assert!(checkpoint.assembly.is_empty());
    assert_eq!(checkpoint.active_turn, None);
    assert_eq!(checkpoint.outcome, outcome);
    let finished = reopened.get_agent(&current.id).await.unwrap().unwrap();
    assert_eq!(finished.status, AgentStatus::Ready);
    assert_eq!(finished.conversation.active_turn, None);
    assert!(checkpoint.matches(&finished));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retained_mongo_output_replays_once_after_commit_before_ack_and_reopen() {
    let (client, name, store) = database().await;
    let current = working(&store, "sbx_output_replay").await;
    let stream = uuid::Uuid::new_v4().to_string();
    let retained = |cursor: u64, update: Option<ConversationUpdate>| {
        let (ack, _) = tokio::sync::mpsc::channel(1);
        ConversationUpdate::RuntimeOutput {
            receipt: layer3_adapter::RuntimeOutputReceipt::new(
                layer3_adapter::RuntimeOutputSource {
                    stream_id: stream.clone(),
                    cursor,
                    native_conversation_id: current.conversation.conversation_id.clone().unwrap(),
                    payload_sha256: format!("{cursor:064x}"),
                },
                ack,
            ),
            update: update.map(|update| Box::new(update).into()),
        }
    };
    let recorder = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    recorder
        .record_update(
            &current.id,
            1,
            &mut AgentMessageAssembly::default(),
            retained(1, Some(chunk("one answer"))),
            NOW.into(),
        )
        .await
        .unwrap();
    drop(recorder);
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    let replacement = DurableAgentUpdateRecorder::new(Arc::new(reopened.clone()));
    let mut assembly = AgentMessageAssembly::default();
    replacement
        .record_update(
            &current.id,
            1,
            &mut assembly,
            retained(1, Some(chunk("one answer"))),
            NOW.into(),
        )
        .await
        .unwrap();
    replacement
        .record_update(
            &current.id,
            1,
            &mut assembly,
            retained(
                2,
                Some(ConversationUpdate::TurnEnded(PromptResponse::new(
                    TurnStopReason::EndTurn,
                ))),
            ),
            NOW.into(),
        )
        .await
        .unwrap();
    drop(replacement);
    let reopened_again = MongoSandboxStore::open(&client, &name).await.unwrap();
    let replacement = DurableAgentUpdateRecorder::new(Arc::new(reopened_again.clone()));
    let before = reopened_again.agent_log(&current.id, 0, 100).await.unwrap();
    // The completed prompt no longer has a waiter on the replacement adapter.
    replacement
        .record_update(
            &current.id,
            1,
            &mut AgentMessageAssembly::default(),
            retained(2, None),
            NOW.into(),
        )
        .await
        .unwrap();
    assert_eq!(
        reopened_again.agent_log(&current.id, 0, 100).await.unwrap(),
        before
    );
    let messages: Vec<_> = before
        .iter()
        .filter_map(|entry| match &entry.kind {
            AgentLogKind::AgentMessage { content } => Some(content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(messages, ["one answer"]);
    let checkpoint = reopened_again
        .agent_message_checkpoint(&current.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint.output.unwrap().cursor, 2);
    assert!(checkpoint.assembly.is_empty());
    assert_eq!(
        reopened_again
            .get_agent(&current.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Ready
    );
    client.database(&name).drop().await.unwrap();
}

async fn incremental_working(
    store: &MongoSandboxStore,
    name: &str,
    image: &str,
    origin: SandboxRuntimeImageOrigin,
    segmented: bool,
) -> Agent {
    let mut sandbox = active_sandbox(name);
    sandbox.origin = SandboxOrigin::Lifecycle {
        operation_id: format!("prepare-{name}"),
    };
    sandbox.profile.content.runtime_image = Some(image.into());
    sandbox.profile.digest =
        layer2_core::profiles::content_digest(&sandbox.profile.content).unwrap();
    sandbox.lifecycle.runtime_image_origin = Some(origin);
    sandbox.runtime.as_mut().unwrap().runtime_version = image.into();
    store.create_sandbox(sandbox.clone()).await.unwrap();
    let agent = ready_agent(&sandbox, &format!("agt_{name}"));
    if segmented {
        use segments::mongo_tests::fixtures as segment_fixture;
        let opened = AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: 1,
            appended_at: NOW.into(),
            kind: AgentLogKind::ConversationOpened {
                conversation_id: agent.conversation.conversation_id.clone().unwrap(),
                runtime_generation: 1,
            },
        };
        let (agent, entries) = segment_fixture::install(store, agent, vec![opened], 0, 1).await;
        segment_fixture::certify(store, &agent, &entries).await;
        start_turn(store, &sandbox, &agent).await
    } else {
        insert_agent(store, &agent, NOW).await;
        start_turn(store, &sandbox, &agent).await
    }
}

fn incremental_recorder(store: &MongoSandboxStore) -> Arc<DurableAgentUpdateRecorder> {
    DurableAgentUpdateRecorder::with_incremental_output_policy(
        Arc::new(store.clone()),
        Arc::default(),
        None,
        false,
        false,
        None,
        true,
    )
}

async fn record_incremental_terminal(store: &MongoSandboxStore, current: &Agent) {
    let recorder = incremental_recorder(store);
    let stream_id = uuid::Uuid::new_v4().to_string();
    let retained = |cursor, fingerprint, update: ConversationUpdate| {
        let (ack, _) = tokio::sync::mpsc::channel(1);
        ConversationUpdate::RuntimeOutput {
            receipt: layer3_adapter::RuntimeOutputReceipt::new(
                layer3_adapter::RuntimeOutputSource {
                    stream_id: stream_id.clone(),
                    cursor,
                    native_conversation_id: current.conversation.conversation_id.clone().unwrap(),
                    payload_sha256: fingerprint,
                },
                ack,
            ),
            update: Some(Box::new(update).into()),
        }
    };
    let mut assembly = AgentMessageAssembly::default();
    let mut offset = 0;
    for (cursor, text) in [(1, "prefix "), (2, "suffix é")] {
        let outcome = recorder
            .record_update(
                &current.id,
                1,
                &mut assembly,
                retained(cursor, format!("{cursor:064x}"), chunk(text)),
                NOW.into(),
            )
            .await
            .unwrap();
        assert_eq!(
            outcome.appended[0].kind,
            AgentLogKind::AgentMessageDelta {
                offset,
                content: text.into(),
            }
        );
        offset += text.len() as u32;
    }
    let terminal = |fingerprint| {
        retained(
            3,
            fingerprint,
            ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::EndTurn)),
        )
    };
    let update = recorder
        .record_update(
            &current.id,
            1,
            &mut assembly,
            terminal("a".repeat(64)),
            NOW.into(),
        )
        .await
        .unwrap();
    assert!(update.turn_closed);
    let checkpoint = store
        .agent_message_checkpoint(&current.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint.outcome, update);
    assert!(checkpoint.assembly.is_empty());
    assert_eq!(checkpoint.output.as_ref().unwrap().cursor, 3);
    let finished = store.get_agent(&current.id).await.unwrap().unwrap();
    assert!(checkpoint.matches(&finished));
    assert_eq!(finished.status, AgentStatus::Ready);
    assert_eq!(finished.conversation.active_turn, None);

    let before = store.agent_log(&current.id, 0, 100).await.unwrap();
    assert_eq!(
        before
            .iter()
            .filter_map(|entry| match &entry.kind {
                AgentLogKind::AgentMessage { content } => Some(content.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        ["prefix suffix é"],
    );
    assert_eq!(
        before
            .iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::TurnCompleted { .. }))
            .count(),
        1,
    );

    let replacement = incremental_recorder(store);
    let mut assembly = AgentMessageAssembly::default();
    assert_eq!(
        replacement
            .record_update(
                &current.id,
                1,
                &mut assembly,
                terminal("a".repeat(64)),
                NOW.into()
            )
            .await
            .unwrap(),
        update,
    );
    assert!(matches!(
        replacement
            .record_update(
                &current.id,
                1,
                &mut assembly,
                terminal("b".repeat(64)),
                NOW.into()
            )
            .await,
        Err(AgentConversationError::Corrupt(_)),
    ));
    assert_eq!(store.get_agent(&current.id).await.unwrap(), Some(finished));
    assert_eq!(
        store.agent_message_checkpoint(&current.id).await.unwrap(),
        Some(checkpoint)
    );
    assert_eq!(store.agent_log(&current.id, 0, 100).await.unwrap(), before);
}

async fn close_without_runtime_terminal(store: &MongoSandboxStore, current: &Agent) {
    incremental_recorder(store)
        .record_update(
            &current.id,
            1,
            &mut AgentMessageAssembly::default(),
            chunk("retained prefix"),
            NOW.into(),
        )
        .await
        .unwrap();
    let working = store.get_agent(&current.id).await.unwrap().unwrap();
    let mut mutation = update(&working, "");
    mutation
        .agent
        .transition(AgentStatus::Ready, NOW.into(), None)
        .unwrap();
    mutation.agent.revision = working.revision + 1;
    mutation.agent.conversation.active_turn = None;
    mutation.agent.conversation.last_seq += 1;
    mutation.agent_logs = vec![
        AgentLogKind::TurnInterrupted {
            turn_sequence: working.conversation.active_turn.unwrap(),
        },
        AgentLogKind::StatusChanged {
            from: AgentStatus::Working,
            to: AgentStatus::Ready,
        },
    ];
    let appended = store.commit_agent_conversation(mutation).await.unwrap();
    assert_eq!(appended.len(), 3);
    assert_eq!(
        appended[0].kind,
        AgentLogKind::AgentMessage {
            content: "retained prefix".into()
        }
    );
    assert!(matches!(
        appended[1].kind,
        AgentLogKind::TurnInterrupted { .. }
    ));
    let finished = store.get_agent(&current.id).await.unwrap().unwrap();
    assert_eq!(finished.status, AgentStatus::Ready);
    assert_eq!(finished.conversation.active_turn, None);
    assert_eq!(finished.conversation.last_seq, appended[2].seq);
    assert!(store
        .agent_message_checkpoint(&current.id)
        .await
        .unwrap()
        .unwrap()
        .assembly
        .is_empty());
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn incremental_terminal_commits_one_reply_for_standard_and_omarchy_in_both_log_formats() {
    let (client, name, store) = database().await;
    segments::schema::migrate(&client.database(&name))
        .await
        .unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    for (profile, origin) in [
        ("standard", SandboxRuntimeImageOrigin::ManagedDesktop),
        ("omarchy", SandboxRuntimeImageOrigin::ManagedMachine),
    ] {
        let image = format!(
            "example.invalid/runtime-{profile}@sha256:{}",
            "a".repeat(64)
        );
        for segmented in [false, true] {
            let id = format!("sbx_incremental_{profile}_{segmented}");
            let current = incremental_working(&store, &id, &image, origin, segmented).await;
            let row = store
                .agents
                .find_one(doc! { "_id": current.id.as_str() })
                .await
                .unwrap()
                .unwrap();
            assert_eq!(segments::read_format(&row).unwrap().is_some(), segmented);
            record_incremental_terminal(&store, &current).await;

            let current = incremental_working(
                &store,
                &format!("{id}_lifecycle"),
                &image,
                origin,
                segmented,
            )
            .await;
            close_without_runtime_terminal(&store, &current).await;
        }
    }
    client.database(&name).drop().await.unwrap();
}
