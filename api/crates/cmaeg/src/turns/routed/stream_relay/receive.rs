use super::*;
use crate::turns::routed::{
    custom_delivery::is_custom_origin, dispatcher::current_target, model::delivery_event,
    DeliveryStatus, Phase, ReplayEffect, RoutedToolReplies, TargetState,
};
use axum::{body::Bytes, extract::State, http::StatusCode};
use cmaeg_core::channel::Sink;
use std::sync::Arc;

pub(in crate::turns::routed) async fn receive(
    State(replies): State<Arc<RoutedToolReplies>>,
    bytes: Bytes,
) -> Result<Bytes, StatusCode> {
    if replies.replica_relay.is_none() || replies.journal.partitions.is_none() {
        return Err(StatusCode::NOT_FOUND);
    }
    let envelope: Envelope = serde_json::from_slice(&bytes).map_err(|_| StatusCode::BAD_REQUEST)?;
    let request: Request = codec::open(
        &replies.journal.keys,
        Direction::StreamRequest,
        CONTEXT,
        (replies.journal.clock)(),
        &envelope,
    )
    .map_err(|_| StatusCode::UNAUTHORIZED)?;
    let outcome = match tokio::time::timeout(WINDOW, deliver(&replies, &request)).await {
        Ok(Ok(value)) => value,
        _ => return Err(StatusCode::CONFLICT),
    };
    let now = (replies.journal.clock)();
    let response = codec::seal(
        &replies.journal.keys,
        Direction::StreamResponse,
        &envelope.id,
        &storage::hash(&bytes),
        now + WINDOW.as_secs() as i64,
        now,
        &outcome,
    )
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    storage::bytes(&response)
        .map(Bytes::from)
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
}

async fn deliver(
    replies: &RoutedToolReplies,
    request: &Request,
) -> Result<LocalStreamOutcome, ExecutionError> {
    let partitions = replies
        .journal
        .partitions
        .as_ref()
        .ok_or(ExecutionError::Unsupported)?;
    let plan = replies
        .plans
        .load(&request.identity)
        .await
        .map_err(super::super::dispatcher::admission_error)?
        .ok_or(ExecutionError::Invalid)?;
    let target = plan
        .targets
        .iter()
        .find(|target| target.id == request.target)
        .ok_or(ExecutionError::Invalid)?;
    let event = delivery_event(&plan, target);
    if waiter_owner(&event) != Some(partitions.membership().instance.as_str()) {
        return Err(ExecutionError::Invalid);
    }
    let (state, version) = replies
        .journal
        .read::<TargetState>(&target.id)
        .await?
        .ok_or(ExecutionError::Invalid)?;
    validate(&state, request, &plan, (replies.journal.clock)())?;
    let body = replies.journal.retained_body(&state).await?;
    body.validate_settlement(
        &plan,
        target,
        &replies.journal.keys,
        (replies.journal.clock)(),
    )?;
    if !is_custom_origin(&body) {
        return Err(ExecutionError::Invalid);
    }
    let current = current_target(replies.authority.as_ref(), &plan, target).await?;
    if current.version != request.channel_version
        || !current.definition.reply.sinks.contains(&Sink::Sync {})
    {
        return Err(ExecutionError::AuthorityChanged);
    }
    let owner = partitions.owner(&plan, target).await?;
    if owner.fence.instance != request.partition.instance
        || owner.fence.epoch != request.partition.epoch
        || owner.fence.claim != request.partition.claim
        || owner.fence.until_ms < request.partition.until_ms
        || request.partition.until_ms <= (replies.journal.clock)().saturating_mul(1000)
    {
        return Err(ExecutionError::LostLease);
    }
    // Recheck the exact intent after authority/body awaits. This never adopts
    // the remote worker's lease and never changes durable execution state.
    let (latest, latest_version) = replies
        .journal
        .read::<TargetState>(&target.id)
        .await?
        .ok_or(ExecutionError::Invalid)?;
    if latest_version != version {
        return Err(ExecutionError::LostLease);
    }
    validate(&latest, request, &plan, (replies.journal.clock)())?;
    let custom = replies
        .coordinator
        .custom
        .as_ref()
        .ok_or(ExecutionError::Unsupported)?;
    let ReplyAddress::Sync { waiter } = &event.reply else {
        return Err(ExecutionError::Invalid);
    };
    if !custom
        .waiters
        .claim_relay(waiter, request.checkpoint.cursor)
    {
        return Ok(LocalStreamOutcome::Unavailable);
    }
    Ok(
        if body
            .metadata
            .contains_key(super::super::media_schedule::KEY)
        {
            custom.local_response(&event, &request.effect, &request.checkpoint.response_id)
        } else {
            custom.local(&event, &request.effect)
        },
    )
}

pub(super) fn validate(
    state: &TargetState,
    request: &Request,
    plan: &AdmittedPlan,
    now: i64,
) -> Result<(), ExecutionError> {
    let checkpoint = state.delivery.as_ref().ok_or(ExecutionError::Invalid)?;
    let effect_hash = storage::hash(&storage::bytes(&ReplayEffect::Custom(
        request.effect.clone(),
    ))?);
    if state.id != request.target
        || state.root != plan.admission_id
        || state.owner != plan.identity.owner
        || state.request_hash != plan.request_hash
        || state.retained_until != plan.retained_until
        || state.ownership != request.ownership
        || state.ownership.until <= now
        || state.retained_until <= now
        || state.event_expires_at <= now
        || request.partition.until_ms <= now.saturating_mul(1000)
        || !matches!(&state.phase, Phase::Accepted { response_id, .. } if response_id == &checkpoint.response_id)
        || !matches!(checkpoint.status, DeliveryStatus::Intent)
        || checkpoint.custom.as_ref().and_then(|c| c.local) != Some(LocalStreamOutcome::Pending)
        || storage::bytes(checkpoint)? != storage::bytes(&request.checkpoint)?
        || checkpoint.effect_hash != effect_hash
        || checkpoint.terminal != request.effect.terminal()
    {
        return Err(ExecutionError::LostLease);
    }
    Ok(())
}
