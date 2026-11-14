use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::{insert_agent, ready_agent};
use layer2_core::profiles::{WorkspaceLocation, WorkspaceProfileContent, WorkspaceProfileSnapshot};
use layer2_core::resource_access::{
    CallerContext, MembershipError, OrganizationAuthority, OrganizationChoice,
    ResourceAccessService,
};
use layer2_core::sandbox::store::{AgentStore, WorkspaceStore, WorktreeStore};
use layer2_core::sandbox::{
    Agent, AgentReadService, Sandbox, SandboxResourceReadError, SandboxResourceReader,
    SandboxResourceReading, Workspace, WorkspaceOrigin, WorkspaceSource, WorkspaceStatus,
    WorkspaceWorkState, WorkspaceWorkStep, Worktree, WorktreeStatus,
};
use layer2_core::{Authorization, Capability, WorkspaceProfileId};

pub(super) struct Services {
    pub(super) stores: super::super::mongo_tests::Fixture,
    pub(super) left: DesktopService,
    pub(super) right: DesktopService,
    pub(super) receiver: Arc<receiver::Receiver>,
    pub(super) agents: [AgentId; 2],
    pub(super) caller: Caller,
}

struct Resources;

struct NoOrganizations;
#[async_trait::async_trait]
impl OrganizationAuthority for NoOrganizations {
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        Err(MembershipError::Unavailable)
    }

    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &layer2_core::SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        Err(MembershipError::Unavailable)
    }
}

#[async_trait::async_trait]
impl SandboxResourceReader for Resources {
    async fn sandbox_resources(
        &self,
        _: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        panic!("desktop must not read resource usage")
    }
}

impl Services {
    pub(super) async fn new(replicas: bool) -> Self {
        let stores = super::super::mongo_tests::Fixture::new().await;
        let sandbox = stores
            .left
            .get_sandbox(&stores.request.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        let agents = [
            ready_agent(&sandbox, "agt_desktop_a"),
            ready_agent(&sandbox, "agt_desktop_b"),
        ];
        let now = layer2_core::now_rfc3339();
        let workspace = Workspace {
            id: agents[0].workspace_id.clone(),
            sandbox_id: sandbox.id.clone(),
            owner: sandbox.owner.clone(),
            origin: WorkspaceOrigin::Manual,
            profile: WorkspaceProfileSnapshot {
                id: WorkspaceProfileId("wkp_desktop".into()),
                revision: 1,
                digest: "d".repeat(64),
                content: WorkspaceProfileContent {
                    location: WorkspaceLocation::Directory {
                        name: "desktop".into(),
                    },
                    name: "desktop".into(),
                    setup_commands: vec![],
                    agent_profiles: vec![],
                },
            },
            name: "desktop".into(),
            path: "/home/cma/desktop".into(),
            source: WorkspaceSource::Empty,
            resolved_commit: None,
            status: WorkspaceStatus::Ready,
            work: WorkspaceWorkState::new(WorkspaceWorkStep::None, now.clone()),
            next_worktree_number: 3,
            created_at: now.clone(),
            ready_at: Some(now.clone()),
            failed: None,
            removed_at: None,
            revision: 1,
        };
        stores.left.create_workspace(workspace).await.unwrap();
        for (index, agent) in agents.iter().enumerate() {
            stores
                .left
                .create_worktree(worktree(agent, index))
                .await
                .unwrap();
            insert_agent(&stores.left, agent, &now).await;
        }
        let receiver = Arc::new(receiver::Receiver::new(stores.db.clone()));
        let left = service(stores.left.clone(), receiver.clone(), replicas);
        let right = service(stores.right.clone(), receiver.clone(), replicas);
        Self {
            stores,
            left,
            right,
            receiver,
            agents: agents.map(|agent| agent.id),
            caller: Caller::new(
                sandbox.owner,
                Authorization::new([Capability::UseOwnSandboxes]),
            ),
        }
    }

    pub(super) fn authority(&self) -> Arc<dyn layer2_core::sandbox::DesktopAuthorization> {
        Arc::new(FixtureAuthority {
            subject: self.caller.subject.clone(),
        })
    }

    pub(super) async fn stop_agent(&self, index: usize) {
        let mut agent = self
            .stores
            .right
            .get_agent(&self.agents[index])
            .await
            .unwrap()
            .unwrap();
        let revision = agent.revision;
        agent.status = layer2_core::sandbox::AgentStatus::Stopped;
        agent.revision += 1;
        self.stores.right.save_agent(revision, agent).await.unwrap();
    }
}

pub(super) fn service(
    store: MongoSandboxStore,
    adapter: Arc<dyn layer3_adapter::SandboxAdapter>,
    replicas: bool,
) -> DesktopService {
    let access = Arc::new(store.clone());
    service_with_access(store, adapter, replicas, access)
}

pub(super) fn service_with_access(
    store: MongoSandboxStore,
    adapter: Arc<dyn layer3_adapter::SandboxAdapter>,
    replicas: bool,
    access: Arc<dyn layer2_core::resource_access::ResourceShareStore>,
) -> DesktopService {
    let store = Arc::new(store);
    // Match the production resource-authority path instead of measuring the
    // fallback reader's separate ancestry requests. Personal owners need no
    // organization authority, but every snapshot still validates current rows.
    let access = Arc::new(ResourceAccessService::new(
        access,
        Arc::new(NoOrganizations),
    ));
    let reads = AgentReadService::new(store.clone(), Arc::new(Resources))
        .with_sessions(store)
        .with_resource_access(access);
    DesktopService::new(reads, adapter).with_replicas_enabled(replicas)
}

fn worktree(agent: &Agent, index: usize) -> Worktree {
    Worktree {
        id: agent.worktree_id.clone(),
        workspace_id: agent.workspace_id.clone(),
        sandbox_id: agent.sandbox_id.clone(),
        owner: agent.owner.clone(),
        name: format!("wt-{index}"),
        path: format!("/home/cma/desktop/.worktrees/wt-{index}"),
        branch: format!("cma/wt-{index}"),
        base_commit: "a".repeat(40),
        status: WorktreeStatus::Ready,
        agent_id: Some(agent.id.clone()),
        revision: 1,
    }
}

// Production credential/cookie proof is tested separately in auth::desktop.
// These service tests isolate shared resource and ownership orchestration.
struct FixtureAuthority {
    subject: layer2_core::SubjectId,
}
#[async_trait::async_trait]
impl layer2_core::sandbox::DesktopAuthorization for FixtureAuthority {
    async fn current(&self, caller: &Caller) -> Result<(), DesktopError> {
        if caller.subject != self.subject {
            return Err(DesktopError::Unavailable);
        }
        Ok(())
    }
}
