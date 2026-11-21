//! Transfer only the preassigned child identities from the atomically claimed environment.

use crate::store::sandbox_mongo::MongoSandboxStore;
use layer2_core::sandbox::store::{AgentAdmission, AgentCreation};
use layer2_core::sandbox::{
    Sandbox, SandboxStoreError, WarmSandbox, WarmSandboxState, Workspace, WorkspaceOrigin,
};
use mongodb::{bson::doc, ClientSession};

async fn claimed(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    session: &mut ClientSession,
) -> Result<Option<WarmSandbox>, SandboxStoreError> {
    let Some(document) = store
        .warm_sandboxes
        .find_one(doc! { "_id": sandbox.id.as_str(), "state": "claimed" })
        .session(session)
        .await
        .map_err(super::unavailable)?
    else {
        return Ok(None);
    };
    let warm = super::read(&document)?;
    let same_runtime = warm
        .runtime
        .as_ref()
        .zip(sandbox.runtime.as_ref())
        .is_some_and(|(warm, live)| {
            warm.runtime_id == live.runtime_id
                && warm.provision_generation == live.provision_generation
                && warm.runtime_version == live.runtime_version
                && warm.boot_id == live.boot_id
        });
    if warm.state != WarmSandboxState::Claimed
        || warm.plan.profile != sandbox.profile
        || !same_runtime
    {
        return Ok(None);
    }
    Ok(Some(warm))
}

pub(in crate::store) async fn adopt_workspace(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    candidate: &Workspace,
    session: &mut ClientSession,
) -> Result<Workspace, SandboxStoreError> {
    let mut next = candidate.clone();
    let WorkspaceOrigin::SandboxProfile { index } = candidate.origin else {
        return Ok(next);
    };
    if let Some(warm) = claimed(store, sandbox, session).await? {
        let index = usize::from(index);
        if let Some((workspace, plan)) = warm
            .workspaces
            .get(index)
            .zip(warm.plan.workspaces.get(index))
        {
            if plan.profile == candidate.profile && workspace.is_ready() {
                next.id = workspace.id.clone();
                next.work.execution = Some(warm.plan.execution);
                next.next_worktree_number = workspace.agents.len() as u64 + 1;
            }
        }
    }
    Ok(next)
}

pub(in crate::store) async fn adopt_agent(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    workspace: &Workspace,
    admission: &AgentAdmission,
    session: &mut ClientSession,
) -> Result<AgentCreation, SandboxStoreError> {
    let Some(warm) = claimed(store, sandbox, session).await? else {
        return admission.materialize_for_parent(workspace, sandbox);
    };
    let Some((prepared, plan)) = warm
        .workspaces
        .iter()
        .zip(&warm.plan.workspaces)
        .find(|(row, _)| row.id == workspace.id)
    else {
        return admission.materialize_for_parent(workspace, sandbox);
    };
    let Some((index, (agent, profile))) = prepared
        .agents
        .iter()
        .zip(&plan.agents)
        .enumerate()
        .find(|(_, (_, profile))| **profile == admission.profile)
    else {
        return admission.materialize_for_parent(workspace, sandbox);
    };
    if *profile != admission.profile
        || !prepared.is_ready()
        || prepared.resolved_commit != workspace.resolved_commit
    {
        return admission.materialize_for_parent(workspace, sandbox);
    }
    if store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .session(&mut *session)
        .await
        .map_err(super::unavailable)?
        .is_some()
    {
        return admission.materialize_for_parent(workspace, sandbox);
    }
    let mut admitted = admission.clone();
    admitted.agent_id = agent.id.clone();
    admitted.worktree_id = agent.worktree_id.clone();
    let mut reserved_slot = workspace.clone();
    reserved_slot.next_worktree_number = index as u64 + 1;
    let mut creation = admitted.materialize(&reserved_slot)?;
    creation.agent.work.resolved_skills = agent.skills.clone();
    creation.agent.work.warm_conversation_id = agent.conversation_id.clone();
    creation.agent.work.next_fence = 2;
    creation
        .agent
        .work
        .validate_resolved_skills(&creation.agent.profile.content.skills)
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    creation.validate_replay()?;
    Ok(creation)
}
