//! Conversation publication and capacity settlement share one atomic writer.
use super::*;
use crate::store::agent_log_integrity::trusted_summary_at_head;

#[cfg(test)]
pub(super) async fn apply_mutation(
    caller: ScanCaller,
    store: &MongoSandboxStore,
    mutation: &AgentConversationMutation,
    session: &mut mongodb::ClientSession,
) -> Result<ConversationReceipt, ApplyError> {
    apply_mutation_with_capacity(caller, store, mutation, session, None, None).await
}

pub(in crate::store) async fn apply_mutation_with_capacity(
    caller: ScanCaller,
    store: &MongoSandboxStore,
    mutation: &AgentConversationMutation,
    session: &mut mongodb::ClientSession,
    capacity: Option<&layer2_core::sandbox::store::AgentConversationCapacitySettlement>,
    exact_checkpoint: Option<&layer2_core::sandbox::store::AgentMessageCheckpoint>,
) -> Result<ConversationReceipt, ApplyError> {
    let current_row = crate::store::stale_turns::find_with_proof_budget(
        &store.agents,
        doc! { "_id": mutation.agent.id.as_str() },
        session,
        false,
    )
    .await
    .map_err(ApplyError::Driver)?
    .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    let current = read_agent(&current_row).map_err(ApplyError::Domain)?;
    before(store, &current, mutation, session).await?;
    // These alternative proof futures are large in debug builds. Keep their
    // state off the nested Trigger/conversation publication stack.
    let summary = if exact_checkpoint.is_some() {
        Box::pin(trusted_summary_at_head(
            store,
            &current,
            &current_row,
            session,
        ))
        .await?
    } else {
        Box::pin(trusted_summary(
            store,
            &current,
            &current_row,
            caller,
            session,
            false,
        ))
        .await?
    };
    let (actual_last_seq, latest_turn, log_summary, semantic) = match summary {
        Some(summary) => {
            let semantic = summary.semantic().cloned().ok_or_else(|| {
                ApplyError::Domain(SandboxStoreError::Corrupt(
                    "trusted Agent log metadata has no semantic checkpoint".into(),
                ))
            })?;
            (
                summary.entry_count(),
                semantic.latest_turn(),
                summary,
                semantic,
            )
        }
        None if exact_checkpoint.is_some() => {
            // The stale-turn proof budget cannot authorize a full-log audit.
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
        None => {
            let mut entries = Vec::new();
            let summary = Box::pin(scan_complete_log_with_session(
                caller,
                store,
                &mutation.agent.id,
                current.conversation.last_seq,
                session,
                |entry| {
                    entries.push(entry);
                    Ok(())
                },
            ))
            .await?;
            let semantic = Box::pin(crate::store::agent_log_integrity::audit_checkpoint(
                store, &current, &entries, session,
            ))
            .await?;
            (
                summary.entry_count(),
                semantic.latest_turn(),
                summary,
                semantic,
            )
        }
    };
    if let Some(proof) = capacity {
        proof
            .validate_snapshot(
                &current,
                checkpoint::read(&current_row)
                    .map_err(ApplyError::Domain)?
                    .as_ref(),
            )
            .map_err(ApplyError::Domain)?;
        let refused = proof
            .refused()
            .iter()
            .map(|entry| {
                mongodb::bson::RawDocumentBuf::try_from(&crate::store::agent_log::log_document(
                    entry,
                )?)
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(ApplyError::Domain)?;
        match crate::store::agent_log_integrity::ensure_append_capacity(
            &log_summary,
            &refused,
            AppendCapacity::Normal,
        ) {
            Err(SandboxStoreError::AgentLogCapacityExceeded) => {}
            Ok(()) => return Err(ApplyError::Domain(SandboxStoreError::Conflict)),
            Err(error) => return Err(ApplyError::Domain(error)),
        }
    } else {
        validate_mutation(&current, actual_last_seq, mutation).map_err(ApplyError::Domain)?;
    }
    cleanup_admission::require(&current_row, &current, mutation).map_err(ApplyError::Domain)?;
    let mut logs = conversation_logs(mutation).map_err(ApplyError::Domain)?;
    let mut derived = None;
    if exact_checkpoint.is_none()
        && current.conversation.active_turn != mutation.agent.conversation.active_turn
    {
        let mut output = mutation.clone();
        let count = logs.len();
        checkpoint::finish_published_text(&current_row, &current, &mut output.agent, &mut logs)
            .map_err(ApplyError::Domain)?;
        if logs.len() > count {
            derived = Some(output);
        }
    }
    let mutation = derived.as_ref().unwrap_or(mutation);

    let semantic = semantic
        .after_append(&logs, &mutation.agent)
        .map_err(ApplyError::Domain)?;
    let integrity = prepare_append(
        &current,
        &log_summary,
        &logs,
        mutation.agent.revision,
        semantic,
        if capacity.is_some() || derived.is_some() {
            AppendCapacity::StopOrFailure
        } else {
            AppendCapacity::Normal
        },
    )
    .map_err(ApplyError::Domain)?;

    let command_dispatch = Box::pin(bind_domain(
        store,
        &current,
        latest_turn,
        mutation,
        capacity.is_some(),
        session,
    ))
    .await?;
    replace_agent(
        store,
        &current,
        mutation,
        integrity.integrity,
        exact_checkpoint,
        session,
    )
    .await?;
    if exact_checkpoint.is_none() {
        checkpoint::carry(store, &current_row, &current, &mutation.agent, session).await?;
    }
    insert_logs(store, &integrity.documents, session).await?;
    crate::store::suggestions::enqueue(store, &mutation.agent, &logs, session).await?;
    Ok(ConversationReceipt {
        segmented: None,
        command_dispatch,
        agent: mutation.agent.clone(),
        logs,
    })
}

// These domain helpers perform no transaction lifecycle operation. The audited
// segmented adapter calls them inside the caller's exclusive scope.
pub(in crate::store) async fn before(
    store: &MongoSandboxStore,
    current: &Agent,
    mutation: &AgentConversationMutation,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    store
        .fence_agent_effect_transaction(&mutation.agent.id, session)
        .await
        .map_err(|error| match error {
            crate::store::agent_ownership::FenceError::Domain(error) => ApplyError::Domain(error),
            crate::store::agent_ownership::FenceError::Driver(error) => ApplyError::Driver(error),
        })?;
    // Command reservation also writes this Agent, so this transaction cannot
    // miss a concurrently admitted message and then take its idle boundary.
    if mutation.is_managed_context_admission() && store.agent_commands.find_one(doc! {
        "agent_id": mutation.agent.id.as_str(), "status": {"$nin": ["completed", "failed", "cancelled"]}
    }).session(&mut *session).await.map_err(ApplyError::Driver)?.is_some() {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    crate::store::trigger_first_messages::require_mutation(store, current, mutation, session)
        .await
        .map_err(ApplyError::Domain)?;
    Ok(())
}

pub(in crate::store) fn require_cleanup(
    row: &mongodb::bson::Document,
    current: &Agent,
    mutation: &AgentConversationMutation,
) -> Result<(), ApplyError> {
    cleanup_admission::require(row, current, mutation).map_err(ApplyError::Domain)
}

pub(in crate::store) async fn bind_domain(
    store: &MongoSandboxStore,
    current: &Agent,
    latest_turn: Option<u64>,
    mutation: &AgentConversationMutation,
    capacity: bool,
    session: &mut mongodb::ClientSession,
) -> Result<Option<layer2_core::sandbox::agent_io::CommandDispatch>, ApplyError> {
    if !capacity {
        admission::validate_and_fence(store, current, mutation, session).await?;
    }
    crate::store::budget::apply(store, current, mutation, session).await?;
    let command_dispatch =
        crate::store::agent_commands::binding::bind(store, current, latest_turn, mutation, session)
            .await
            .map_err(|error| match error {
                crate::store::agent_commands::ApplyError::Domain(error) => {
                    ApplyError::Domain(crate::store::agent_commands::binding::domain_error(error))
                }
                crate::store::agent_commands::ApplyError::Driver(error) => {
                    ApplyError::Driver(error)
                }
            })?;
    Ok(command_dispatch)
}
