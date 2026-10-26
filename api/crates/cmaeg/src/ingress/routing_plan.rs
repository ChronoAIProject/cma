//! Ordinary ingress planning only: no persistence, provider calls or new tokens.
//!
//! Selected verified ingress durably admits this plan before scheduling it. A
//! dispatcher must isolate delivery by consumer before using it. A
//! replay restores that original plan; it must not run routing against a newer
//! policy. Current owner/source/consumer authority still needs rechecking before
//! effects. Callback ownership and agent-published events use separate paths.
use super::launch::LaunchRequest;
use cmaeg_core::{
    activity::{Activity, ActivityKind, SourceKind},
    channel::{ProviderBinding, Source},
    ids::{compact_partition_key, ConsumerId, ConversationId, IdempotencyKey},
    routing::{self, Resolution, RouteError, RouteInput, Trace},
    seal::{event_ref::activity_digest, EventRefPayload, MasterKeyRing},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Authentication facts are supplied by the ingress verifier, never the event.
pub struct RoutingOptions<'a> {
    pub enabled: bool,
    /// False for custom binary-mode HMAC: its CloudEvent headers are unsigned.
    pub attributes_signed: bool,
    pub non_human_providers: &'a BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OrdinaryPlan {
    /// The caller retains its exact original launch request, including its
    /// sealed reference, default conversation, idempotency key and activity.
    Legacy,
    Routed(Box<RoutedPlan>),
}

/// An immutable decision in memory, not a durable admission or launch receipt.
#[derive(Debug, Clone, PartialEq)]
pub struct RoutedPlan {
    /// Every target receives exactly this activity and routing snapshot.
    pub activity: Activity,
    /// The admitted source configuration; a future dispatcher must not widen it.
    pub source: Source,
    /// Target order is the resolver's order, with the primary target first.
    pub targets: Vec<ConsumerLaunch>,
    pub trace: Trace,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsumerLaunch {
    pub consumer: ConsumerId,
    /// A binding snapshot, not a credential or permission to execute later.
    pub provider: ProviderBinding,
    pub idempotency_key: IdempotencyKey,
    /// Ready to seal only after durable admission; the original expiry and
    /// reply authority are retained. Default keeps consumer=None.
    pub event: EventRefPayload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    NotOrdinary,
    InvalidReference,
    ContextMismatch,
    SourceUnavailable,
    ConsumerUnavailable,
    Routing(RouteError),
}

/// Plan a verified, source-admitted ordinary event without changing its request.
///
/// Unrouted channels and the resolver's allowed kill-switch fallback return
/// `Legacy`, including older references without an activity digest. A routed
/// plan requires an unexpired reference that attests the exact incoming activity
/// and identities; only its activity digest and named-consumer fields change.
pub fn plan(
    request: &LaunchRequest,
    keys: &MasterKeyRing,
    now: i64,
    options: RoutingOptions<'_>,
) -> Result<OrdinaryPlan, PlanError> {
    if request.consumer.is_some()
        || request.activity.routing.is_some()
        || matches!(request.activity.kind, ActivityKind::Interaction(_))
    {
        return Err(PlanError::NotOrdinary);
    }
    let definition = &request.channel.definition;
    if definition.routing.is_none() {
        return Ok(OrdinaryPlan::Legacy);
    }
    let source = definition
        .source_of(&request.activity.source)
        .ok_or(PlanError::SourceUnavailable)?;
    if let (SourceKind::NyxidRelay { route_id, .. }, Source::NyxidRelay { route_ids, .. }) =
        (&request.activity.source, source)
    {
        if !route_ids.contains(route_id) {
            return Err(PlanError::SourceUnavailable);
        }
    }
    let input = RouteInput {
        activity: &request.activity,
        chat_source: Some(source),
        card_consumer: None,
        published: None,
        non_human_providers: options.non_human_providers,
        attributes_signed: options.attributes_signed,
    };
    let decision =
        match routing::resolve(definition, &input, options.enabled).map_err(PlanError::Routing)? {
            Resolution::Legacy => return Ok(OrdinaryPlan::Legacy),
            Resolution::Routed(decision) => decision,
        };
    let original = request
        .event_ref
        .open(keys, now)
        .map_err(|_| PlanError::InvalidReference)?
        .payload;
    if original.consumer.is_some() || original.lineage.is_some() {
        return Err(PlanError::NotOrdinary);
    }
    if original.channel_id != request.channel.id
        || original.record_id != request.channel.pointer.record_id
        || original.record_id != definition.record_id
        || original.conversation_id != request.conversation_id
        || !original.verifies_activity(&request.activity)
        || request.activity.validate().is_err()
    {
        return Err(PlanError::ContextMismatch);
    }
    let mut activity = request.activity.clone();
    activity.routing = Some(decision.routing);
    activity
        .validate()
        .map_err(|_| PlanError::ContextMismatch)?;
    let digest = activity_digest(&activity);
    let partition = compact_partition_key(&original.partition_key);
    let mut targets = Vec::new();
    for target in &activity.routing.as_ref().expect("routing was set").targets {
        let consumer = request
            .channel
            .consumer(Some(&target.id))
            .filter(|consumer| !consumer.pending && consumer.credential.is_some())
            .ok_or(PlanError::ConsumerUnavailable)?;
        let mut event = original.clone();
        event.activity_sha256 = Some(digest.clone());
        let idempotency_key = if consumer.id.is_default() {
            request.idempotency_key.clone()
        } else {
            event.consumer = Some(consumer.id.clone());
            event.partition_key = partition.clone();
            event.conversation_id = ConversationId::derive_for_consumer(
                keys,
                &request.channel.id,
                definition.partition.version,
                consumer.id,
                &partition,
            );
            IdempotencyKey::for_consumer(&activity.event_id, consumer.id)
        };
        targets.push(ConsumerLaunch {
            consumer: consumer.id.clone(),
            provider: consumer.provider.clone(),
            idempotency_key,
            event,
        });
    }
    Ok(OrdinaryPlan::Routed(Box::new(RoutedPlan {
        activity,
        source: source.clone(),
        targets,
        trace: decision.trace,
    })))
}

#[cfg(test)]
#[path = "routing_plan/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "routing_plan/fixture.rs"]
pub(crate) mod fixture;
