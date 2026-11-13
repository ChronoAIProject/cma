use super::*;
use crate::sandbox::store::{AgentLogFinalAnswer, AgentReadQueryStore, Page, SandboxStoreError};

struct Probe {
    inner: Arc<InMemorySandboxStore>,
    fail: bool,
}

macro_rules! delegate_entity {
    ($store:ident, $entity:ty, $id:ty, $create:ident, $get:ident, $save:ident, $list:ident, $parent:ty) => {
        #[async_trait]
        impl $store for Probe {
            async fn $create(&self, _: $entity) -> Result<(), SandboxStoreError> {
                unreachable!("read path mutated an entity")
            }
            async fn $get(&self, id: &$id) -> Result<Option<$entity>, SandboxStoreError> {
                self.inner.$get(id).await
            }
            async fn $save(&self, _: u64, _: $entity) -> Result<(), SandboxStoreError> {
                unreachable!("read path mutated an entity")
            }
            async fn $list(
                &self,
                _: &$parent,
                _: &PageRequest,
            ) -> Result<Page<$entity>, SandboxStoreError> {
                unreachable!("event window listed entities")
            }
        }
    };
}
delegate_entity!(
    SandboxStore,
    Sandbox,
    SandboxId,
    create_sandbox,
    get_sandbox,
    save_sandbox,
    list_sandboxes,
    SubjectId
);
delegate_entity!(
    WorkspaceStore,
    Workspace,
    WorkspaceId,
    create_workspace,
    get_workspace,
    save_workspace,
    list_workspaces,
    SandboxId
);
delegate_entity!(
    WorktreeStore,
    Worktree,
    WorktreeId,
    create_worktree,
    get_worktree,
    save_worktree,
    list_worktrees,
    WorkspaceId
);
delegate_entity!(
    AgentStore,
    Agent,
    AgentId,
    create_agent,
    get_agent,
    save_agent,
    list_agents,
    SubjectId
);
#[async_trait]
impl AgentReadQueryStore for Probe {
    async fn list_owned_agents(
        &self,
        _: &SubjectId,
        _: &AgentListQuery,
        _: &PageRequest,
    ) -> Result<Page<Agent>, SandboxStoreError> {
        unreachable!("event window listed Agents")
    }
}
#[async_trait]
impl AgentLogStore for Probe {
    async fn append_agent_log(
        &self,
        _: &AgentId,
        _: String,
        _: AgentLogKind,
    ) -> Result<AgentLogEntry, SandboxStoreError> {
        panic!("read mutated history")
    }
    async fn agent_log(
        &self,
        _: &AgentId,
        _: u64,
        _: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        panic!("certified consumer fell back to history")
    }
    async fn agent_log_transcript_projection(
        &self,
        _: &Agent,
    ) -> Result<Option<AgentTranscript>, SandboxStoreError> {
        Err(SandboxStoreError::AgentLogCapacityExceeded)
    }
    async fn agent_log_final_answer(
        &self,
        _: &Agent,
        sequence: u64,
    ) -> Result<Option<AgentLogFinalAnswer>, SandboxStoreError> {
        if self.fail {
            return Err(SandboxStoreError::Corrupt("missing evidence".into()));
        }
        if sequence != 5 {
            return Err(SandboxStoreError::NotFound);
        }
        Ok(Some(AgentLogFinalAnswer {
            turn_sequence: 3,
            message: AgentTranscriptMessage {
                sequence,
                role: AgentTranscriptRole::Agent,
                content: "final answer".into(),
                appended_at: NOW.into(),
            },
        }))
    }
}

async fn fixture(fail: bool) -> (Arc<AgentReadService>, Agent) {
    let inner = Arc::new(InMemorySandboxStore::default());
    let agent = insert_hierarchy(
        &inner,
        "agt_bounded",
        "wt_bounded",
        "ws_bounded",
        "sb_bounded",
        "owner",
        AgentStatus::Starting,
    )
    .await;
    let agent = replace_history(
        &inner,
        agent,
        vec![
            AgentLogKind::ConversationOpened {
                conversation_id: "native".into(),
                runtime_generation: 1,
            },
            AgentLogKind::TurnStarted {
                turn_sequence: 3,
                origin_key: None,
                model: None,
            },
            AgentLogKind::OwnerMessage {
                content: "owner".into(),
            },
            AgentLogKind::AgentMessage {
                content: "final answer".into(),
            },
            AgentLogKind::TurnCompleted { turn_sequence: 3 },
        ],
        Some("native"),
        Some(1),
        None,
    )
    .await;
    let reads = AgentReadService::new(
        Arc::new(Probe { inner, fail }),
        Arc::new(StubResources::reporting(SandboxResourceReading::default())),
    );
    (reads, agent)
}

#[tokio::test]
async fn certified_answers_and_feedback_preserve_eligibility_and_owner_fences() {
    for fail in [false, true] {
        let (reads, agent) = fixture(fail).await;
        let owner = caller("owner", true);
        let feedback =
            MessageFeedbackService::new(reads.clone(), Arc::new(FeedbackStore::default()));
        if fail {
            assert!(matches!(
                reads.final_answer(&owner, None, None, &agent.id, 5).await,
                Err(AgentReadError::Corrupt(_))
            ));
            assert!(matches!(
                feedback
                    .set(&owner, &agent.id, 5, Some(MessageVote::Up))
                    .await,
                Err(AgentReadError::Corrupt(_))
            ));
        } else {
            assert_eq!(
                reads
                    .final_answer(&owner, None, None, &agent.id, 5)
                    .await
                    .unwrap()
                    .message
                    .sequence,
                5
            );
            feedback
                .set(&owner, &agent.id, 5, Some(MessageVote::Up))
                .await
                .unwrap();
            assert!(matches!(
                reads.final_answer(&owner, None, None, &agent.id, 4).await,
                Err(AgentReadError::NotFound)
            ));
            assert!(matches!(
                feedback
                    .set(&owner, &agent.id, 4, Some(MessageVote::Up))
                    .await,
                Err(AgentReadError::NotFound)
            ));
        }
        assert!(matches!(
            reads
                .final_answer(&caller("other", true), None, None, &agent.id, 5)
                .await,
            Err(AgentReadError::NotFound)
        ));
    }
}

#[tokio::test]
async fn personal_transcript_uses_certified_projection_and_requests_pagination() {
    let (reads, agent) = fixture(false).await;
    assert!(matches!(
        reads.transcript(&caller("owner", true), &agent.id).await,
        Err(AgentReadError::TranscriptPaginationRequired)
    ));
    assert!(matches!(
        reads.transcript(&caller("other", true), &agent.id).await,
        Err(AgentReadError::NotFound)
    ));
}
