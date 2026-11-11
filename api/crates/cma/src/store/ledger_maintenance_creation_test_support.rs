use super::*;
use layer2_core::sandbox::store::{
    AgentAdmission, AgentAdmissionResult, AgentAdmissionStore, WorkspaceAdmission,
    WorkspaceAdmissionKind, WorkspaceAdmissionResult, WorkspaceAdmissionStore,
};
use layer2_core::sandbox::{AgentOrigin, WorkspaceStatus, WorkspaceWorkState, WorkspaceWorkStep};

#[derive(Clone)]
pub(super) enum Request {
    Agent(AgentAdmission),
    Workspace(WorkspaceAdmission),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    Agent(AgentAdmissionResult),
    Workspace(WorkspaceAdmissionResult),
}

pub(super) fn request(
    source: &layer2_core::sandbox::ledger_maintenance_plan::LedgerMaintenanceSnapshotV1,
    agent: bool,
) -> Request {
    if agent {
        let original = &source.agents[0];
        Request::Agent(AgentAdmission {
            binding: None,
            created_by: None,
            owner: original.owner.clone(),
            workspace_id: original.workspace_id.clone(),
            agent_id: layer2_core::AgentId::generate(),
            worktree_id: layer2_core::WorktreeId::generate(),
            profile: original.profile.clone(),
            capabilities: original.capabilities.clone(),
            title: None,
            first_message: None,
            origin: AgentOrigin::Manual,
            creation_key: "maintenance-new-agent".into(),
            event_at: NOW.into(),
        })
    } else {
        let mut workspace = source.workspaces[0].clone();
        workspace.id = layer2_core::WorkspaceId::generate();
        workspace.name = "maintenance-new-workspace".into();
        workspace.path = format!("/home/cma/{}", workspace.name);
        workspace.profile.content.name.clone_from(&workspace.name);
        workspace.profile.content.location = layer2_core::profiles::WorkspaceLocation::Directory {
            name: workspace.name.clone(),
        };
        workspace.status = WorkspaceStatus::Preparing;
        workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, NOW.into());
        workspace.resolved_commit = None;
        workspace.ready_at = None;
        workspace.next_worktree_number = 1;
        workspace.revision = 1;
        Request::Workspace(WorkspaceAdmission {
            binding: None,
            workspace,
            kind: WorkspaceAdmissionKind::Manual,
            event_at: NOW.into(),
        })
    }
}

pub(super) async fn create(
    store: &MongoSandboxStore,
    request: &Request,
) -> Result<Outcome, SandboxStoreError> {
    match request {
        Request::Agent(request) => store.admit_agent(request.clone()).await.map(Outcome::Agent),
        Request::Workspace(request) => store
            .admit_workspace(request.clone())
            .await
            .map(Outcome::Workspace),
    }
}

pub(super) fn assert_created(result: &Outcome) {
    assert!(matches!(
        result,
        Outcome::Agent(AgentAdmissionResult::Created(_))
            | Outcome::Workspace(WorkspaceAdmissionResult::Created(_))
    ));
}
