use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::profiles::{
    AgentProfileContent, AgentProfileSnapshot, AgentProvider, ModelAccess, PermissionLevel,
    SandboxProfileContent, SandboxProfileSnapshot, ThinkingEffort, WorkspaceLocation,
    WorkspaceProfileContent, WorkspaceProfileSnapshot,
};
use crate::{
    AgentId, AgentProfileId, Authorization, Caller, Capability, RuntimeCapabilities,
    RuntimeCapability, SandboxId, SandboxProfileId, SubjectId, WorkspaceId, WorkspaceProfileId,
    WorktreeId,
};

use super::store::{
    AgentLogStore, AgentStore, InMemorySandboxStore, PageRequest, SandboxStore, WorkspaceStore,
    WorktreeStore,
};
use super::*;

const NOW: &str = "2026-09-05T00:00:00Z";

fn caller(subject: &str, allowed: bool) -> Caller {
    Caller::new(
        SubjectId::new(subject),
        Authorization::new(allowed.then_some(Capability::UseOwnSandboxes)),
    )
}

fn sandbox(id: &str, owner: &str) -> Sandbox {
    Sandbox {
        id: SandboxId(id.into()),
        owner: SubjectId::new(owner),
        name: id.into(),
        origin: SandboxOrigin::Manual,
        profile: SandboxProfileSnapshot {
            id: SandboxProfileId(format!("profile-{id}")),
            revision: 1,
            digest: "a".repeat(64),
            content: SandboxProfileContent {
                cpu_millis: 1_000,
                memory_mib: 2_048,
                nyxid_services: vec![],
                agent_provider: AgentProvider::Codex,
                model_access: ModelAccess::ChronoaiManaged,
                max_workspaces: 16,
                max_agents_per_workspace: 16,
                workspace_profiles: vec![],
                software_install_commands: vec![],
                environment_variables: vec![],
            },
        },
        status: SandboxStatus::Active,
        failure: None,
        runtime: None,
        credential: SandboxCredentialState {
            reaches: vec![],
            key_generation: 0,
            delivered: false,
            current: None,
            pending: None,
            revoke_debt: vec![],
            sign_in: SandboxSignInState::NotRequired,
        },
        work: SandboxWorkState::new(SandboxWorkStep::None, NOW.into()),
        limits: SandboxLimits {
            max_workspaces: 16,
            max_agents_per_workspace: 16,
        },
        lifecycle: Default::default(),
        last_activity_at: NOW.into(),
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
        purge_after: None,
        revision: 1,
    }
}

fn workspace(id: &str, sandbox_id: &str, owner: &str) -> Workspace {
    Workspace {
        id: WorkspaceId(id.into()),
        sandbox_id: SandboxId(sandbox_id.into()),
        owner: SubjectId::new(owner),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: WorkspaceProfileId(format!("profile-{id}")),
            revision: 1,
            digest: "b".repeat(64),
            content: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory { name: id.into() },
                name: id.into(),
                setup_commands: vec![],
                agent_profiles: vec![],
            },
        },
        name: id.into(),
        path: format!("/home/cma/{id}"),
        source: WorkspaceSource::Empty,
        resolved_commit: Some("c".repeat(40)),
        status: WorkspaceStatus::Ready,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::None, NOW.into()),
        next_worktree_number: 1,
        created_at: NOW.into(),
        ready_at: Some(NOW.into()),
        failed: None,
        removed_at: None,
        revision: 1,
    }
}

fn worktree(
    id: &str,
    workspace_id: &str,
    sandbox_id: &str,
    agent_id: &str,
    owner: &str,
) -> Worktree {
    Worktree {
        id: WorktreeId(id.into()),
        workspace_id: WorkspaceId(workspace_id.into()),
        sandbox_id: SandboxId(sandbox_id.into()),
        owner: SubjectId::new(owner),
        name: format!("wt-{id}"),
        path: format!("/home/cma/{workspace_id}/.worktrees/{id}"),
        branch: format!("cma/{id}"),
        base_commit: "c".repeat(40),
        status: WorktreeStatus::Ready,
        agent_id: Some(AgentId(agent_id.into())),
        revision: 1,
    }
}

fn agent(
    id: &str,
    worktree_id: &str,
    workspace_id: &str,
    sandbox_id: &str,
    owner: &str,
    status: AgentStatus,
) -> Agent {
    Agent {
        id: AgentId(id.into()),
        worktree_id: WorktreeId(worktree_id.into()),
        workspace_id: WorkspaceId(workspace_id.into()),
        sandbox_id: SandboxId(sandbox_id.into()),
        owner: SubjectId::new(owner),
        creation_key: format!("manual:{id}"),
        creation_fingerprint: "d".repeat(64),
        profile: AgentProfileSnapshot {
            id: AgentProfileId(format!("profile-{id}")),
            revision: 1,
            digest: "e".repeat(64),
            content: AgentProfileContent {
                model: Some("model-a".into()),
                thinking_effort: ThinkingEffort::High,
                permission_level: PermissionLevel::Guarded,
                skills: vec![],
                system_prompt: String::new(),
            },
        },
        capabilities: RuntimeCapabilities::new([
            RuntimeCapability::Conversation,
            RuntimeCapability::Usage,
            RuntimeCapability::Resources,
        ]),
        status,
        conversation: ConversationState {
            conversation_id: None,
            runtime_generation: None,
            last_seq: 1,
            first_message: None,
            active_turn: None,
            operation: None,
            queued_messages: vec![],
        },
        work: AgentWorkState::new(AgentWorkStep::None, NOW.into()),
        failure: None,
        title: None,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        ready_at: None,
        stopped_at: None,
        requested_stop_reason: None,
        stop_reason: None,
        deleted_at: None,
        origin: AgentOrigin::Manual,
        revision: 1,
    }
}

async fn insert_agent(store: &InMemorySandboxStore, agent: Agent, worktree: Worktree) -> Agent {
    store.create_worktree(worktree).await.unwrap();
    store.create_agent(agent.clone()).await.unwrap();
    store
        .append_agent_log(
            &agent.id,
            NOW.into(),
            AgentLogKind::Created {
                workspace_id: agent.workspace_id.clone(),
                worktree_id: agent.worktree_id.clone(),
            },
        )
        .await
        .unwrap();
    agent
}

async fn insert_hierarchy(
    store: &InMemorySandboxStore,
    agent_id: &str,
    worktree_id: &str,
    workspace_id: &str,
    sandbox_id: &str,
    owner: &str,
    status: AgentStatus,
) -> Agent {
    if store
        .get_sandbox(&SandboxId(sandbox_id.into()))
        .await
        .unwrap()
        .is_none()
    {
        store
            .create_sandbox(sandbox(sandbox_id, owner))
            .await
            .unwrap();
    }
    if store
        .get_workspace(&WorkspaceId(workspace_id.into()))
        .await
        .unwrap()
        .is_none()
    {
        store
            .create_workspace(workspace(workspace_id, sandbox_id, owner))
            .await
            .unwrap();
    }
    insert_agent(
        store,
        agent(
            agent_id,
            worktree_id,
            workspace_id,
            sandbox_id,
            owner,
            status,
        ),
        worktree(worktree_id, workspace_id, sandbox_id, agent_id, owner),
    )
    .await
}

#[derive(Clone)]
struct StubResources {
    response: Arc<Mutex<Result<SandboxResourceReading, SandboxResourceReadError>>>,
    seen: Arc<Mutex<Vec<SandboxId>>>,
}

impl StubResources {
    fn reporting(reading: SandboxResourceReading) -> Self {
        Self {
            response: Arc::new(Mutex::new(Ok(reading))),
            seen: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl SandboxResourceReader for StubResources {
    async fn sandbox_resources(
        &self,
        sandbox: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        self.seen.lock().unwrap().push(sandbox.id.clone());
        self.response.lock().unwrap().clone()
    }
}

fn service(store: Arc<InMemorySandboxStore>) -> Arc<AgentReadService> {
    AgentReadService::new(
        store,
        Arc::new(StubResources::reporting(SandboxResourceReading::default())),
    )
}

#[tokio::test]
async fn projections_refuse_absent_frozen_grants_before_reading_runtime_state() {
    let store = Arc::new(InMemorySandboxStore::default());
    store
        .create_sandbox(sandbox("sbx_ungranted", "owner"))
        .await
        .unwrap();
    store
        .create_workspace(workspace("wks_ungranted", "sbx_ungranted", "owner"))
        .await
        .unwrap();
    let mut ungranted = agent(
        "agt_ungranted",
        "wtr_ungranted",
        "wks_ungranted",
        "sbx_ungranted",
        "owner",
        AgentStatus::Starting,
    );
    ungranted.capabilities = RuntimeCapabilities::default();
    insert_agent(
        &store,
        ungranted.clone(),
        worktree(
            "wtr_ungranted",
            "wks_ungranted",
            "sbx_ungranted",
            "agt_ungranted",
            "owner",
        ),
    )
    .await;
    let resources = Arc::new(StubResources::reporting(SandboxResourceReading::default()));
    let reads = AgentReadService::new(store, resources.clone());

    for error in [
        reads
            .transcript(&caller("owner", true), &ungranted.id)
            .await
            .unwrap_err(),
        reads
            .usage(&caller("owner", true), &ungranted.id)
            .await
            .unwrap_err(),
        reads
            .resources(&caller("owner", true), &ungranted.id)
            .await
            .unwrap_err(),
    ] {
        assert!(matches!(
            error,
            AgentReadError::CapabilityUnsupported { .. }
        ));
    }
    assert!(resources.seen.lock().unwrap().is_empty());
}

async fn replace_history(
    store: &InMemorySandboxStore,
    mut agent: Agent,
    kinds: Vec<AgentLogKind>,
    conversation_id: Option<&str>,
    generation: Option<u64>,
    active_turn: Option<u64>,
) -> Agent {
    agent.conversation.conversation_id = conversation_id.map(str::to_owned);
    agent.conversation.runtime_generation = generation;
    agent.conversation.active_turn = active_turn;
    agent.conversation.last_seq = kinds.len() as u64 + 1;
    agent.status = if active_turn.is_some() {
        AgentStatus::Working
    } else if conversation_id.is_some() {
        AgentStatus::Ready
    } else {
        agent.status
    };
    agent.revision += 1;
    store.save_agent(1, agent.clone()).await.unwrap();
    for kind in kinds {
        store
            .append_agent_log(&agent.id, NOW.into(), kind)
            .await
            .unwrap();
    }
    agent
}

#[tokio::test]
async fn filtered_agent_pages_are_owner_scoped_bounded_and_cursor_stable() {
    let store = Arc::new(InMemorySandboxStore::default());
    insert_hierarchy(
        &store,
        "agt_a",
        "wtr_a",
        "wks_one",
        "sbx_one",
        "owner",
        AgentStatus::Starting,
    )
    .await;
    insert_hierarchy(
        &store,
        "agt_b",
        "wtr_b",
        "wks_one",
        "sbx_one",
        "owner",
        AgentStatus::Failed,
    )
    .await;
    insert_hierarchy(
        &store,
        "agt_c",
        "wtr_c",
        "wks_two",
        "sbx_one",
        "owner",
        AgentStatus::Deleted,
    )
    .await;
    insert_hierarchy(
        &store,
        "agt_d",
        "wtr_d",
        "wks_three",
        "sbx_two",
        "owner",
        AgentStatus::Failed,
    )
    .await;
    insert_hierarchy(
        &store,
        "agt_foreign",
        "wtr_foreign",
        "wks_foreign",
        "sbx_foreign",
        "other",
        AgentStatus::Starting,
    )
    .await;
    let reads = service(store);
    let owner = caller("owner", true);

    let first = reads
        .list(
            &owner,
            &AgentListQuery::default(),
            &PageRequest::new(2, None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        first
            .items
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        ["agt_a", "agt_b"]
    );
    assert_eq!(first.next.as_deref(), Some("agt_b"));
    let second = reads
        .list(
            &owner,
            &AgentListQuery::default(),
            &PageRequest::new(2, first.next).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        second
            .items
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        ["agt_c", "agt_d"]
    );
    assert_eq!(second.next, None);

    let sandbox_rows = reads
        .list(
            &owner,
            &AgentListQuery {
                sandbox_id: Some(SandboxId("sbx_one".into())),
                ..AgentListQuery::default()
            },
            &PageRequest::default(),
        )
        .await
        .unwrap();
    assert_eq!(sandbox_rows.items.len(), 3);
    let workspace_rows = reads
        .list(
            &owner,
            &AgentListQuery {
                workspace_id: Some(WorkspaceId("wks_one".into())),
                ..AgentListQuery::default()
            },
            &PageRequest::default(),
        )
        .await
        .unwrap();
    assert_eq!(workspace_rows.items.len(), 2);
    let failed = reads
        .list(
            &owner,
            &AgentListQuery {
                status: Some(AgentStatus::Failed),
                ..AgentListQuery::default()
            },
            &PageRequest::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        failed
            .items
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        ["agt_b", "agt_d"]
    );
}

#[tokio::test]
async fn reads_require_owner_authority_and_hide_foreign_agents() {
    let store = Arc::new(InMemorySandboxStore::default());
    let agent = insert_hierarchy(
        &store,
        "agt_owned",
        "wtr_owned",
        "wks_owned",
        "sbx_owned",
        "owner",
        AgentStatus::Starting,
    )
    .await;
    let reads = service(store);
    let lineage = reads
        .lineage(&caller("owner", true), &agent.id)
        .await
        .unwrap();
    assert_eq!(lineage.agent.id, agent.id);
    assert_eq!(lineage.worktree.id, agent.worktree_id);
    assert_eq!(lineage.workspace.id, agent.workspace_id);
    assert_eq!(lineage.sandbox.id, agent.sandbox_id);
    assert_eq!(
        reads.read(&caller("owner", false), &agent.id).await,
        Err(AgentReadError::Forbidden)
    );
    assert_eq!(
        reads.read(&caller("other", true), &agent.id).await,
        Err(AgentReadError::NotFound)
    );
    assert_eq!(
        reads
            .read(&caller("owner", true), &AgentId("agt_missing".into()))
            .await,
        Err(AgentReadError::NotFound)
    );
}

#[tokio::test]
async fn every_parent_identity_mismatch_fails_closed() {
    #[derive(Clone, Copy)]
    enum Fault {
        MissingWorktree,
        WorktreeWorkspace,
        WorktreeSandbox,
        WorktreeOwner,
        WorktreeAgent,
        MissingWorkspace,
        WorkspaceSandbox,
        WorkspaceOwner,
        MissingSandbox,
        SandboxOwner,
    }

    for fault in [
        Fault::MissingWorktree,
        Fault::WorktreeWorkspace,
        Fault::WorktreeSandbox,
        Fault::WorktreeOwner,
        Fault::WorktreeAgent,
        Fault::MissingWorkspace,
        Fault::WorkspaceSandbox,
        Fault::WorkspaceOwner,
        Fault::MissingSandbox,
        Fault::SandboxOwner,
    ] {
        let store = Arc::new(InMemorySandboxStore::default());
        let mut sandbox = sandbox("sbx_lineage", "owner");
        let mut workspace = workspace("wks_lineage", "sbx_lineage", "owner");
        let mut worktree = worktree(
            "wtr_lineage",
            "wks_lineage",
            "sbx_lineage",
            "agt_lineage",
            "owner",
        );
        match fault {
            Fault::WorktreeWorkspace => worktree.workspace_id = WorkspaceId("wrong".into()),
            Fault::WorktreeSandbox => worktree.sandbox_id = SandboxId("wrong".into()),
            Fault::WorktreeOwner => worktree.owner = SubjectId::new("other"),
            Fault::WorktreeAgent => worktree.agent_id = Some(AgentId("wrong".into())),
            Fault::WorkspaceSandbox => workspace.sandbox_id = SandboxId("wrong".into()),
            Fault::WorkspaceOwner => workspace.owner = SubjectId::new("other"),
            Fault::SandboxOwner => sandbox.owner = SubjectId::new("other"),
            _ => {}
        }
        if !matches!(fault, Fault::MissingSandbox) {
            store.create_sandbox(sandbox).await.unwrap();
        }
        if !matches!(fault, Fault::MissingWorkspace) {
            store.create_workspace(workspace).await.unwrap();
        }
        if !matches!(fault, Fault::MissingWorktree) {
            store.create_worktree(worktree).await.unwrap();
        }
        let agent = agent(
            "agt_lineage",
            "wtr_lineage",
            "wks_lineage",
            "sbx_lineage",
            "owner",
            AgentStatus::Starting,
        );
        store.create_agent(agent.clone()).await.unwrap();
        store
            .append_agent_log(
                &agent.id,
                NOW.into(),
                AgentLogKind::Created {
                    workspace_id: agent.workspace_id.clone(),
                    worktree_id: agent.worktree_id.clone(),
                },
            )
            .await
            .unwrap();
        assert!(matches!(
            service(store.clone())
                .read(&caller("owner", true), &agent.id)
                .await,
            Err(AgentReadError::Corrupt(_))
        ));
        assert!(matches!(
            service(store)
                .list(
                    &caller("owner", true),
                    &AgentListQuery::default(),
                    &PageRequest::default(),
                )
                .await,
            Err(AgentReadError::Corrupt(_))
        ));
    }
}

#[tokio::test]
async fn transcript_reconstructs_completed_and_active_turns() {
    let store = Arc::new(InMemorySandboxStore::default());
    let agent = insert_hierarchy(
        &store,
        "agt_transcript",
        "wtr_transcript",
        "wks_transcript",
        "sbx_transcript",
        "owner",
        AgentStatus::Starting,
    )
    .await;
    let agent = replace_history(
        &store,
        agent,
        vec![
            AgentLogKind::ConversationOpened {
                conversation_id: "conversation-1".into(),
                runtime_generation: 1,
            },
            AgentLogKind::TurnStarted {
                turn_sequence: 3,
                origin_key: Some("manual".into()),
            },
            AgentLogKind::OwnerMessage {
                content: "First question".into(),
            },
            AgentLogKind::AgentMessage {
                content: "First answer".into(),
            },
            AgentLogKind::AgentMessage {
                content: "Second paragraph".into(),
            },
            AgentLogKind::TurnCompleted { turn_sequence: 3 },
            AgentLogKind::TurnStarted {
                turn_sequence: 8,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "Follow up".into(),
            },
            AgentLogKind::AgentMessage {
                content: "In progress".into(),
            },
        ],
        Some("conversation-1"),
        Some(1),
        Some(8),
    )
    .await;
    let transcript = service(store)
        .transcript(&caller("owner", true), &agent.id)
        .await
        .unwrap();
    assert_eq!(transcript.agent_id, agent.id);
    assert_eq!(transcript.through_seq, 10);
    assert_eq!(transcript.turns.len(), 2);
    assert_eq!(transcript.turns[0].owner_message, "First question");
    assert_eq!(
        transcript.turns[0].agent_messages,
        ["First answer", "Second paragraph"]
    );
    assert!(matches!(
        transcript.turns[0].outcome,
        AgentTurnOutcome::Completed { .. }
    ));
    assert_eq!(transcript.turns[1].turn_sequence, 8);
    assert!(matches!(
        transcript.turns[1].outcome,
        AgentTurnOutcome::Active
    ));
}

#[tokio::test]
async fn usage_takes_generation_peaks_preserves_context_and_saturates() {
    let store = Arc::new(InMemorySandboxStore::default());
    let agent = insert_hierarchy(
        &store,
        "agt_usage",
        "wtr_usage",
        "wks_usage",
        "sbx_usage",
        "owner",
        AgentStatus::Starting,
    )
    .await;
    let agent = replace_history(
        &store,
        agent,
        vec![
            AgentLogKind::ConversationOpened {
                conversation_id: "conversation-1".into(),
                runtime_generation: 1,
            },
            AgentLogKind::Usage {
                input_tokens: u64::MAX,
                output_tokens: u64::MAX - 5,
                context_used: 400,
                context_window: Some(1_000),
            },
            AgentLogKind::Usage {
                input_tokens: 0,
                output_tokens: 0,
                context_used: 0,
                context_window: None,
            },
            AgentLogKind::ConversationOpened {
                conversation_id: "conversation-2".into(),
                runtime_generation: 3,
            },
            AgentLogKind::Usage {
                input_tokens: 10,
                output_tokens: 20,
                context_used: 500,
                context_window: None,
            },
            AgentLogKind::Usage {
                input_tokens: 5,
                output_tokens: 10,
                context_used: 300,
                context_window: Some(2_000),
            },
        ],
        Some("conversation-2"),
        Some(3),
        None,
    )
    .await;
    let reading = service(store)
        .usage(&caller("owner", true), &agent.id)
        .await
        .unwrap();
    assert_eq!(reading.through_seq, 7);
    assert_eq!(
        reading.usage,
        AgentUsage {
            input_tokens: u64::MAX,
            output_tokens: u64::MAX,
            context_used: 300,
            context_window: Some(2_000),
        }
    );
}

#[tokio::test]
async fn log_validation_rejects_gaps_wrong_identity_and_uncommitted_tail() {
    #[derive(Clone, Copy)]
    enum Fault {
        Gap,
        WrongAgent,
        WrongSequence,
        Tail,
    }
    for fault in [
        Fault::Gap,
        Fault::WrongAgent,
        Fault::WrongSequence,
        Fault::Tail,
    ] {
        let store = Arc::new(InMemorySandboxStore::default());
        let mut agent = insert_hierarchy(
            &store,
            "agt_log",
            "wtr_log",
            "wks_log",
            "sbx_log",
            "owner",
            AgentStatus::Starting,
        )
        .await;
        let mut entries = vec![AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: 1,
            appended_at: NOW.into(),
            kind: AgentLogKind::Created {
                workspace_id: agent.workspace_id.clone(),
                worktree_id: agent.worktree_id.clone(),
            },
        }];
        if matches!(fault, Fault::Tail) {
            entries.push(AgentLogEntry {
                agent_id: agent.id.clone(),
                seq: 2,
                appended_at: NOW.into(),
                kind: AgentLogKind::ModeChanged {
                    mode: "code".into(),
                },
            });
        } else {
            agent.conversation.last_seq = 2;
            agent.revision = 2;
            store.save_agent(1, agent.clone()).await.unwrap();
            if !matches!(fault, Fault::Gap) {
                entries.push(AgentLogEntry {
                    agent_id: if matches!(fault, Fault::WrongAgent) {
                        AgentId("agt_other".into())
                    } else {
                        agent.id.clone()
                    },
                    seq: if matches!(fault, Fault::WrongSequence) {
                        3
                    } else {
                        2
                    },
                    appended_at: NOW.into(),
                    kind: AgentLogKind::ModeChanged {
                        mode: "code".into(),
                    },
                });
            }
        }
        store.replace_agent_log_for_test(&agent.id, entries);
        assert!(matches!(
            service(store)
                .log(&caller("owner", true), &agent.id, 0, 10)
                .await,
            Err(AgentReadError::Corrupt(_))
        ));
    }
}

#[tokio::test]
async fn semantic_log_corruption_is_refused_by_every_projection() {
    let cases = vec![
        vec![AgentLogKind::ConversationGap { frames: 1 }],
        vec![AgentLogKind::OwnerMessage {
            content: "outside".into(),
        }],
        vec![
            AgentLogKind::ConversationOpened {
                conversation_id: "conversation-1".into(),
                runtime_generation: 1,
            },
            AgentLogKind::TurnStarted {
                turn_sequence: 3,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "one".into(),
            },
            AgentLogKind::OwnerMessage {
                content: "two".into(),
            },
        ],
        vec![
            AgentLogKind::ConversationOpened {
                conversation_id: "conversation-1".into(),
                runtime_generation: 1,
            },
            AgentLogKind::TurnStarted {
                turn_sequence: 3,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "question".into(),
            },
            AgentLogKind::TurnCompleted { turn_sequence: 4 },
        ],
        vec![
            AgentLogKind::ConversationOpened {
                conversation_id: "conversation-1".into(),
                runtime_generation: 1,
            },
            AgentLogKind::TurnStarted {
                turn_sequence: 3,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "question".into(),
            },
            AgentLogKind::TurnStarted {
                turn_sequence: 5,
                origin_key: None,
            },
        ],
        vec![AgentLogKind::Usage {
            input_tokens: 1,
            output_tokens: 1,
            context_used: 1,
            context_window: None,
        }],
    ];
    for (index, kinds) in cases.into_iter().enumerate() {
        let store = Arc::new(InMemorySandboxStore::default());
        let id = format!("agt_semantic_{index}");
        let agent = insert_hierarchy(
            &store,
            &id,
            &format!("wtr_semantic_{index}"),
            &format!("wks_semantic_{index}"),
            &format!("sbx_semantic_{index}"),
            "owner",
            AgentStatus::Starting,
        )
        .await;
        let has_conversation = kinds
            .iter()
            .any(|kind| matches!(kind, AgentLogKind::ConversationOpened { .. }));
        let active = kinds.iter().find_map(|kind| match kind {
            AgentLogKind::TurnStarted { turn_sequence, .. } => Some(*turn_sequence),
            _ => None,
        });
        let agent = replace_history(
            &store,
            agent,
            kinds,
            has_conversation.then_some("conversation-1"),
            has_conversation.then_some(1),
            active,
        )
        .await;
        let reads = service(store);
        assert!(matches!(
            reads.transcript(&caller("owner", true), &agent.id).await,
            Err(AgentReadError::Corrupt(_))
        ));
        assert!(matches!(
            reads.usage(&caller("owner", true), &agent.id).await,
            Err(AgentReadError::Corrupt(_))
        ));
        assert!(matches!(
            reads.log(&caller("owner", true), &agent.id, 0, 10).await,
            Err(AgentReadError::Corrupt(_))
        ));
    }
}

#[tokio::test]
async fn conversation_markers_must_increase_and_match_the_aggregate() {
    let store = Arc::new(InMemorySandboxStore::default());
    let agent = insert_hierarchy(
        &store,
        "agt_generation",
        "wtr_generation",
        "wks_generation",
        "sbx_generation",
        "owner",
        AgentStatus::Starting,
    )
    .await;
    let agent = replace_history(
        &store,
        agent,
        vec![
            AgentLogKind::ConversationOpened {
                conversation_id: "conversation-1".into(),
                runtime_generation: 2,
            },
            AgentLogKind::ConversationOpened {
                conversation_id: "conversation-2".into(),
                runtime_generation: 2,
            },
        ],
        Some("conversation-2"),
        Some(2),
        None,
    )
    .await;
    assert!(matches!(
        service(store)
            .usage(&caller("owner", true), &agent.id)
            .await,
        Err(AgentReadError::Corrupt(_))
    ));

    let store = Arc::new(InMemorySandboxStore::default());
    let agent = insert_hierarchy(
        &store,
        "agt_identity",
        "wtr_identity",
        "wks_identity",
        "sbx_identity",
        "owner",
        AgentStatus::Starting,
    )
    .await;
    let agent = replace_history(
        &store,
        agent,
        vec![AgentLogKind::ConversationOpened {
            conversation_id: "logged".into(),
            runtime_generation: 1,
        }],
        Some("aggregate"),
        Some(1),
        None,
    )
    .await;
    assert!(matches!(
        service(store)
            .transcript(&caller("owner", true), &agent.id)
            .await,
        Err(AgentReadError::Corrupt(_))
    ));
}

#[tokio::test]
async fn complete_log_reads_cross_the_store_page_boundary() {
    let store = Arc::new(InMemorySandboxStore::default());
    let agent = insert_hierarchy(
        &store,
        "agt_long",
        "wtr_long",
        "wks_long",
        "sbx_long",
        "owner",
        AgentStatus::Starting,
    )
    .await;
    let kinds = (0..205)
        .map(|index| AgentLogKind::ModeChanged {
            mode: format!("mode-{index}"),
        })
        .collect();
    let agent = replace_history(&store, agent, kinds, None, None, None).await;
    let page = service(store)
        .log(&caller("owner", true), &agent.id, 199, 7)
        .await
        .unwrap();
    assert_eq!(page.len(), 7);
    assert_eq!(page.first().unwrap().seq, 200);
    assert_eq!(page.last().unwrap().seq, 206);
}

#[tokio::test]
async fn list_and_log_reject_unbounded_page_sizes() {
    let store = Arc::new(InMemorySandboxStore::default());
    let agent = insert_hierarchy(
        &store,
        "agt_bounds",
        "wtr_bounds",
        "wks_bounds",
        "sbx_bounds",
        "owner",
        AgentStatus::Starting,
    )
    .await;
    let reads = service(store);
    assert_eq!(
        reads
            .list(
                &caller("owner", true),
                &AgentListQuery::default(),
                &PageRequest {
                    limit: 0,
                    cursor: None,
                },
            )
            .await,
        Err(AgentReadError::InvalidPage)
    );
    assert_eq!(
        reads.log(&caller("owner", true), &agent.id, 0, 201).await,
        Err(AgentReadError::InvalidPage)
    );
}

#[tokio::test]
async fn resource_readings_are_explicitly_attributed_to_the_parent_sandbox() {
    let store = Arc::new(InMemorySandboxStore::default());
    let agent = insert_hierarchy(
        &store,
        "agt_resources",
        "wtr_resources",
        "wks_resources",
        "sbx_resources",
        "owner",
        AgentStatus::Starting,
    )
    .await;
    let expected = SandboxResourceReading {
        uptime_ms: 99,
        cpu_usage_usec: Some(7),
        memory_bytes_used: Some(8),
        ..SandboxResourceReading::default()
    };
    let resources = Arc::new(StubResources::reporting(expected.clone()));
    let reads = AgentReadService::new(store, resources.clone());
    let reading = reads
        .resources(&caller("owner", true), &agent.id)
        .await
        .unwrap();
    assert_eq!(reading.agent_id, agent.id);
    assert_eq!(reading.sandbox_id.as_str(), "sbx_resources");
    assert_eq!(reading.reading, expected);
    assert_eq!(
        resources.seen.lock().unwrap().as_slice(),
        &[SandboxId("sbx_resources".into())]
    );

    *resources.response.lock().unwrap() = Err(SandboxResourceReadError::Unsupported(
        "runtime capability absent".into(),
    ));
    assert!(matches!(
        reads.resources(&caller("owner", true), &agent.id).await,
        Err(AgentReadError::ResourcesUnsupported(_))
    ));
}

#[test]
fn read_errors_have_stable_codes() {
    assert_eq!(AgentReadError::Forbidden.code(), "forbidden");
    assert_eq!(AgentReadError::NotFound.code(), "agent_not_found");
    assert_eq!(AgentReadError::InvalidPage.code(), "invalid_page");
    assert_eq!(
        AgentReadError::CapabilityUnsupported {
            capability: RuntimeCapability::Usage,
        }
        .code(),
        "agent_capability_unsupported"
    );
    assert_eq!(
        AgentReadError::ResourcesUnsupported(String::new()).code(),
        "agent_resources_unsupported"
    );
    assert_eq!(
        AgentReadError::Corrupt(String::new()).code(),
        "stored_agent_corrupt"
    );
    assert_eq!(
        AgentReadError::Unavailable(String::new()).code(),
        "agent_read_unavailable"
    );
}
