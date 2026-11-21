//! Read-only exact-parent capability for Manager's deferred media admission.
//! No caller-selected target, content read, provider call or execution lease.
use super::{
    dispatcher::{admission_error, current_target},
    model::delivery_event,
    Authority, ExecutionError, Journal, Phase, TargetState,
};
use crate::ingress::routing_store::{AdmittedPlan, PlanStore, StoredTarget};
use cmaeg_core::{
    ids::{BindingId, ChannelId, ConversationId, EventId},
    media_continuation::MediaArtifactClaim,
    seal::{EventRef, MAX_TOKEN_LENGTH},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// All identifiers are comparisons against the capability's original target.
/// This cannot select another target, attachment or filesystem path.
#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EventAttestationRequest {
    pub event_ref: EventRef,
    pub provider_binding_id: BindingId,
    pub channel_id: ChannelId,
    pub conversation_id: ConversationId,
    pub event_id: EventId,
    pub activity_sha256: String,
    /// Exact sealed capability expiry, as Unix seconds. This comparison lets
    /// Manager bound a deferred continuation without trusting caller expiry.
    pub event_expires_at: i64,
    /// The generation actually present in the frozen Gateway request. None is
    /// not a claim about Manager generation; Manager must check its own parent.
    pub binding_generation: Option<u64>,
    /// Required by attachment and supplement admission. An event-only result
    /// does not attest file bytes or derived text.
    pub artifact: Option<MediaArtifactClaim>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EventAttestationResponse {
    pub current: bool,
}

pub(super) async fn attest(
    plans: &PlanStore,
    journal: &Journal,
    authority: &dyn Authority,
    request: &EventAttestationRequest,
) -> Result<bool, ExecutionError> {
    if request.event_ref.as_str().len() > MAX_TOKEN_LENGTH
        || request.activity_sha256.len() != 64
        || request.binding_generation == Some(0)
    {
        return Ok(false);
    }
    let Ok(opened) = request.event_ref.open(&journal.keys, (journal.clock)()) else {
        return Ok(false);
    };
    let event = opened.payload;
    if event.lineage.is_some()
        || event.activity_sha256.as_ref() != Some(&request.activity_sha256)
        || event.channel_id != request.channel_id
        || event.conversation_id != request.conversation_id
        || event.event_id != request.event_id
        || event.expires_at != request.event_expires_at
    {
        return Ok(false);
    }
    let Some(binding) = journal.tool_reply_binding(&event).await? else {
        return Ok(false);
    };
    let plan = plans
        .load(&binding.identity)
        .await
        .map_err(admission_error)?
        .ok_or(ExecutionError::Invalid)?;
    if plan.admission_id != binding.root
        || plan.request_hash != binding.request_hash
        || !event.verifies_activity(&plan.activity)
    {
        return Err(ExecutionError::Corrupt);
    }
    let Some(target) = plan
        .targets
        .iter()
        .find(|target| target.id == binding.target)
    else {
        return Err(ExecutionError::Corrupt);
    };
    if delivery_event(&plan, target) != event
        || target.launch.provider.slug.as_str() != "cma_codex"
        || target.launch.provider.binding_id != request.provider_binding_id
    {
        return Ok(false);
    }
    let before = submitted(journal, &plan, target).await?;
    let generation = before
        .binding_preparation
        .as_ref()
        .map(|preparation| preparation.resolved_generation())
        .transpose()?
        .flatten();
    if generation != request.binding_generation {
        return Ok(false);
    }
    if let Some(artifact) = &request.artifact {
        if !super::MediaObligationStore(journal.clone())
            .attests(&plan, target, artifact)
            .await?
        {
            return Ok(false);
        }
    }
    // Bypass caches after reading provenance, so a stale retained body never proves
    // present authorization after its channel/source/consumer was revoked.
    current_target(authority, &plan, target).await?;
    let after = submitted(journal, &plan, target).await?;
    if before.body != after.body || before.binding_preparation != after.binding_preparation {
        return Err(ExecutionError::Corrupt);
    }
    Ok(event.expires_at > (journal.clock)() && plan.retained_until > (journal.clock)())
}

/// Freeze verified the body against this exact plan before committing the
/// checkpoint. Only immutable submission facts are needed for attestation;
/// do not reread private input chunks or acquire the response worker's lease.
async fn submitted(
    journal: &Journal,
    plan: &AdmittedPlan,
    target: &StoredTarget,
) -> Result<TargetState, ExecutionError> {
    let (state, _) = journal
        .read::<TargetState>(&target.id)
        .await?
        .ok_or(ExecutionError::Invalid)?;
    super::journal::validate(&state, plan, target)?;
    if !matches!(
        state.phase,
        Phase::Starting | Phase::Unknown | Phase::Accepted { .. } | Phase::Completed { .. }
    ) {
        return Err(ExecutionError::Invalid);
    }
    Ok(state)
}

#[cfg(test)]
#[path = "event_attestation_tests.rs"]
mod tests;
