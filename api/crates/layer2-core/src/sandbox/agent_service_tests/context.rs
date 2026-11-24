use std::sync::Arc;
use std::time::Duration;

use crate::profiles::{InMemoryProfileStore, SandboxProfileRef, WorkspaceProfileRef};
use crate::sandbox::store::{SandboxStore, WorkspaceStore};
use crate::{PlatformDefaults, SandboxId, SubjectId, WorkspaceId};

use super::*;

struct ContextHarness {
    service: Arc<AgentContextService>,
    agents: Arc<AgentService>,
    store: Arc<InMemorySandboxStore>,
    profiles: Arc<InMemoryProfileStore>,
    defaults: PlatformDefaults,
}

impl ContextHarness {
    async fn new(sandbox_status: SandboxStatus, workspace_status: WorkspaceStatus) -> Self {
        let (agents, store, profiles, agent_profile) = setup(
            sandbox_status,
            workspace_status,
            8,
            "owner",
            ProfileVisibility::Private,
        )
        .await;
        let workspace = workspace(workspace_status);
        let sandbox = store
            .get_sandbox(&workspace.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        let defaults = PlatformDefaults {
            sandbox_profile: SandboxProfileRef::new(sandbox.profile.id, sandbox.profile.revision),
            workspace_profile: WorkspaceProfileRef::new(
                workspace.profile.id,
                workspace.profile.revision,
            ),
            agent_profile,
        };
        let service = AgentContextService::new(
            SandboxService::new(
                store.clone(),
                profiles.clone(),
                SandboxPolicy::default(),
                Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
            ),
            WorkspaceService::new(store.clone(), profiles.clone()),
            agents.clone(),
            profiles.clone(),
            Arc::new(defaults.clone()),
        );
        Self {
            service,
            agents,
            store,
            profiles,
            defaults,
        }
    }

    async fn create(&self) -> Result<Agent, AgentContextError> {
        self.service
            .create(&caller("owner", true), Default::default())
            .await
    }
}

#[tokio::test]
async fn default_context_reuses_owned_pinned_resources_and_creates_only_agent() {
    let harness = ContextHarness::new(SandboxStatus::Active, WorkspaceStatus::Ready).await;
    let first = harness.create().await.unwrap();
    let second = harness.create().await.unwrap();
    assert_ne!(first.id, second.id);
    assert_eq!(first.sandbox_id, second.sandbox_id);
    assert_eq!(first.workspace_id, workspace(WorkspaceStatus::Ready).id);
    assert_eq!(first.profile.id, harness.defaults.agent_profile.id);
    assert_eq!(
        first.profile.revision,
        harness.defaults.agent_profile.revision
    );
    assert!(first.conversation.first_message.is_none());
    assert!(first.title.is_none());
    assert_eq!(first.status, AgentStatus::Starting);
    assert_eq!(
        harness
            .store
            .list_sandboxes(&SubjectId::new("owner"), &Default::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
}

#[tokio::test]
async fn default_context_selects_most_recently_active_matching_sandbox() {
    let harness = ContextHarness::new(SandboxStatus::Active, WorkspaceStatus::Ready).await;
    let mut recent = harness
        .store
        .get_sandbox(&workspace(WorkspaceStatus::Ready).sandbox_id)
        .await
        .unwrap()
        .unwrap();
    recent.id = SandboxId("sbx_recent_context".into());
    recent.name = "recent-context".into();
    recent.last_activity_at = "2026-09-06T01:00:00Z".into();
    harness.store.create_sandbox(recent.clone()).await.unwrap();
    let mut recent_workspace = workspace(WorkspaceStatus::Ready);
    recent_workspace.id = WorkspaceId("wks_recent_context".into());
    recent_workspace.sandbox_id = recent.id.clone();
    harness
        .store
        .create_workspace(recent_workspace.clone())
        .await
        .unwrap();
    let mut wrong_revision = recent.clone();
    wrong_revision.id = SandboxId("sbx_unmatched_context".into());
    wrong_revision.name = "unmatched-context".into();
    wrong_revision.profile.revision += 1;
    wrong_revision.last_activity_at = "2026-09-06T02:00:00Z".into();
    harness.store.create_sandbox(wrong_revision).await.unwrap();
    let agent = harness.create().await.unwrap();
    assert_eq!(agent.sandbox_id, recent.id);
    assert_eq!(agent.workspace_id, recent_workspace.id);
}

#[tokio::test]
async fn overrides_are_owner_scoped_and_workspace_cannot_cross_sandboxes() {
    let harness = ContextHarness::new(SandboxStatus::Active, WorkspaceStatus::Ready).await;
    let owner = caller("owner", true);
    let existing = workspace(WorkspaceStatus::Ready);
    assert_eq!(
        harness
            .service
            .create(
                &owner,
                AgentContextOverrides {
                    workspace_id: Some(existing.id.clone()),
                    sandbox_id: Some(SandboxId("sbx_mismatch".into())),
                    ..Default::default()
                }
            )
            .await,
        Err(AgentContextError::InvalidOverride)
    );
    assert_eq!(
        harness
            .service
            .create(
                &caller("stranger", true),
                AgentContextOverrides {
                    workspace_id: Some(existing.id.clone()),
                    ..Default::default()
                }
            )
            .await,
        Err(AgentContextError::Workspace(
            WorkspaceServiceError::NotFound
        ))
    );
    let mut foreign = profile(
        "agp_foreign_context",
        "publisher",
        ProfileVisibility::Public,
    );
    foreign.draft.system_prompt = "Public but not the caller's override.".into();
    harness
        .profiles
        .create_agent_profile(foreign.clone())
        .await
        .unwrap();
    harness
        .profiles
        .publish_agent_profile(&foreign.id, 1, NOW.into())
        .await
        .unwrap();
    assert_eq!(
        harness
            .service
            .create(
                &owner,
                AgentContextOverrides {
                    agent_profile_id: Some(foreign.id),
                    ..Default::default()
                }
            )
            .await,
        Err(AgentContextError::InvalidOverride)
    );
    let explicit = harness
        .service
        .create(
            &owner,
            AgentContextOverrides {
                workspace_id: Some(existing.id.clone()),
                agent_profile_id: Some(harness.defaults.agent_profile.id.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(explicit.workspace_id, existing.id);
}

#[tokio::test]
async fn visitor_without_context_is_refused_without_creating_resources() {
    let harness = ContextHarness::new(SandboxStatus::Active, WorkspaceStatus::Ready).await;
    assert_eq!(
        harness
            .service
            .create(
                &launch_caller("visitor", Capability::LaunchTriggers),
                Default::default()
            )
            .await,
        Err(AgentContextError::SandboxRequired)
    );
    assert_eq!(
        harness
            .service
            .create(&caller("stranger", true), Default::default())
            .await,
        Err(AgentContextError::SandboxRequired)
    );
    assert_eq!(
        harness
            .service
            .create(&caller("owner", false), Default::default())
            .await,
        Err(AgentContextError::Forbidden)
    );
    assert!(harness
        .store
        .list_agents(&SubjectId::new("owner"), &Default::default())
        .await
        .unwrap()
        .items
        .is_empty());
}

#[tokio::test(start_paused = true)]
async fn context_readiness_is_bounded_and_workspace_failure_is_terminal() {
    let harness = ContextHarness::new(SandboxStatus::Active, WorkspaceStatus::Preparing).await;
    let before = tokio::time::Instant::now();
    assert_eq!(
        harness.create().await,
        Err(AgentContextError::AdmissionTimeout)
    );
    assert_eq!(before.elapsed(), AGENT_CONTEXT_ADMISSION_BOUND);
    let harness = ContextHarness::new(SandboxStatus::Active, WorkspaceStatus::Failed).await;
    assert_eq!(
        harness
            .service
            .create(
                &caller("owner", true),
                AgentContextOverrides {
                    workspace_id: Some(workspace(WorkspaceStatus::Failed).id),
                    ..Default::default()
                }
            )
            .await,
        Err(AgentContextError::NotReady)
    );
}

#[tokio::test(start_paused = true)]
async fn context_waits_for_workspace_and_agent_readiness() {
    let harness = ContextHarness::new(SandboxStatus::Active, WorkspaceStatus::Preparing).await;
    let store = harness.store.clone();
    let ready = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let mut workspace = workspace(WorkspaceStatus::Preparing);
        workspace.status = WorkspaceStatus::Ready;
        workspace.resolved_commit = Some("a".repeat(40));
        workspace.revision += 1;
        store.save_workspace(1, workspace).await.unwrap();
    });
    let mut agent = harness.create().await.unwrap();
    ready.await.unwrap();
    let store = harness.store.clone();
    let agent_id = agent.id.clone();
    let ready = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let revision = agent.revision;
        agent.status = AgentStatus::Ready;
        agent.revision += 1;
        store.save_agent(revision, agent).await.unwrap();
    });
    assert_eq!(
        harness
            .service
            .await_input_ready(&caller("owner", true), &agent_id)
            .await
            .unwrap()
            .status,
        AgentStatus::Ready
    );
    ready.await.unwrap();
    assert_eq!(
        harness
            .agents
            .read(&caller("owner", true), &agent_id)
            .await
            .unwrap()
            .status,
        AgentStatus::Ready
    );
}

#[tokio::test(start_paused = true)]
async fn first_input_durably_requests_revive_and_obeys_the_admission_bound() {
    let (harness, agent) = paused_context().await;
    let before = tokio::time::Instant::now();
    assert_eq!(
        harness
            .service
            .await_input_ready(&caller("owner", true), &agent.id)
            .await,
        Err(AgentContextError::AdmissionTimeout)
    );
    assert_eq!(before.elapsed(), AGENT_CONTEXT_ADMISSION_BOUND);
    let sandbox = harness
        .store
        .get_sandbox(&agent.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sandbox.status, SandboxStatus::Reviving);
    assert!(matches!(
        sandbox.lifecycle.operation,
        Some(SandboxLifecycleOperation::Revive { .. })
    ));
    assert!(harness
        .store
        .get_agent(&agent.id)
        .await
        .unwrap()
        .unwrap()
        .conversation
        .active_turn
        .is_none());
}

#[tokio::test(start_paused = true)]
async fn paused_input_automatically_revives_and_returns_the_same_ready_context() {
    let (harness, agent) = paused_context().await;
    let store = harness.store.clone();
    let original = agent.clone();
    let worker = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let mut parent = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
        assert_eq!(parent.status, SandboxStatus::Reviving);
        let revision = parent.revision;
        parent.status = SandboxStatus::Active;
        parent.lifecycle.operation = None;
        parent.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
        parent.revision += 1;
        store.save_sandbox(revision, parent).await.unwrap();
        let mut ready = agent;
        let revision = ready.revision;
        ready.status = AgentStatus::Ready;
        ready.revision += 1;
        store.save_agent(revision, ready).await.unwrap();
    });
    let ready = harness
        .service
        .await_input_ready(&caller("owner", true), &original.id)
        .await
        .unwrap();
    worker.await.unwrap();
    assert_eq!(ready.status, AgentStatus::Ready);
    assert_eq!(ready.id, original.id);
    assert_eq!(ready.sandbox_id, original.sandbox_id);
    assert_eq!(ready.workspace_id, original.workspace_id);
    assert_eq!(ready.worktree_id, original.worktree_id);
    assert_eq!(ready.conversation, original.conversation);
}

async fn paused_context() -> (ContextHarness, Agent) {
    use crate::sandbox::store::{SandboxTransaction, SandboxTransactionStore};

    let harness = ContextHarness::new(SandboxStatus::Active, WorkspaceStatus::Ready).await;
    let mut agent = harness.create().await.unwrap();
    let agent_revision = agent.revision;
    agent.status = AgentStatus::Paused;
    agent.revision += 1;
    harness
        .store
        .save_agent(agent_revision, agent.clone())
        .await
        .unwrap();
    let mut sandbox = harness
        .store
        .get_sandbox(&agent.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let revision = sandbox.revision;
    sandbox.status = SandboxStatus::Paused;
    sandbox.lifecycle.checkpoints.latest =
        Some(crate::sandbox::lifecycle_test_support::checkpoint(1, NOW));
    sandbox.revision += 1;
    harness
        .store
        .commit(SandboxTransaction::SaveSandboxAndAppend {
            command_revival: None,
            released_runtime: None,
            expected_revision: revision,
            sandbox: Box::new(sandbox),
            event_at: NOW.into(),
            event: SandboxEventKind::Created,
        })
        .await
        .unwrap();
    (harness, agent)
}
