//! Retire only the original allocation of a never-started repository task.

use layer2_core::sandbox::{
    store::RepositoryPreparationRuntime, Agent, AgentOrigin, AgentStatus, AgentWorkStep,
    FirstMessageDelivery, FirstMessagePurpose, Sandbox, SandboxOrigin, SandboxStatus, Workspace,
    WorkspaceOrigin, WorkspaceSource, WorkspaceWorkStep, WorktreeStatus,
};
use mongodb::{
    bson::{Bson, DateTime, Document},
    ClientSession, Collection,
};

use super::*;
use crate::store::agent_lifecycle_operations::{LifecycleBehavior, LifecycleRecord, COLLECTION};

pub(super) async fn settle(
    store: &MongoSandboxStore,
    session: &mut ClientSession,
    before: &Workspace,
    failed: &Workspace,
    runtime: &RepositoryPreparationRuntime,
    at: &str,
) -> Result<(), ApplyError> {
    let Some(failure) = failed
        .failed
        .as_ref()
        .filter(|reason| reason.class.starts_with("repository_"))
    else {
        return Ok(());
    };
    if before.status != WorkspaceStatus::Preparing
        || before.work.step != WorkspaceWorkStep::Prepare
        || before.ready_at.is_some()
        || before.origin != (WorkspaceOrigin::SandboxProfile { index: 0 })
        || !matches!(
            before.source,
            WorkspaceSource::RepositorySnapshot { .. }
                | WorkspaceSource::PublicRepositorySnapshot { .. }
        )
        || failed.status != WorkspaceStatus::Failed
    {
        return Ok(());
    }

    let Some(parent) = find(
        &store.sandboxes,
        doc! {"_id": before.sandbox_id.as_str()},
        session,
    )
    .await?
    else {
        return Ok(());
    };
    let sandbox = read_sandbox(&parent).map_err(ApplyError::Domain)?;
    let SandboxOrigin::Lifecycle { operation_id } = &sandbox.origin else {
        return Ok(());
    };
    if !same_allocation(&sandbox, before, runtime) {
        tracing::debug!("Repository failure cleanup skipped: allocation authority changed");
        return Ok(());
    }

    let database = store.client.database(&store.sandboxes.namespace().db);
    let operations = database.collection::<LifecycleRecord>(COLLECTION);
    let Some(operation) = operations
        .find_one(doc! {
            "_id": operation_id,
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "owner": sandbox.owner.as_str(),
            "$or": [{"sandbox_id": sandbox.id.as_str()}, {"sandbox_id": Bson::Null}],
            "status": {"$in": ["queued", "running", "waiting_for_connection"]},
        })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
    else {
        return Ok(());
    };
    if !original_repository_task(&operation, before)
        || !original_allocation(store, &sandbox, &operation, session).await?
        || has_other_authority(store, before, session).await?
    {
        tracing::debug!(
            "Repository failure cleanup skipped: original operation lacks exclusive custody"
        );
        return Ok(());
    }
    let Some(agent_id) = fence_starting_hierarchy(store, before, &operation, session).await? else {
        tracing::debug!(
            "Repository failure cleanup skipped: Agent or worktree may have other work"
        );
        return Ok(());
    };

    let mut retiring = sandbox.clone();
    retiring
        .begin_retirement(at.to_owned())
        .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?;
    let change = SandboxEventKind::StatusChanged {
        from: sandbox.status,
        to: SandboxStatus::Retiring,
    };
    sandbox_save::apply(
        store,
        session,
        (None, None),
        sandbox.revision,
        &retiring,
        at,
        &change,
    )
    .await?;

    // The operation and the teardown obligation commit together. A restart
    // cannot lose the original error or resume provisioning this operation.
    let changed = operations.update_one(doc! {
        "_id": operation_id,
        "owner": sandbox.owner.as_str(),
        "sandbox_id": operation.sandbox_id.as_deref(),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "status": &operation.status,
    }, doc! { "$set": {
        "status": "failed", "phase": "failed", "error": format!("{}: {}", failure.class, failure.detail),
        "sandbox_id": sandbox.id.as_str(), "agent_id": agent_id,
        "connection": Bson::Null, "lease_token": Bson::Null, "lease_expires_at": Bson::Null,
        "updated_at": DateTime::now(),
    }}).session(session).await.map_err(ApplyError::Driver)?;
    if changed.matched_count != 1 {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    Ok(())
}

async fn original_allocation(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    operation: &LifecycleRecord,
    session: &mut ClientSession,
) -> Result<bool, ApplyError> {
    if let Some(id) = operation.sandbox_id.as_deref() {
        return Ok(id == sandbox.id.as_str());
    }
    // Sandbox admission already recorded this owned lifecycle origin with its
    // replay metadata. The operation marker can lag the actual allocation.
    let Some(mut origin) =
        super::super::sandbox_admission::origin_filter(sandbox.owner.as_str(), &sandbox.origin)
    else {
        return Ok(false);
    };
    origin.insert("_id", doc! {"$ne": sandbox.id.as_str()});
    Ok(find(&store.sandboxes, origin, session).await?.is_none())
}

fn same_allocation(
    sandbox: &Sandbox,
    workspace: &Workspace,
    runtime: &RepositoryPreparationRuntime,
) -> bool {
    sandbox.owner == workspace.owner
        && sandbox.id == workspace.sandbox_id
        && matches!(
            sandbox.status,
            SandboxStatus::Installing | SandboxStatus::Active
        )
        && sandbox.lifecycle.operation.is_none()
        && sandbox.lifecycle.runtime_provision.is_none()
        && sandbox.runtime.as_ref() == Some(&runtime.binding)
        && runtime.generation > 0
        && runtime.binding.provision_generation == Some(runtime.generation)
        && sandbox.lifecycle.runtime_generation_high_water == runtime.generation
        && sandbox.credential.key_generation == runtime.key_generation
        && sandbox.profile.content.workspace_profiles.len() == 1
        && workspace.origin_matches_sandbox_profile(sandbox)
}

fn original_repository_task(operation: &LifecycleRecord, workspace: &Workspace) -> bool {
    let request = &operation.input.request;
    if !operation.input.behavior.uses_repository_profile()
        || request.sandbox_id.is_some()
        || request.workspace_id.is_some()
        || request.repository.is_none()
    {
        return false;
    }
    match (&operation.input.behavior, &workspace.source) {
        (
            LifecycleBehavior::RepositoryChoice | LifecycleBehavior::PublicRepository,
            WorkspaceSource::PublicRepositorySnapshot { url, commit, .. },
        ) => {
            operation.repository_access_pin.is_none()
                && operation.repository_commit.is_none()
                && operation
                    .public_repository
                    .as_ref()
                    .is_some_and(|pin| &pin.url == url && &pin.commit == commit)
        }
        (
            LifecycleBehavior::RepositoryChoice,
            WorkspaceSource::RepositorySnapshot { url, reference, .. },
        ) => {
            request.initial_message.is_some()
                && operation.public_repository.is_none()
                && operation.repository_access_pin.as_ref().is_some_and(|pin| {
                    pin.owner == operation.owner
                        && request
                            .repository
                            .as_ref()
                            .is_some_and(|repository| repository.url == pin.repository)
                        && *url == format!("https://github.com/{}", pin.connection.repository)
                        && reference.as_ref() == Some(&pin.commit)
                        && operation.repository_commit.as_ref() == Some(&pin.commit)
                })
        }
        (
            LifecycleBehavior::PromptRepository | LifecycleBehavior::ManagedRepository,
            WorkspaceSource::RepositorySnapshot { url, reference, .. },
        ) => {
            request.initial_message.is_some()
                && request.repository.as_ref().is_some_and(|repository| {
                    &repository.url == url
                        && operation
                            .repository_commit
                            .as_ref()
                            .or(repository.reference.as_ref())
                            == reference.as_ref()
                })
        }
        _ => false,
    }
}

async fn has_other_authority(
    store: &MongoSandboxStore,
    workspace: &Workspace,
    session: &mut ClientSession,
) -> Result<bool, ApplyError> {
    let sandbox = workspace.sandbox_id.as_str();
    for (collection, filter) in [
        (
            &store.workspaces,
            doc! {"sandbox_id":sandbox, "_id":{"$ne":workspace.id.as_str()}},
        ),
        (&store.bindings.records, doc! {"sandbox_id":sandbox}),
        (&store.resource_shares, doc! {"sandbox_id":sandbox}),
        (&store.sandbox_sessions, doc! {"sandbox_id":sandbox}),
        (&store.agent_reuse_bindings, doc! {"sandbox_id":sandbox}),
        (&store.warm_sandboxes, doc! {"_id":sandbox}),
        (&store.native_prepared, doc! {"_id":sandbox}),
    ] {
        if collection
            .find_one(filter)
            .projection(doc! {"_id": 1})
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?
            .is_some()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Some(None) means the failed workspace has no Agent reservation yet.
async fn fence_starting_hierarchy(
    store: &MongoSandboxStore,
    workspace: &Workspace,
    operation: &LifecycleRecord,
    session: &mut ClientSession,
) -> Result<Option<Option<String>>, ApplyError> {
    let sandbox = workspace.sandbox_id.as_str();
    let agent = find(&store.agents, doc! {"sandbox_id":sandbox}, session).await?;
    let Some(agent) = agent
        .as_ref()
        .map(read_agent)
        .transpose()
        .map_err(ApplyError::Domain)?
    else {
        let free = operation.agent_id.is_none()
            && find(&store.worktrees, doc! {"sandbox_id":sandbox}, session)
                .await?
                .is_none();
        return Ok(free.then_some(None));
    };
    if !never_dispatched(&agent, workspace, operation)
        || find(
            &store.agents,
            doc! {"sandbox_id":sandbox, "_id":{"$ne":agent.id.as_str()}},
            session,
        )
        .await?
        .is_some()
        || find(
            &store.worktrees,
            doc! {"sandbox_id":sandbox, "_id":{"$ne":agent.worktree_id.as_str()}},
            session,
        )
        .await?
        .is_some()
    {
        return Ok(None);
    }
    let document = find(
        &store.worktrees,
        doc! {"_id":agent.worktree_id.as_str()},
        session,
    )
    .await?;
    let Some(worktree) = document
        .as_ref()
        .map(read_worktree)
        .transpose()
        .map_err(ApplyError::Domain)?
    else {
        return Ok(None);
    };
    if worktree.owner != workspace.owner
        || worktree.sandbox_id != workspace.sandbox_id
        || worktree.workspace_id != workspace.id
        || worktree.agent_id.as_ref() != Some(&agent.id)
        || worktree.status != WorktreeStatus::Creating
        || !worktree.base_commit.is_empty()
    {
        return Ok(None);
    }
    // Current control or Agent work must conflict with this teardown decision.
    for (collection, id, revision) in [
        (&store.agents, agent.id.as_str(), agent.revision),
        (&store.worktrees, worktree.id.as_str(), worktree.revision),
    ] {
        let revision = signed(revision, "repository cleanup child revision")
            .map_err(SandboxStoreError::Corrupt)
            .map_err(ApplyError::Domain)?;
        let changed = collection
            .update_one(
                doc! {"_id":id,"revision":revision,
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                    "channel_reuse_cleanup_execution": { "$exists": false },
                },
                doc! {"$inc":{"repository_cleanup_fence":1_i64}},
            )
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?;
        if changed.matched_count != 1 {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
    }
    Ok(Some(Some(agent.id.to_string())))
}

fn never_dispatched(agent: &Agent, workspace: &Workspace, operation: &LifecycleRecord) -> bool {
    agent.owner == workspace.owner
        && agent.sandbox_id == workspace.sandbox_id
        && agent.workspace_id == workspace.id
        && agent.origin
            == (AgentOrigin::Lifecycle {
                operation_id: operation.id.clone(),
            })
        && operation
            .agent_id
            .as_deref()
            .is_none_or(|id| id == agent.id.as_str())
        && agent.status == AgentStatus::Starting
        && agent.work.step == AgentWorkStep::WaitForWorkspace
        && agent.requested_stop_reason.is_none()
        && agent.has_no_runtime_effects()
        && agent
            .conversation
            .first_message
            .as_ref()
            .is_some_and(|first| {
                first.state == FirstMessageDelivery::Queued
                    && match operation.input.request.initial_message.as_deref() {
                        Some(message) => {
                            first.purpose == Some(FirstMessagePurpose::Task)
                                && first.text == message
                        }
                        None => {
                            operation.public_repository.is_some()
                                && first.purpose == Some(FirstMessagePurpose::Setup)
                                && operation.input.setup_message.as_deref()
                                    == Some(first.text.as_str())
                        }
                    }
                    && first.attempt == 0
                    && first.dispatch_generation == 0
                    && first.dispatch_fenced_at.is_none()
            })
}

async fn find(
    collection: &Collection<Document>,
    filter: Document,
    session: &mut ClientSession,
) -> Result<Option<Document>, ApplyError> {
    collection
        .find_one(filter)
        .session(session)
        .await
        .map_err(ApplyError::Driver)
}
