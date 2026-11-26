use super::history::History;
use super::*;
use crate::store::agent_admission_records::ReusedReceipt;
use crate::store::{
    agents::LIVE_STATUSES,
    sandbox_mongo::{signed, DOCUMENT_SCHEMA_VERSION},
};

pub(super) async fn reuse_profile_agent(
    store: &MongoSandboxStore,
    admission: &AgentAdmission,
    workspace: &layer2_core::sandbox::Workspace,
    sandbox: &layer2_core::sandbox::Sandbox,
    reuse_existing: bool,
    common_launch: Option<&layer2_core::trigger_domain::Launch>,
    scope: &mut TransactionScope<'_>,
) -> Result<Option<AdmissionReceipt>, ApplyError> {
    use crate::store::agents::read_agent;
    use crate::store::worktrees::read_worktree;
    let prepared_only = !reuse_existing && matches!(admission.origin, AgentOrigin::Trigger { .. });
    if !reuse_existing
        && !prepared_only
        && !matches!(admission.origin, AgentOrigin::WorkspaceProfile { .. })
    {
        return Ok(None);
    }
    let open = store
        .workspaces
        .update_one(
            doc! {"_id":workspace.id.as_str(),"revision":workspace.revision as i64,
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
    let mut query = doc! {
        "owner": admission.owner.as_str(),
        "workspace_id": workspace.id.as_str(),
        "record.profile.id": admission.profile.id.as_str(),
        "record.profile.digest": &admission.profile.digest,
        "status": { "$in": LIVE_STATUSES },
        "record.requested_stop_reason": { "$exists": false },
        "channel_reuse_cleanup_admission": { "$exists": false },
    };
    if prepared_only {
        query.insert("record.origin.type", "workspace_profile");
        query.insert("record.work.warm_conversation_id", doc! { "$exists": true });
        query.insert(
            "record.conversation.first_message",
            doc! { "$exists": false },
        );
    } else if !reuse_existing {
        query.insert("record.origin.type", "trigger");
        query.insert(
            "$or",
            mongodb::bson::bson!([
                { "creation_key": { "$regex": "^trigger-profile:" } },
                { "record.work.warm_conversation_id": { "$exists": true } },
            ]),
        );
        query.remove("status");
        query.remove("record.requested_stop_reason");
    }
    let Some(document) = store
        .agents
        .find_one(query)
        .sort(doc! { "record.created_at": -1, "_id": -1 })
        .session(scope.session())
        .await
        .map_err(ApplyError::Driver)?
    else {
        return Ok(None);
    };
    let mut agent = read_agent(&document).map_err(ApplyError::Domain)?;
    if !admission.can_reuse(&agent, workspace, reuse_existing) {
        return Ok(None);
    }
    // Selection always writes the actual candidate, including reuse that only
    // binds a creation key and adds no first prompt. Closure must serialize
    // with this transaction before the binding can commit.
    let changed = store
        .agents
        .update_one(
            doc! {"_id":agent.id.as_str(),"revision":signed(agent.revision,"Agent revision")
            .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error)))?,
            "channel_reuse_cleanup_admission":{"$exists":false}},
            doc! {"$inc":{"agent_reuse_admission_fence":1_i64}},
        )
        .session(scope.session())
        .await
        .map_err(ApplyError::Driver)?;
    if changed.matched_count != 1 {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    if prepared_only
        && store
            .agent_reuse_bindings
            .find_one(doc! { "agent_id": agent.id.as_str() })
            .session(scope.session())
            .await
            .map_err(ApplyError::Driver)?
            .is_some()
    {
        return Ok(None);
    }
    admission
        .materialize(workspace)
        .map_err(ApplyError::Domain)?;
    let history = Box::pin(History::read(store, &agent, &document, scope)).await?;
    let mut has_input = history.has_input;
    if prepared_only && has_input {
        return Ok(None);
    }
    if common_launch.is_some() {
        has_input |= store
            .agent_commands
            .find_one(doc! {"agent_id":agent.id.as_str()})
            .session(scope.session())
            .await
            .map_err(ApplyError::Driver)?
            .is_some();
    }
    let current = agent.clone();
    let kind = admission
        .initialize_reused_agent(&mut agent, has_input)
        .map_err(ApplyError::Domain)?;
    let attached_first = kind.is_some();
    let logs = kind
        .into_iter()
        .map(|kind| layer2_core::sandbox::AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: agent.conversation.last_seq,
            appended_at: admission.event_at.clone(),
            kind,
        })
        .collect::<Vec<_>>();
    let recorded =
        Box::pin(history.publish(store, admission, &current, &agent, &logs, scope)).await?;
    // Explicit reuse participates in the same parent fences as creation, including a race
    // against automatic profile children and inactivity pause.
    if reuse_existing || prepared_only || recorded.is_some() {
        let mut binding = doc! {
            "_id": crate::store::agent_admission_records::binding_id(admission).map_err(ApplyError::Domain)?,
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "owner": admission.owner.as_str(),
            "creation_key": &admission.creation_key,
            "fingerprint": admission.fingerprint().map_err(ApplyError::Domain)?,
            "agent_id": agent.id.as_str(),
            "sandbox_id": agent.sandbox_id.as_str(),
        };
        if let Some(recorded) = &recorded {
            binding.insert("document_schema_version", 2_i64);
            binding.insert(
                "log_publication",
                mongodb::bson::serialize_to_document(&recorded.publication)
                    .map_err(|e| ApplyError::Domain(SandboxStoreError::Corrupt(e.to_string())))?,
            );
        }
        store
            .agent_reuse_bindings
            .insert_one(binding)
            .session(scope.session())
            .await
            .map_err(classify_agent_insert)?;
    }
    if reuse_existing || prepared_only {
        let mut next_workspace = workspace.clone();
        next_workspace.revision = next_workspace
            .revision
            .checked_add(1)
            .ok_or(ApplyError::Domain(SandboxStoreError::Conflict))?;
        advance_workspace(store, workspace, &next_workspace, scope.session()).await?;
        let mut next_sandbox = sandbox.clone();
        let destination = store
            .managed_resume_destination(sandbox, scope.session())
            .await
            .map_err(ApplyError::Domain)?;
        let revival = next_sandbox
            .record_creation_admission_with_managed_runtime(
                SandboxReviveCause::TriggerLaunch,
                destination.as_deref(),
                admission.event_at.clone(),
            )
            .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error.to_string())))?;
        advance_sandbox(store, sandbox, &next_sandbox, scope.session()).await?;
        if let Some(event) = revival {
            append_in_transaction(
                store,
                scope.session(),
                &sandbox.id,
                admission.event_at.clone(),
                event,
            )
            .await?;
        }
    }
    if attached_first {
        if let Some(launch) = common_launch {
            Box::pin(crate::store::trigger_first_messages::attach(
                store,
                launch,
                admission,
                &mut agent,
                scope.session(),
            ))
            .await
            .map_err(ApplyError::from)?;
        }
    }
    let worktree = store
        .worktrees
        .find_one(doc! { "_id": agent.worktree_id.as_str() })
        .session(scope.session())
        .await
        .map_err(ApplyError::Driver)?
        .as_ref()
        .map(read_worktree)
        .transpose()
        .map_err(ApplyError::Domain)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    let creation = layer2_core::sandbox::store::AgentCreation {
        agent,
        worktree,
        initial_log: history.initial,
    };
    creation.validate_replay().map_err(ApplyError::Domain)?;
    Ok(Some(match recorded {
        Some(recorded) => AdmissionReceipt::Reused(Box::new(ReusedReceipt { creation, recorded })),
        None => AdmissionReceipt::Existing(Box::new(creation)),
    }))
}
