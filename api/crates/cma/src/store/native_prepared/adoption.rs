//! Only the exact claimed lifecycle may adopt its reserved identities/session.
use super::{document, model::Record, read, unavailable};
use crate::store::agent_admission::helper_error::HelperError;
use crate::store::sandbox_mongo::MongoSandboxStore;
use layer2_core::sandbox::store::{AgentAdmission, AgentCreation};
use layer2_core::sandbox::{
    AgentOrigin, Sandbox, SandboxOrigin, SandboxStatus, SandboxStoreError, Workspace,
    WorkspaceOrigin, WorkspaceStatus,
};
use mongodb::{bson::doc, ClientSession};

pub(in crate::store) async fn completed_installation_digest(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
) -> Result<Option<String>, SandboxStoreError> {
    use layer2_core::sandbox::SandboxWorkStep;
    if sandbox.status != SandboxStatus::Provisioning
        || sandbox.work.step != SandboxWorkStep::ProvisionRuntime
        || sandbox.lifecycle.operation.is_some()
        || !matches!(sandbox.origin, SandboxOrigin::Lifecycle { .. })
        || !sandbox.credential.source.is_managed()
        || sandbox.credential.key_generation != 0
        || sandbox.credential.delivered
        || sandbox.credential.current.is_some()
        || sandbox.credential.pending.is_some()
        || !sandbox.credential.revoke_debt.is_empty()
        || !sandbox.credential.service_grants.is_empty()
        || sandbox.credential.service_scope.is_some()
        || sandbox
            .lifecycle
            .home
            .as_ref()
            .is_none_or(|home| home.native_pool.is_none())
    {
        return Ok(None);
    }
    let mut session = store.client.start_session().await.map_err(unavailable)?;
    let Some(record) = claimed(store, sandbox, &mut session).await? else {
        return Ok(None);
    };
    let mut reaches = sandbox.credential.reaches.clone();
    reaches.sort();
    reaches.dedup();
    if sandbox.work.execution != Some(record.payload.plan.execution)
        || reaches != record.payload.plan.reaches
    {
        return Ok(None);
    }
    Ok(record.payload.installed_request_sha256)
}

pub(super) async fn claimed(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    session: &mut ClientSession,
) -> Result<Option<Record>, SandboxStoreError> {
    claimed_transaction(store, sandbox, session)
        .await
        .map_err(|error| error.into_store_error(unavailable))
}

pub(super) async fn claimed_transaction(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    session: &mut ClientSession,
) -> Result<Option<Record>, HelperError> {
    let Some(value) = store
        .native_prepared
        .find_one(doc! {"_id":sandbox.id.as_str(),"state":"claimed"})
        .session(session)
        .await
        .map_err(HelperError::Driver)?
    else {
        return Ok(None);
    };
    let record = read(&value)?;
    let owned = record
        .ownership
        .as_ref()
        .ok_or_else(|| super::model::corrupt("claimed ownership missing"))?;
    if sandbox.origin
        != (SandboxOrigin::Lifecycle {
            operation_id: owned.operation_id.clone(),
        })
        || sandbox.owner != owned.owner
        || sandbox.profile != owned.sandbox
        || !same_runtime(&record, sandbox)
        || sandbox.lifecycle.home.as_ref() != record.payload.home.as_ref()
    {
        return Ok(None);
    }
    Ok(Some(record))
}

pub(super) fn same_runtime(record: &Record, sandbox: &Sandbox) -> bool {
    record
        .payload
        .runtime
        .as_ref()
        .zip(sandbox.runtime.as_ref())
        .is_some_and(|(a, b)| {
            a.runtime_id == b.runtime_id
                && a.provision_generation == b.provision_generation
                && a.runtime_version == b.runtime_version
                && a.boot_id == b.boot_id
        })
}

pub(in crate::store) async fn completed_workspace(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    workspace: &Workspace,
) -> Result<Option<layer2_core::sandbox::store::WorkspacePreparationCompletion>, SandboxStoreError>
{
    // Coherent current ownership/lease snapshot before the worker's fresh runtime observation.
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(unavailable)?;
    let Some(parent) = store
        .sandboxes
        .find_one(doc! {"_id":sandbox.id.as_str()})
        .session(&mut session)
        .await
        .map_err(unavailable)?
    else {
        return Ok(None);
    };
    if !crate::store::work_lease_update::sandbox_matches(
        Some(&super::super::sandboxes::read_sandbox(&parent)?),
        sandbox,
    ) {
        return Ok(None);
    }
    let Some(child) = store
        .workspaces
        .find_one(doc! {"_id":workspace.id.as_str()})
        .session(&mut session)
        .await
        .map_err(unavailable)?
    else {
        return Ok(None);
    };
    if !crate::store::work_lease_update::workspace_matches(
        Some(&super::super::workspaces::read_workspace(&child)?),
        workspace,
    ) {
        return Ok(None);
    }
    let Some(record) = claimed(store, sandbox, &mut session).await? else {
        return Ok(None);
    };
    let Some(value) = store
        .native_prepared
        .find_one(doc! {"_id":sandbox.id.as_str(),"state":"claimed"})
        .session(&mut session)
        .await
        .map_err(unavailable)?
    else {
        return Ok(None);
    };
    let Some(proof) = super::workspace_proof::read(&value, &record.payload)? else {
        return Ok(None);
    };
    workspace_completion(&record, &proof, sandbox, workspace)
}

pub(super) fn workspace_completion(
    record: &Record,
    proof: &super::NativePreparedWorkspaceProof,
    sandbox: &Sandbox,
    workspace: &Workspace,
) -> Result<Option<layer2_core::sandbox::store::WorkspacePreparationCompletion>, SandboxStoreError>
{
    proof.validate(&record.payload)?;
    let owned = record
        .ownership
        .as_ref()
        .ok_or_else(|| super::model::corrupt("workspace proof ownership missing"))?;
    let Some(setup) = proof.setup.as_ref() else {
        return Ok(None);
    };
    if sandbox.origin
        != (SandboxOrigin::Lifecycle {
            operation_id: owned.operation_id.clone(),
        })
        || sandbox.owner != owned.owner
        || sandbox.profile != owned.sandbox
        || !same_runtime(record, sandbox)
        || sandbox.lifecycle.home.as_ref() != record.payload.home.as_ref()
        || workspace.origin != (WorkspaceOrigin::SandboxProfile { index: 0 })
        || workspace.owner != owned.owner
        || workspace.sandbox_id != sandbox.id
        || workspace.profile != owned.workspace
        || workspace.id != record.payload.workspaces[0].id
        || workspace.name != proof.prepare.name
        || workspace.path != proof.prepare.path
        || workspace.source != layer2_core::sandbox::WorkspaceSource::Empty
        || workspace.work.execution != Some(record.payload.plan.execution)
        || workspace.work.step != layer2_core::sandbox::WorkspaceWorkStep::Prepare
        || workspace.status != WorkspaceStatus::Preparing
        || workspace.work.lease.is_none()
        || workspace
            .resolved_commit
            .as_ref()
            .is_some_and(|commit| commit != &proof.prepare.resolved_commit)
        || !sandbox.credential.delivered
        || !matches!(
            sandbox.status,
            SandboxStatus::Installing | SandboxStatus::Active
        )
        || sandbox.lifecycle.operation.is_some()
        || sandbox.lifecycle.checkpoint_capture.is_some()
    {
        return Ok(None);
    }
    Ok(Some(
        layer2_core::sandbox::store::WorkspacePreparationCompletion {
            prepare: proof.prepare.clone(),
            setup: setup.clone(),
        },
    ))
}

pub(in crate::store) async fn adopt_workspace(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    candidate: &Workspace,
    session: &mut ClientSession,
) -> Result<Option<Workspace>, SandboxStoreError> {
    let Some(record) = claimed(store, sandbox, session).await? else {
        return Ok(None);
    };
    let owned = record.ownership.as_ref().expect("validated ownership");
    if candidate.origin != (WorkspaceOrigin::SandboxProfile { index: 0 })
        || candidate.profile != owned.workspace
        || candidate.owner != owned.owner
        || candidate.sandbox_id != sandbox.id
    {
        return Ok(None);
    }
    let prepared = &record.payload.workspaces[0];
    if !prepared.is_ready() {
        return Err(super::model::corrupt("claimed workspace is incomplete"));
    }
    let mut next = candidate.clone();
    next.id = prepared.id.clone();
    next.work.execution = Some(record.payload.plan.execution);
    next.next_worktree_number = 2;
    Ok(Some(next))
}

pub(in crate::store) async fn adopt_agent(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    workspace: &Workspace,
    admission: &AgentAdmission,
    session: &mut ClientSession,
) -> Result<Option<AgentCreation>, SandboxStoreError> {
    let Some(mut record) = claimed(store, sandbox, session).await? else {
        return Ok(None);
    };
    let owned = record.ownership.as_mut().expect("validated ownership");
    let prepared = &record.payload.workspaces[0];
    let agent = &prepared.agents[0];
    if admission.origin
        != (AgentOrigin::Lifecycle {
            operation_id: owned.operation_id.clone(),
        })
        || admission.owner != owned.owner
        || admission.binding.is_some()
        || admission.profile.content != owned.agent_content
        || workspace.profile != owned.workspace
        || workspace.id != prepared.id
    {
        return Ok(None);
    }
    // A prepared conversation must never enter effect-free WaitForWorkspace.
    if sandbox.status != SandboxStatus::Active {
        return Err(SandboxStoreError::SandboxNotReady);
    }
    if workspace.status != WorkspaceStatus::Ready
        || prepared.resolved_commit != workspace.resolved_commit
    {
        return Err(SandboxStoreError::WorkspaceNotReady);
    }
    if owned.agent.is_some()
        || store
            .agents
            .find_one(doc! {"_id":agent.id.as_str()})
            .session(&mut *session)
            .await
            .map_err(unavailable)?
            .is_some()
    {
        return Err(SandboxStoreError::Conflict);
    }
    let mut admitted = admission.clone();
    admitted.agent_id = agent.id.clone();
    admitted.worktree_id = agent.worktree_id.clone();
    let mut slot = workspace.clone();
    slot.next_worktree_number = 1;
    let mut creation = admitted.materialize(&slot)?;
    creation.agent.work.resolved_skills = agent.skills.clone();
    creation.agent.work.warm_conversation_id = agent.conversation_id.clone();
    creation.agent.work.next_fence = 2;
    creation
        .agent
        .work
        .validate_resolved_skills(&creation.agent.profile.content.skills)
        .map_err(|e| super::model::corrupt(&e.to_string()))?;
    creation.validate_replay()?;
    owned.agent = Some(admission.profile.clone());
    let previous = record.payload.revision;
    record.payload.revision = previous.checked_add(1).ok_or(SandboxStoreError::Conflict)?;
    record.payload.updated_at = admission.event_at.clone();
    let mut fields = document(&record)?;
    fields.remove("_id");
    // Preserve optional validated envelope evidence through Agent ownership publication.
    let result=store.native_prepared.update_one(doc!{"_id":record.payload.id.as_str(),"state":"claimed","revision":super::signed(previous,"native prepared revision").map_err(SandboxStoreError::Corrupt)?},doc!{"$set":fields})
        .session(session).await.map_err(unavailable)?;
    if result.matched_count != 1 {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(Some(creation))
}
