//! Admission orchestration inside the root's exclusive transaction.
//! Domain helpers below never change the transaction lifecycle.
use super::transaction_scope::TransactionScope;
use crate::store::{
    agent_admission::{
        attribution, classify_agent_insert, classify_aggregate_insert, classify_log_insert,
        initial_integrity::initial_integrity,
        next_workspace,
        parent_fences::{advance_sandbox, advance_workspace},
        trigger_protocol, ApplyError,
    },
    agent_admission_records::{replay_in_transaction, AdmissionReceipt, CreatedReceipt},
    agents::agent_document,
    sandbox_events::append_in_transaction,
    sandbox_mongo::MongoSandboxStore,
    sandboxes::read_sandbox,
    workspaces::read_workspace,
    worktrees::worktree_document,
};
use layer2_core::sandbox::{
    store::{AgentAdmission, SandboxStoreError},
    AgentOrigin, SandboxEventKind, SandboxReviveCause, WorkspaceStatus,
};
use mongodb::bson::doc;
mod history;
pub(in crate::store) mod replay;
mod reuse;
use reuse::reuse_profile_agent;

#[cfg(test)]
pub(in crate::store) async fn apply(
    store: &MongoSandboxStore,
    admission: &AgentAdmission,
    reuse_existing: bool,
    initiation: Option<&layer2_core::trigger_domain::LaunchInitiation>,
    scope: &mut TransactionScope<'_>,
) -> Result<AdmissionReceipt, ApplyError> {
    apply_catalogue(store, admission, reuse_existing, false, initiation, scope).await
}

pub(in crate::store) async fn apply_catalogue(
    store: &MongoSandboxStore,
    admission: &AgentAdmission,
    reuse_existing: bool,
    catalogue: bool,
    initiation: Option<&layer2_core::trigger_domain::LaunchInitiation>,
    scope: &mut TransactionScope<'_>,
) -> Result<AdmissionReceipt, ApplyError> {
    store
        .bindings
        .admit_agent(admission, scope.session())
        .await
        .map_err(ApplyError::from)?;
    let persisted_launch = trigger_protocol::selected(store, admission, scope.session()).await?;
    let common_launch = persisted_launch
        .as_ref()
        .filter(|launch| launch.input_protocol.is_some());
    if let Some(launch) = common_launch {
        if launch.initiation.as_ref() != initiation {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
        launch
            .initiation
            .as_ref()
            .ok_or(ApplyError::Domain(SandboxStoreError::Conflict))?
            .require_agent_admission(admission)
            .map_err(ApplyError::Domain)?;
        // An accepted receipt may be replayed after its original worker claim
        // has ended. New work still requires the live initiation fence below.
        if let Some(replay) = replay_in_transaction(store, admission, scope.session()).await? {
            return Ok(replay);
        }
    }
    if let Some(initiation) = initiation {
        attribution::fence_initiation(store, admission, initiation, scope.session()).await?;
    }
    if let Some(replay) = replay_in_transaction(store, admission, scope.session()).await? {
        return Ok(replay);
    }

    if let Some(launch) = &persisted_launch {
        let database = store.client.database(&store.agents.namespace().db);
        if !crate::store::trigger_definitions::fence_new_agent_publisher(
            &database,
            launch,
            scope.session(),
        )
        .await
        .map_err(ApplyError::Driver)?
        {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
    }

    let open = store
        .workspaces
        .update_one(
            doc! {"_id":admission.workspace_id.as_str(),
            "channel_reuse_cleanup_execution":{"$exists":false},
            "owned_workspace_cleanup_admission_v1":{"$exists":false}},
            doc! {"$inc":{"owned_workspace_admission_fence":1_i64}},
        )
        .session(scope.session())
        .await
        .map_err(ApplyError::Driver)?;
    if open.matched_count != 1 {
        return Err(ApplyError::Domain(SandboxStoreError::WorkspaceNotReady));
    }
    let workspace = store
        .workspaces
        .find_one(doc! { "_id": admission.workspace_id.as_str() })
        .session(scope.session())
        .await
        .map_err(ApplyError::Driver)?
        .as_ref()
        .map(read_workspace)
        .transpose()
        .map_err(ApplyError::Domain)?
        .ok_or(ApplyError::Domain(SandboxStoreError::WorkspaceNotReady))?;
    if workspace.owner != admission.owner {
        return Err(ApplyError::Domain(SandboxStoreError::WorkspaceNotReady));
    }
    let sandbox = store
        .sandboxes
        .find_one(doc! { "_id": workspace.sandbox_id.as_str() })
        .session(scope.session())
        .await
        .map_err(ApplyError::Driver)?
        .as_ref()
        .map(read_sandbox)
        .transpose()
        .map_err(ApplyError::Domain)?
        .ok_or(ApplyError::Domain(SandboxStoreError::SandboxNotReady))?;
    if sandbox.owner != workspace.owner {
        return Err(ApplyError::Domain(SandboxStoreError::WorkspaceNotReady));
    }
    let bootstrap = admission.admits_bootstrap(&workspace, &sandbox);
    if !bootstrap && workspace.status != WorkspaceStatus::Ready {
        return Err(ApplyError::Domain(SandboxStoreError::WorkspaceNotReady));
    }
    if !bootstrap && !sandbox.accepts_creation_admission() {
        return Err(ApplyError::Domain(SandboxStoreError::SandboxNotReady));
    }
    // Before reuse too: reusing a profile Agent revives a Paused Sandbox, and
    // revival is refused under a platform stop (contract section 5.1).
    crate::store::budget::require_open(
        store,
        crate::store::budget::now_seconds(),
        admission.admission_cause(),
        scope.session(),
    )
    .await?;
    // Keep the bounded publication future off the admission caller stack.
    if let Some(creation) = Box::pin(reuse_profile_agent(
        store,
        admission,
        &workspace,
        &sandbox,
        reuse_existing,
        common_launch,
        scope,
    ))
    .await?
    {
        return Ok(creation);
    }
    if matches!(admission.origin, AgentOrigin::WorkspaceProfile { .. })
        && sandbox.status != layer2_core::sandbox::SandboxStatus::Active
    {
        return Err(ApplyError::Domain(SandboxStoreError::SandboxNotReady));
    }

    if catalogue {
        let profile = &admission.profile;
        let database = store.client.database(&store.agents.namespace().db);
        if !crate::store::profile_pin_admission::agent(
            &database,
            profile.id.as_str(),
            profile.revision,
            Some(&profile.digest),
            scope.session(),
        )
        .await
        .map_err(ApplyError::Driver)?
        {
            crate::store::profile_pin_admission::frozen::agent(
                &database,
                admission,
                &sandbox,
                &workspace,
                scope.session(),
            )
            .await
            .map_err(ApplyError::Driver)?
            .require()
            .map_err(ApplyError::Domain)?;
        }
    }

    let live = crate::store::shared_child_capacity::agents(
        store,
        &sandbox.id,
        &workspace.id,
        scope.session(),
    )
    .await
    .map_err(|error| match error {
        crate::store::shared_child_capacity::CapacityError::Domain(error) => {
            ApplyError::Domain(error)
        }
        crate::store::shared_child_capacity::CapacityError::Driver(error) => {
            ApplyError::Driver(error)
        }
    })?;
    if live >= u64::from(sandbox.limits.max_agents_per_workspace) {
        return Err(ApplyError::Domain(SandboxStoreError::AgentLimitReached {
            limit: sandbox.limits.max_agents_per_workspace,
            live: u8::try_from(live).unwrap_or(u8::MAX),
        }));
    }

    let mut creation = match Box::pin(crate::store::native_prepared::adopt_agent(
        store,
        &sandbox,
        &workspace,
        admission,
        scope.session(),
    ))
    .await
    .map_err(ApplyError::Domain)?
    {
        Some(creation) => creation,
        None => Box::pin(crate::store::warm_sandboxes::adopt_agent(
            store,
            &sandbox,
            &workspace,
            admission,
            scope.session(),
        ))
        .await
        .map_err(ApplyError::Domain)?,
    };
    creation.validate_replay().map_err(ApplyError::Domain)?;
    let provenance = crate::store::agent_provenance::record_creation(
        store,
        admission,
        &creation.agent,
        &sandbox,
        initiation,
        scope.session(),
    )
    .await
    .map_err(|error| match error {
        crate::store::resource_access::ReadError::Driver(error) => classify_agent_insert(error),
        crate::store::resource_access::ReadError::Domain(error) => {
            ApplyError::Domain(SandboxStoreError::Corrupt(error.to_string()))
        }
    })?;
    if store
        .agent_log
        .find_one(doc! { "agent_id": creation.agent.id.as_str() })
        .projection(doc! { "_id": 1 })
        .session(scope.session())
        .await
        .map_err(ApplyError::Driver)?
        .is_some()
    {
        return Err(ApplyError::Domain(SandboxStoreError::SequenceConflict));
    }
    let integrity = initial_integrity(&creation).map_err(ApplyError::Domain)?;
    let next_workspace = next_workspace(&workspace)?;
    let mut next_sandbox = sandbox.clone();
    next_sandbox
        .revision
        .checked_add(1)
        .ok_or(ApplyError::Domain(SandboxStoreError::Conflict))?;
    let cause = match admission.origin {
        AgentOrigin::Manual
        | AgentOrigin::Lifecycle { .. }
        | AgentOrigin::WorkspaceProfile { .. } => SandboxReviveCause::AgentCreation,
        AgentOrigin::Trigger { .. } => SandboxReviveCause::TriggerLaunch,
    };
    let destination = store
        .managed_resume_destination(&sandbox, scope.session())
        .await
        .map_err(ApplyError::Domain)?;
    let revival = admission
        .record_parent_admission(&mut next_sandbox, &workspace, cause, destination.as_deref())
        .map_err(ApplyError::Domain)?;
    advance_workspace(store, &workspace, &next_workspace, scope.session()).await?;
    advance_sandbox(store, &sandbox, &next_sandbox, scope.session()).await?;

    store
        .worktrees
        .insert_one(worktree_document(&creation.worktree).map_err(ApplyError::Domain)?)
        .session(scope.session())
        .await
        .map_err(classify_aggregate_insert)?;
    let mut agent_document = agent_document(&creation.agent).map_err(ApplyError::Domain)?;
    agent_document.insert("agent_log_integrity_v1", integrity.integrity);
    store
        .agents
        .insert_one(agent_document)
        .session(scope.session())
        .await
        .map_err(classify_agent_insert)?;
    store
        .agent_log
        .clone_with_type::<mongodb::bson::RawDocumentBuf>()
        .insert_one(&integrity.documents[0])
        .session(scope.session())
        .await
        .map_err(classify_log_insert)?;

    let onboarding = store.onboarding_command_admission
        && matches!(admission.origin, AgentOrigin::Lifecycle { .. })
        && admission
            .created_by
            .as_ref()
            .and_then(|actor| actor.ordinary_subject())
            == Some(&admission.owner);
    if common_launch.is_some() || onboarding {
        Box::pin(crate::store::trigger_first_messages::attach(
            store,
            common_launch,
            admission,
            &mut creation.agent,
            scope.session(),
        ))
        .await
        .map_err(ApplyError::from)?;
    }
    let revival_event = match revival {
        Some(event) => Some(
            append_in_transaction(
                store,
                scope.session(),
                &sandbox.id,
                admission.event_at.clone(),
                event,
            )
            .await?,
        ),
        None => None,
    };
    let worktree_event = append_in_transaction(
        store,
        scope.session(),
        &sandbox.id,
        admission.event_at.clone(),
        SandboxEventKind::WorktreeCreated {
            workspace_id: workspace.id.clone(),
            worktree_id: creation.worktree.id.clone(),
        },
    )
    .await?;
    let agent_event = append_in_transaction(
        store,
        scope.session(),
        &sandbox.id,
        admission.event_at.clone(),
        SandboxEventKind::AgentCreated {
            workspace_id: workspace.id,
            worktree_id: creation.worktree.id.clone(),
            agent_id: creation.agent.id.clone(),
        },
    )
    .await?;
    Ok(AdmissionReceipt::Created(Box::new(CreatedReceipt {
        provenance,
        creation,
        workspace: next_workspace,
        sandbox: next_sandbox,
        revival_event,
        worktree_event,
        agent_event,
    })))
}
