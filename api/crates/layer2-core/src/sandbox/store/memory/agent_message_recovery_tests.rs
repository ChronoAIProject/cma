use super::*;
use crate::sandbox::store::{AgentMessageCheckpoint, Page, PageRequest, SandboxStoreError};
use crate::sandbox::{Agent, AgentUpdateOutcome, MAX_DURABLE_AGENT_MESSAGE_BYTES};
use crate::{AgentId, SubjectId};
use std::sync::atomic::{AtomicBool, Ordering};

#[path = "agent_message_anchor_tests.rs"]
mod anchor_tests;

#[path = "agent_output_recovery_tests.rs"]
mod output_tests;

fn messages(entries: &[AgentLogEntry]) -> Vec<String> {
    entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            AgentLogKind::AgentMessage { content } => Some(content.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn replacement_recorder_recovers_private_prefix_before_suffix_and_terminal() {
    let (store, working, recorder) = working_agent().await;
    let original_agent_json = serde_json::to_value(&working).unwrap();
    let original_log = store.agent_log(&working.id, 0, 200).await.unwrap();
    let mut assembly = AgentMessageAssembly::default();
    recorder
        .record_update(
            &working.id,
            1,
            &mut assembly,
            message_chunk("reply", "prefix "),
            NOW.into(),
        )
        .await
        .unwrap();
    assert_eq!(
        store.get_agent(&working.id).await.unwrap(),
        Some(working.clone())
    );
    assert_eq!(
        store.agent_log(&working.id, 0, 200).await.unwrap(),
        original_log
    );
    assert_eq!(
        serde_json::to_value(store.get_agent(&working.id).await.unwrap().unwrap()).unwrap(),
        original_agent_json
    );
    drop(recorder);
    let replacement = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    let mut live = replacement.subscribe_live(&working.id).unwrap();
    let mut fresh = AgentMessageAssembly::default();
    replacement
        .record_update(
            &working.id,
            1,
            &mut fresh,
            message_chunk("reply", "suffix"),
            NOW.into(),
        )
        .await
        .unwrap();
    assert_eq!(fresh.content, "prefix suffix");
    assert_eq!(live.try_recv().unwrap().delta, "suffix");
    // A second replacement can receive only the terminal frame.
    let terminal_recorder = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    let outcome = terminal_recorder
        .record_update(
            &working.id,
            1,
            &mut AgentMessageAssembly::default(),
            turn_ended(),
            NOW.into(),
        )
        .await
        .unwrap();
    assert_eq!(messages(&outcome.appended), ["prefix suffix"]);
    assert!(outcome.turn_closed);
    let checkpoint = store
        .agent_message_checkpoint(&working.id)
        .await
        .unwrap()
        .unwrap();
    assert!(checkpoint.assembly.is_empty());
    assert_eq!(checkpoint.active_turn, None);
    assert_eq!(
        messages(&store.agent_log(&working.id, 0, 200).await.unwrap()),
        ["prefix suffix"]
    );
}

#[tokio::test]
async fn semantic_boundary_commits_old_message_and_recovers_only_new_prefix() {
    let (store, working, recorder) = working_agent().await;
    let mut assembly = AgentMessageAssembly::default();
    recorder
        .record_update(
            &working.id,
            1,
            &mut assembly,
            message_chunk("first", "first answer"),
            NOW.into(),
        )
        .await
        .unwrap();
    let boundary = recorder
        .record_update(
            &working.id,
            1,
            &mut assembly,
            message_chunk("second", "second "),
            NOW.into(),
        )
        .await
        .unwrap();
    assert_eq!(messages(&boundary.appended), ["first answer"]);
    let replacement = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    let mut fresh = AgentMessageAssembly::default();
    replacement
        .record_update(
            &working.id,
            1,
            &mut fresh,
            message_chunk("second", "answer"),
            NOW.into(),
        )
        .await
        .unwrap();
    replacement
        .record_update(&working.id, 1, &mut fresh, turn_ended(), NOW.into())
        .await
        .unwrap();
    assert_eq!(
        messages(&store.agent_log(&working.id, 0, 200).await.unwrap()),
        ["first answer", "second answer"]
    );
}

#[tokio::test]
async fn foreign_native_generation_or_turn_cannot_restore_message_content() {
    for change in 0..3 {
        let (store, working, recorder) = working_agent().await;
        recorder
            .record_update(
                &working.id,
                1,
                &mut AgentMessageAssembly::default(),
                message_chunk("reply", "foreign prefix "),
                NOW.into(),
            )
            .await
            .unwrap();
        {
            let mut state = store.lock();
            let checkpoint = state
                .agent_message_checkpoints
                .get_mut(&working.id)
                .unwrap();
            match change {
                0 => checkpoint.native_conversation_id = "different-conversation".into(),
                1 => checkpoint.runtime_generation += 1,
                _ => checkpoint.active_turn = Some(1),
            }
        }
        let replacement = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
        let mut fresh = AgentMessageAssembly::default();
        replacement
            .record_update(
                &working.id,
                1,
                &mut fresh,
                message_chunk("reply", "current"),
                NOW.into(),
            )
            .await
            .unwrap();
        replacement
            .record_update(&working.id, 1, &mut fresh, turn_ended(), NOW.into())
            .await
            .unwrap();
        assert_eq!(
            messages(&store.agent_log(&working.id, 0, 200).await.unwrap()),
            ["current"]
        );
    }
}

#[tokio::test]
async fn stale_log_anchor_or_oversized_checkpoint_fails_without_public_mutation() {
    for oversized in [false, true] {
        let (store, working, recorder) = working_agent().await;
        recorder
            .record_update(
                &working.id,
                1,
                &mut AgentMessageAssembly::default(),
                message_chunk("reply", "prefix"),
                NOW.into(),
            )
            .await
            .unwrap();
        let before = store.agent_log(&working.id, 0, 200).await.unwrap();
        {
            let mut state = store.lock();
            let checkpoint = state
                .agent_message_checkpoints
                .get_mut(&working.id)
                .unwrap();
            if oversized {
                checkpoint.assembly.content = "x".repeat(MAX_DURABLE_AGENT_MESSAGE_BYTES + 1);
            } else {
                checkpoint.after_seq += 1;
            }
        }
        let mut fresh = AgentMessageAssembly::default();
        assert!(matches!(
            recorder
                .record_update(&working.id, 1, &mut fresh, turn_ended(), NOW.into())
                .await,
            Err(AgentConversationError::Corrupt(_))
        ));
        assert!(fresh.is_empty());
        assert_eq!(store.agent_log(&working.id, 0, 200).await.unwrap(), before);
        assert_eq!(store.get_agent(&working.id).await.unwrap(), Some(working));
    }
}

#[tokio::test]
async fn unrelated_semantic_write_advances_private_anchor_without_public_message() {
    let (store, working, recorder) = working_agent().await;
    recorder
        .record_update(
            &working.id,
            1,
            &mut AgentMessageAssembly::default(),
            message_chunk("reply", "prefix "),
            NOW.into(),
        )
        .await
        .unwrap();
    let mut next = working.clone();
    next.revision += 1;
    next.conversation.last_seq += 1;
    store
        .commit_agent_conversation(AgentConversationMutation {
            command_claim: None,
            execution_fence: None,
            trigger_execution_fence: None,
            admission_guard: None,
            expected_agent_revision: working.revision,
            expected_last_seq: working.conversation.last_seq,
            runtime_generation: 1,
            agent: next.clone(),
            appended_at: NOW.into(),
            agent_logs: vec![AgentLogKind::ConversationGap { frames: 1 }],
        })
        .await
        .unwrap();
    assert!(store
        .agent_message_checkpoint(&working.id)
        .await
        .unwrap()
        .unwrap()
        .matches(&next));
    let replacement = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    let mut fresh = AgentMessageAssembly::default();
    replacement
        .record_update(
            &working.id,
            1,
            &mut fresh,
            message_chunk("reply", "suffix"),
            NOW.into(),
        )
        .await
        .unwrap();
    replacement
        .record_update(&working.id, 1, &mut fresh, turn_ended(), NOW.into())
        .await
        .unwrap();
    assert_eq!(
        messages(&store.agent_log(&working.id, 0, 200).await.unwrap()),
        ["prefix suffix"]
    );
}

struct LostCommitReply {
    store: super::super::InMemorySandboxStore,
    fail_next: AtomicBool,
    advance_on_read: AtomicBool,
    publish_on_commit: AtomicBool,
}

#[async_trait::async_trait]
impl AgentLogStore for LostCommitReply {
    async fn append_agent_log(
        &self,
        id: &AgentId,
        at: String,
        kind: AgentLogKind,
    ) -> Result<AgentLogEntry, SandboxStoreError> {
        self.store.append_agent_log(id, at, kind).await
    }

    async fn agent_log(
        &self,
        id: &AgentId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        self.store.agent_log(id, after, limit).await
    }
}

#[async_trait::async_trait]
impl AgentStore for LostCommitReply {
    async fn create_agent(&self, agent: Agent) -> Result<(), SandboxStoreError> {
        self.store.create_agent(agent).await
    }
    async fn get_agent(&self, id: &AgentId) -> Result<Option<Agent>, SandboxStoreError> {
        self.store.get_agent(id).await
    }
    async fn save_agent(&self, revision: u64, agent: Agent) -> Result<(), SandboxStoreError> {
        self.store.save_agent(revision, agent).await
    }
    async fn list_agents(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<Agent>, SandboxStoreError> {
        self.store.list_agents(owner, page).await
    }
}

#[async_trait::async_trait]
impl AgentConversationStore for LostCommitReply {
    async fn agent_message_checkpoint(
        &self,
        id: &AgentId,
    ) -> Result<Option<AgentMessageCheckpoint>, SandboxStoreError> {
        if self.advance_on_read.swap(false, Ordering::SeqCst) {
            let current = self.store.get_agent(id).await?.unwrap();
            let mut next = current.clone();
            next.revision += 1;
            next.conversation.last_seq += 1;
            self.store
                .commit_agent_conversation(AgentConversationMutation {
                    command_claim: None,
                    execution_fence: None,
                    trigger_execution_fence: None,
                    admission_guard: None,
                    expected_agent_revision: current.revision,
                    expected_last_seq: current.conversation.last_seq,
                    runtime_generation: 1,
                    agent: next,
                    appended_at: NOW.into(),
                    agent_logs: vec![AgentLogKind::ConversationGap { frames: 1 }],
                })
                .await?;
        }
        self.store.agent_message_checkpoint(id).await
    }
    async fn commit_agent_message_update(
        &self,
        current: Agent,
        previous: Option<AgentMessageCheckpoint>,
        next: AgentMessageCheckpoint,
        mutation: Option<AgentConversationMutation>,
    ) -> Result<AgentUpdateOutcome, SandboxStoreError> {
        if self.publish_on_commit.swap(false, Ordering::SeqCst) {
            anchor_tests::publish_connection(&self.store, &current.id).await;
        }
        let outcome = self
            .store
            .commit_agent_message_update(current, previous, next, mutation)
            .await?;
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err(SandboxStoreError::Unavailable(
                "simulated lost commit reply".into(),
            ));
        }
        Ok(outcome)
    }
    async fn commit_agent_conversation(
        &self,
        mutation: AgentConversationMutation,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        self.store.commit_agent_conversation(mutation).await
    }
    async fn pending_agent_approvals(
        &self,
        id: &AgentId,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        self.store.pending_agent_approvals(id).await
    }
}

#[tokio::test]
async fn uncertain_checkpoint_and_terminal_retries_recover_exact_original_outcome() {
    let (store, working, _) = working_agent().await;
    let flaky = Arc::new(LostCommitReply {
        store: store.clone(),
        fail_next: AtomicBool::new(true),
        advance_on_read: AtomicBool::new(false),
        publish_on_commit: AtomicBool::new(false),
    });
    let recorder = DurableAgentUpdateRecorder::new(flaky.clone());
    let mut live = recorder.subscribe_live(&working.id).unwrap();
    let mut assembly = AgentMessageAssembly::default();
    let chunk = message_chunk("reply", "exact answer");
    assert!(matches!(
        recorder
            .record_update(&working.id, 1, &mut assembly, chunk.clone(), NOW.into())
            .await,
        Err(AgentConversationError::Unavailable(_))
    ));
    assert!(
        live.try_recv().is_err(),
        "unconfirmed persistence must not publish live text"
    );
    let checkpoint = store
        .agent_message_checkpoint(&working.id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        recorder
            .record_update(
                &working.id,
                1,
                &mut assembly,
                message_chunk("reply", "different"),
                NOW.into()
            )
            .await,
        Err(AgentConversationError::InvalidState(_))
    ));
    recorder
        .record_update(&working.id, 1, &mut assembly, chunk, NOW.into())
        .await
        .unwrap();
    assert_eq!(assembly.content, "exact answer");
    assert_eq!(
        store
            .agent_message_checkpoint(&working.id)
            .await
            .unwrap()
            .unwrap(),
        checkpoint
    );
    flaky.fail_next.store(true, Ordering::SeqCst);
    assert!(matches!(
        recorder
            .record_update(&working.id, 1, &mut assembly, turn_ended(), NOW.into())
            .await,
        Err(AgentConversationError::Unavailable(_))
    ));
    let terminal_checkpoint = store
        .agent_message_checkpoint(&working.id)
        .await
        .unwrap()
        .unwrap();
    let outcome = recorder
        .record_update(&working.id, 1, &mut assembly, turn_ended(), NOW.into())
        .await
        .unwrap();
    assert_eq!(outcome, terminal_checkpoint.outcome);
    assert!(outcome.turn_closed);
    assert!(assembly.is_empty());
    assert_eq!(
        messages(&store.agent_log(&working.id, 0, 200).await.unwrap()),
        ["exact answer"]
    );
}

#[tokio::test]
async fn semantic_write_between_agent_and_checkpoint_reads_retries_without_losing_prefix() {
    let (store, working, recorder) = working_agent().await;
    recorder
        .record_update(
            &working.id,
            1,
            &mut AgentMessageAssembly::default(),
            message_chunk("reply", "prefix "),
            NOW.into(),
        )
        .await
        .unwrap();
    let interleaved = Arc::new(LostCommitReply {
        store: store.clone(),
        fail_next: AtomicBool::new(false),
        advance_on_read: AtomicBool::new(true),
        publish_on_commit: AtomicBool::new(false),
    });
    let replacement = DurableAgentUpdateRecorder::new(interleaved);
    let mut fresh = AgentMessageAssembly::default();
    replacement
        .record_update(
            &working.id,
            1,
            &mut fresh,
            message_chunk("reply", "suffix"),
            NOW.into(),
        )
        .await
        .unwrap();
    replacement
        .record_update(&working.id, 1, &mut fresh, turn_ended(), NOW.into())
        .await
        .unwrap();
    let logs = store.agent_log(&working.id, 0, 200).await.unwrap();
    assert_eq!(messages(&logs), ["prefix suffix"]);
    assert_eq!(
        logs.iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::ConversationGap { frames: 1 }))
            .count(),
        1
    );
}
