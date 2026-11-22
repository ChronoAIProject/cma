use super::State;
use crate::sandbox::binding::{
    reset_binding_eligibility, reset_eligibility, BindingConversation, BindingError,
    ConversationHead, ConversationResetFormat, ConversationResetReceipt, ConversationResetRequest,
    ConversationResetState,
};

pub(super) fn apply(
    state: &mut State,
    request: ConversationResetRequest,
    enabled: bool,
) -> Result<ConversationResetReceipt, BindingError> {
    request.validate()?;
    let binding = state
        .bindings
        .get(&request.binding_id)
        .cloned()
        .ok_or(BindingError::NotFound)?;
    let scope = state
        .bindings
        .get(&request.idempotency_binding_id)
        .cloned()
        .ok_or(BindingError::NotFound)?;
    request.authorize(&binding, &scope)?;
    ConversationResetState::validate_format(
        &binding,
        state.binding_resets.get(&binding.binding_id),
    )
    .map_err(BindingError::Corrupt)?;
    ConversationResetState::validate_format(&scope, state.binding_resets.get(&scope.binding_id))
        .map_err(BindingError::Corrupt)?;
    let mut resets = state
        .binding_resets
        .get(&request.binding_id)
        .cloned()
        .unwrap_or_default();
    let mut receipts = state
        .binding_resets
        .get(&request.idempotency_binding_id)
        .cloned()
        .unwrap_or_default();
    if let Some(receipt) = receipts.replay(&request)? {
        return Ok(receipt);
    }
    if !enabled {
        return Err(BindingError::ResetRefused("partition_reset_disabled"));
    }
    reset_binding_eligibility(&binding)?;
    let head = resets.head(binding.generation, &request.conversation_id);
    let id = head.map_or(request.conversation_id.as_str(), |h| {
        h.successor.conversation_id.as_str()
    });
    let previous = state
        .binding_conversations
        .get(&format!(
            "{}:{:020}:{}",
            binding.binding_id, binding.generation, id
        ))
        .cloned()
        .ok_or(BindingError::ResetRefused(
            "partition_reset_mapping_missing",
        ))?;
    if previous.agent_id != request.expected_agent_id {
        return Err(BindingError::ResetRefused(
            "partition_reset_mapping_changed",
        ));
    }
    reset_eligibility(
        &binding,
        &previous,
        state.agents.get(&previous.agent_id),
        state.sandboxes.get(&binding.sandbox_id),
        state.workspaces.get(&binding.workspace_id),
    )?;
    super::admission::require_operator_open(state, crate::admission::AdmissionCause::Fresh)?;
    if (resets.heads.len() >= 256 && head.is_none()) || receipts.receipts.len() >= 512 {
        return Err(BindingError::ResetRefused("partition_reset_capacity"));
    }
    let incarnation = head
        .map_or(Some(1), |h| h.incarnation.checked_add(1))
        .ok_or(BindingError::Conflict)?;
    let internal = crate::profiles::content_digest(&(
        &binding.binding_id,
        binding.generation,
        &request.conversation_id,
        incarnation,
    ))
    .map_err(|e| BindingError::Corrupt(e.to_string()))?;
    let successor =
        BindingConversation::new(&binding, format!("inc_{internal}"), crate::now_rfc3339())
            .map_err(BindingError::Corrupt)?;
    let head = ConversationHead {
        transport_conversation_id: request.conversation_id.clone(),
        incarnation,
        successor,
    };
    let receipt = ConversationResetReceipt {
        request: request.clone(),
        previous,
        head: head.clone(),
    };
    resets.heads.insert(
        ConversationResetState::key(binding.generation, &request.conversation_id),
        head,
    );
    if request.idempotency_binding_id == request.binding_id {
        resets
            .receipts
            .insert(request.operation_id.clone(), receipt.clone());
    } else {
        receipts
            .receipts
            .insert(request.operation_id.clone(), receipt.clone());
    }
    let at = crate::now_rfc3339();
    let promote = |mut value: crate::sandbox::binding::ProviderBinding,
                   resets: &ConversationResetState| {
        if !resets
            .within_capacity()
            .map_err(|error| BindingError::Corrupt(error.to_string()))?
        {
            return Err(BindingError::ResetRefused("partition_reset_capacity"));
        }
        value.reset_format = Some(ConversationResetFormat::V1);
        value
            .advance_revision(at.clone())
            .map_err(BindingError::Corrupt)?;
        ConversationResetState::validate_format(&value, Some(resets))
            .map_err(BindingError::Corrupt)?;
        Ok(value)
    };
    let next = promote(binding, &resets)?;
    let next_scope = if request.idempotency_binding_id != request.binding_id {
        Some(promote(scope, &receipts)?)
    } else {
        None
    };
    if let Some(scope) = next_scope {
        state.bindings.insert(scope.binding_id.clone(), scope);
        state
            .binding_resets
            .insert(request.idempotency_binding_id, receipts);
    }
    state.bindings.insert(next.binding_id.clone(), next);
    state.binding_resets.insert(request.binding_id, resets);
    Ok(receipt)
}
