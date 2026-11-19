use super::*;
use crate::sandbox::store::{
    AgentConversationMutation, AgentConversationStore, AgentLogStore, AgentMessageCheckpoint,
    AgentMessageSnapshot, InMemorySandboxStore, Page, PageRequest,
};
use crate::sandbox::{Agent, AgentLogEntry, AgentLogKind, AgentStatus};
use crate::SubjectId;
use layer3_adapter::{ContentBlock, ContentChunk, MessageId, SessionUpdate};
use std::sync::{
    atomic::{AtomicU8, Ordering},
    Mutex,
};

const NOW: &str = "2026-01-01T00:00:00Z";
const PASS: u8 = 0;
const BLOCK_BEFORE: u8 = 1;
const BLOCK_AFTER: u8 = 2;
const CONFLICT: u8 = 3;

struct CommitGate {
    store: InMemorySandboxStore,
    mode: AtomicU8,
    attempts: Mutex<Vec<AgentMessageCheckpoint>>,
}

#[async_trait::async_trait]
impl AgentStore for CommitGate {
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
impl AgentLogStore for CommitGate {
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
impl AgentConversationStore for CommitGate {
    async fn agent_message_snapshot(
        &self,
        id: &AgentId,
    ) -> Result<Option<AgentMessageSnapshot>, SandboxStoreError> {
        self.store.agent_message_snapshot(id).await
    }
    async fn agent_message_checkpoint(
        &self,
        id: &AgentId,
    ) -> Result<Option<AgentMessageCheckpoint>, SandboxStoreError> {
        self.store.agent_message_checkpoint(id).await
    }
    async fn commit_agent_message_update(
        &self,
        current: Agent,
        previous: Option<AgentMessageCheckpoint>,
        next: AgentMessageCheckpoint,
        mutation: Option<AgentConversationMutation>,
    ) -> Result<AgentUpdateOutcome, SandboxStoreError> {
        self.attempts.lock().unwrap().push(next.clone());
        let mode = self.mode.load(Ordering::SeqCst);
        if mode == CONFLICT {
            return Err(SandboxStoreError::Conflict);
        }
        self.mode.store(PASS, Ordering::SeqCst);
        if mode == BLOCK_BEFORE {
            std::future::pending::<()>().await;
        }
        let result = self
            .store
            .commit_agent_message_update(current, previous, next, mutation)
            .await;
        if mode == BLOCK_AFTER {
            assert!(
                result.is_ok(),
                "fixture must commit before withholding its reply: {result:?}"
            );
            std::future::pending::<()>().await;
        }
        result
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

async fn fixture(mode: u8) -> (Arc<CommitGate>, Agent, Arc<DurableAgentUpdateRecorder>) {
    let mut agent = crate::sandbox::lifecycle_test_support::agent(
        "agt_cancelled_recording",
        "synthetic-conversation",
        1,
    );
    agent.status = AgentStatus::Working;
    agent.conversation.active_turn = Some(1);
    let store = InMemorySandboxStore::with_agent_fixture(agent.clone());
    store
        .append_agent_log(
            &agent.id,
            NOW.into(),
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 1,
                origin_key: None,
            },
        )
        .await
        .unwrap();
    let store = Arc::new(CommitGate {
        store,
        mode: AtomicU8::new(mode),
        attempts: Mutex::new(Vec::new()),
    });
    let recorder = DurableAgentUpdateRecorder::new(store.clone());
    (store, agent, recorder)
}

fn chunk(content: &str) -> ConversationUpdate {
    ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(
        ContentChunk::new(ContentBlock::from(content))
            .message_id(MessageId::new("synthetic-message")),
    ))
}

#[tokio::test]
async fn cancelled_recording_preserves_exact_identity_before_or_after_commit() {
    for mode in [BLOCK_BEFORE, BLOCK_AFTER] {
        let (store, agent, recorder) = fixture(mode).await;
        let update = chunk("retained once");
        let mut session = recorder.recording_session();
        let mut operation = Box::pin(recorder.record_session_update(
            &agent.id,
            4,
            &mut session,
            &update,
            NOW.into(),
        ));
        assert!(futures_util::poll!(&mut operation).is_pending());
        drop(operation);
        let pending = session
            .assembly()
            .pending_update
            .clone()
            .expect("cancellation lost the pending commit identity");
        let original = store.attempts.lock().unwrap()[0].clone();
        assert_eq!(pending.id, original.update_id);
        assert_eq!(pending.fingerprint, original.update_fingerprint);
        assert_eq!(
            pending.native_conversation_id,
            original.native_conversation_id
        );
        assert_eq!(pending.runtime_generation, 4);
        assert_eq!(pending.active_turn, agent.conversation.active_turn);
        assert_eq!(pending.after_seq, agent.conversation.last_seq);

        let refused = recorder
            .record_session_update(
                &agent.id,
                4,
                &mut session,
                &chunk("different input"),
                NOW.into(),
            )
            .await
            .unwrap_err();
        assert!(matches!(&*refused, AgentConversationError::InvalidState(_)));
        assert_eq!(session.assembly().pending_update.as_ref(), Some(&pending));
        assert_eq!(store.attempts.lock().unwrap().len(), 1);

        recorder
            .record_session_update(&agent.id, 4, &mut session, &update, NOW.into())
            .await
            .unwrap();
        assert!(session.assembly().pending_update.is_none());
        assert_eq!(session.assembly().content, "retained once");
        let checkpoint = store
            .agent_message_checkpoint(&agent.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(checkpoint.update_id, original.update_id);
        assert_eq!(checkpoint.update_fingerprint, original.update_fingerprint);
        assert_eq!(checkpoint.assembly.content, "retained once");
        let attempts = store.attempts.lock().unwrap();
        assert_eq!(attempts.len(), if mode == BLOCK_AFTER { 1 } else { 2 });
        assert!(attempts
            .iter()
            .all(|attempt| attempt.update_id == original.update_id
                && attempt.update_fingerprint == original.update_fingerprint));
    }
}

#[tokio::test]
async fn known_abort_restores_prior_uncertainty_without_inventing_new_uncertainty() {
    for cancelled in [false, true] {
        let (store, agent, recorder) =
            fixture(if cancelled { BLOCK_BEFORE } else { CONFLICT }).await;
        let update = chunk("same pending input");
        let mut session = recorder.recording_session();
        if cancelled {
            let mut operation = Box::pin(recorder.record_session_update(
                &agent.id,
                4,
                &mut session,
                &update,
                NOW.into(),
            ));
            assert!(futures_util::poll!(&mut operation).is_pending());
            drop(operation);
            assert!(session.assembly().pending_update.is_some());
            store.mode.store(CONFLICT, Ordering::SeqCst);
        }
        let before = session.assembly().pending_update.clone();
        let error = recorder
            .record_session_update(&agent.id, 4, &mut session, &update, NOW.into())
            .await
            .unwrap_err();
        assert!(matches!(&*error, AgentConversationError::Unavailable(_)));
        assert_eq!(session.assembly().pending_update, before);
        assert!(store
            .agent_message_checkpoint(&agent.id)
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            store.attempts.lock().unwrap().len(),
            MAX_CONVERSATION_COMMIT_RETRIES + usize::from(cancelled)
        );
    }
}

#[tokio::test]
async fn uncertain_retry_never_moves_to_a_different_native_conversation() {
    for mode in [BLOCK_BEFORE, BLOCK_AFTER] {
        let (store, agent, recorder) = fixture(mode).await;
        let update = chunk("bound to the original conversation");
        let mut session = recorder.recording_session();
        let mut operation = Box::pin(recorder.record_session_update(
            &agent.id,
            4,
            &mut session,
            &update,
            NOW.into(),
        ));
        assert!(futures_util::poll!(&mut operation).is_pending());
        drop(operation);
        let original = session.assembly().pending_update.clone().unwrap();
        let mut changed = store.get_agent(&agent.id).await.unwrap().unwrap();
        let revision = changed.revision;
        changed.revision += 1;
        changed.conversation.conversation_id = Some("different-conversation".into());
        store.save_agent(revision, changed).await.unwrap();

        let error = recorder
            .record_session_update(&agent.id, 4, &mut session, &update, NOW.into())
            .await
            .expect_err("uncertain input crossed its original conversation boundary");
        assert!(matches!(&*error, AgentConversationError::Unavailable(_)));
        assert_eq!(session.assembly().pending_update.as_ref(), Some(&original));
        assert_eq!(store.attempts.lock().unwrap().len(), 1);

        let mut restored = store.get_agent(&agent.id).await.unwrap().unwrap();
        let revision = restored.revision;
        restored.revision += 1;
        restored.conversation.conversation_id = agent.conversation.conversation_id.clone();
        store.save_agent(revision, restored).await.unwrap();
        recorder
            .record_session_update(&agent.id, 4, &mut session, &update, NOW.into())
            .await
            .unwrap();
        assert!(session.assembly().pending_update.is_none());
        assert_eq!(
            session.assembly().content,
            "bound to the original conversation"
        );
    }
}
