use super::*;

/// Final Channel startup consumes the real provider-open confirmation in the
/// same transaction that publishes Ready. An adapter success without the exact
/// durable tool binding cannot create a tool-capable Channel session.
pub(in crate::store) async fn validate_and_fence_open(
    resources: &MongoSandboxStore,
    runtime: &RuntimeHandle,
    request: &AgentOpenRequest,
    authority_fence: u64,
    conversation: &str,
    session: &mut ClientSession,
) -> Result<(), OutboxError> {
    let agent_id = layer2_core::AgentId(request.options.slot.0.clone());
    let snapshot = super::super::agent_provenance::read_snapshot(resources, &agent_id, session)
        .await
        .map_err(|_| OutboxError::ToolUnavailable)?;
    let agent = snapshot
        .resource
        .lineage
        .agent
        .as_ref()
        .ok_or(OutboxError::ToolUnavailable)?;
    let sandbox = &snapshot.resource.lineage.sandbox;
    let tree = snapshot
        .resource
        .lineage
        .worktree
        .as_ref()
        .ok_or(OutboxError::ToolUnavailable)?;
    let expected = AgentOpenRequest::new(
        format!("agent:{}:open:v1", agent.id),
        crate::sandbox::runtime_options(agent, sandbox, tree),
        None,
    );
    if !request.has_valid_sha256()
        || request != &expected
        || !binding::valid_fence(agent, authority_fence)
        || conversation.is_empty()
        || conversation.len() > 512
        || conversation.chars().any(char::is_control)
    {
        return Err(OutboxError::SourceMismatch);
    }
    let id = records::identity(&agent.id, &agent.created_at, &agent.creation_fingerprint)?;
    let collection = super::super::durability::majority_collection::<Document>(
        &resources
            .client
            .database(&resources.sandboxes.namespace().db),
        COLLECTION,
    );
    let document = collection
        .find_one(doc! {"_id":&id,"revoked":false})
        .session(&mut *session)
        .await
        .map_err(unavailable)?
        .ok_or(OutboxError::ToolUnavailable)?;
    let record = records::read(&document)?;
    record.require_connection(&snapshot)?;
    if record.runtime.runtime_id != runtime.0
        || record.request_sha256 != request.request_sha256
        || record.opened_conversation.as_deref() != Some(conversation)
        || agent
            .conversation
            .conversation_id
            .as_deref()
            .is_some_and(|id| id != conversation)
    {
        return Err(OutboxError::SourceMismatch);
    }
    let changed = collection
        .update_one(
            doc! {"_id":&id,"revision":record.revision as i64,"revoked":false},
            doc! {"$inc":{"startup_fence":1_i64}},
        )
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    if changed.matched_count != 1 {
        return Err(OutboxError::Conflict);
    }
    super::super::resource_access::fence_lineage(resources, &snapshot.resource, session)
        .await
        .map_err(|_| OutboxError::Conflict)?;
    Ok(())
}
