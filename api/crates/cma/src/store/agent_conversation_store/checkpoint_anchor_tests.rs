use std::sync::Arc;

use layer2_core::sandbox::store::{
    AgentConnectionRecoveryCandidate, AgentWorkMutation, AgentWorkerStore,
};
use layer2_core::sandbox::{
    AgentUpdateRecorder, AgentWorkStep, DurableAgentUpdateRecorder, Worktree, WorktreeStatus,
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

pub(super) async fn publish_legacy_connection(store: &MongoSandboxStore, id: &AgentId) {
    let previous = store.agent_message_checkpoint(id).await.unwrap().unwrap();
    let current = store.get_agent(id).await.unwrap().unwrap();
    publish_connection(store, id).await;
    let published = store.get_agent(id).await.unwrap().unwrap();
    assert_eq!(
        store.agent_message_checkpoint(id).await.unwrap(),
        previous.advance(&current, &published)
    );
    // Preserve the exact old v1 sidecar to reproduce an already-stranded fixture.
    store
        .agents
        .update_one(
            doc! { "_id": id.as_str() },
            doc! { "$set": {
                checkpoint::FIELD: crate::store::sandbox_mongo::record_bson(&previous).unwrap(),
            }},
        )
        .await
        .unwrap();
}

pub(super) async fn publish_connection(store: &MongoSandboxStore, id: &AgentId) {
    let current = store.get_agent(id).await.unwrap().unwrap();
    store
        .schedule_agent_connection_recovery(
            AgentConnectionRecoveryCandidate::from_agent(&current)
                .unwrap()
                .restart_request(NOW.into()),
        )
        .await
        .unwrap();
    let claimed = store
        .claim_agent_work(id, "reconnector", NOW.into(), "2999-01-01T00:00:00Z".into())
        .await
        .unwrap()
        .unwrap();
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let mut next = claimed.clone();
    next.work
        .advance("reconnector", fence, AgentWorkStep::None, NOW.into())
        .unwrap();
    next.revision += 1;
    next.conversation.last_seq += 1;
    next.updated_at = NOW.into();
    store
        .commit_agent_work(AgentWorkMutation {
            expected_agent_revision: claimed.revision,
            holder: "reconnector".into(),
            fence,
            agent: next,
            worktree: None,
            appended_at: NOW.into(),
            sandbox_event: None,
            agent_logs: vec![AgentLogKind::WorkStepCompleted {
                step: AgentWorkStep::PublishConnection,
            }],
        })
        .await
        .unwrap();
}

pub(super) async fn working_with_worktree(store: &MongoSandboxStore, id: &str) -> Agent {
    let current = working(store, id).await;
    let worktree = Worktree {
        id: current.worktree_id.clone(),
        workspace_id: current.workspace_id.clone(),
        sandbox_id: current.sandbox_id.clone(),
        owner: current.owner.clone(),
        name: "wt-1".into(),
        path: "/home/cma/project/.worktrees/wt-1".into(),
        branch: "cma/wt-1".into(),
        base_commit: "0123456789abcdef".into(),
        status: WorktreeStatus::Ready,
        agent_id: Some(current.id.clone()),
        revision: 1,
    };
    store
        .worktrees
        .insert_one(crate::store::worktrees::worktree_document(&worktree).unwrap())
        .await
        .unwrap();
    current
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn replacement_mongo_recorder_preserves_v1_prefix_across_connection_publications() {
    for suffix in [None, Some("suffix")] {
        let (client, name, store) = database().await;
        let current = working_with_worktree(&store, "sbx_checkpoint_connection").await;
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
        let original_checkpoint = store
            .agent_message_checkpoint(&current.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(original_checkpoint.version, 1);
        for _ in 0..9 {
            publish_legacy_connection(&store, &current.id).await;
        }
        assert_eq!(
            store.agent_message_checkpoint(&current.id).await.unwrap(),
            Some(original_checkpoint)
        );
        let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
        let replacement = DurableAgentUpdateRecorder::new(Arc::new(reopened.clone()));
        if let Some(suffix) = suffix {
            replacement
                .record_update(
                    &current.id,
                    1,
                    &mut AgentMessageAssembly::default(),
                    chunk(suffix),
                    NOW.into(),
                )
                .await
                .unwrap();
        }
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
        let logs = reopened.agent_log(&current.id, 0, 200).await.unwrap();
        let messages: Vec<_> = logs
            .iter()
            .filter_map(|entry| match &entry.kind {
                AgentLogKind::AgentMessage { content } => Some(content.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            messages,
            [if suffix.is_some() {
                "prefix suffix"
            } else {
                "prefix "
            }]
        );
        assert_eq!(
            logs.iter()
                .filter(|entry| matches!(entry.kind, AgentLogKind::TurnStarted { .. }))
                .count(),
            1
        );
        assert_eq!(
            logs.iter()
                .filter(|entry| matches!(
                    entry.kind,
                    AgentLogKind::WorkStepCompleted {
                        step: AgentWorkStep::PublishConnection
                    }
                ))
                .count(),
            9
        );
        let finished = reopened.get_agent(&current.id).await.unwrap().unwrap();
        assert!(reopened
            .agent_message_checkpoint(&current.id)
            .await
            .unwrap()
            .unwrap()
            .matches(&finished));
        assert_eq!(finished.status, AgentStatus::Ready);
        client.database(&name).drop().await.unwrap();
    }
}
