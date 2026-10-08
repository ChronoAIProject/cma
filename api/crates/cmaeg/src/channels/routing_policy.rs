//! Owner-managed routing policy and its always-available removal path.
use super::{
    management::{load, seal_definition},
    state::{InvalidationPath, LoadedChannel, ManagementError, ManagementState},
    target::{open_target, ChannelTarget},
    write_gate,
};
use crate::{auth::Creator, records::RecordValue};
use cmaeg_core::{
    channel::{
        Addressing, AgentEventPolicy, ChannelDefinition, DefaultConsumer, DefaultReply, ReplyLabel,
        RoutingPolicy, RoutingRule, Source, ROUTING_POLICY_VERSION,
    },
    ids::ConsumerId,
    ValidationError,
};
use serde::Deserialize;
use utoipa::ToSchema;

/// Full replacement of routing policy; consumer bindings are managed separately.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingPutRequest {
    pub expected_version: u64,
    pub default_consumer: DefaultConsumer,
    pub default_targets: Vec<ConsumerId>,
    #[serde(default)]
    pub rules: Vec<RoutingRule>,
    #[serde(default)]
    pub addressing: Addressing,
    /// Defaults to 4 and cannot exceed the gateway's routed channel turn cap.
    #[serde(default)]
    pub max_targets: Option<u8>,
    #[serde(default)]
    pub reply_label: ReplyLabel,
    #[serde(default)]
    pub default_reply: Option<DefaultReply>,
    #[serde(default)]
    pub agent_events: AgentEventPolicy,
}

pub async fn put(
    state: &ManagementState,
    creator: &Creator,
    target: &ChannelTarget,
    request: RoutingPutRequest,
) -> Result<LoadedChannel, ManagementError> {
    let pointer = open_target(state, target)?;
    let _guard = state.mutation_guard(pointer.record_id.as_str()).await;
    let current = load(state, creator, target).await?;
    check_version(&current, request.expected_version)?;
    if !state.routing.multi_consumer_enabled {
        return Err(ManagementError::InvalidReason(
            write_gate::MULTI_CONSUMER_DISABLED,
        ));
    }
    let mut next = current.definition.clone();
    next.routing = Some(RoutingPolicy {
        version: ROUTING_POLICY_VERSION,
        consumers: next
            .routing
            .take()
            .map_or_else(Vec::new, |policy| policy.consumers),
        default_consumer: request.default_consumer,
        default_targets: request.default_targets,
        rules: request.rules,
        addressing: request.addressing,
        max_targets: request.max_targets.unwrap_or(4),
        reply_label: request.reply_label,
        default_reply: request.default_reply,
        agent_events: request.agent_events,
    });
    next.validate().map_err(request_error)?;
    if u32::from(next.routing.as_ref().unwrap().max_targets) > state.routed_capacity.per_channel {
        return Err(ManagementError::Invalid(ValidationError::new(
            "/max_targets",
            format!(
                "max_targets exceeds the routed channel turn cap ({})",
                state.routed_capacity.per_channel
            ),
        )));
    }
    write_gate::check(Some(&current.definition), &next, state.write_flags())
        .map_err(ManagementError::Invalid)?;
    persist(state, current, next).await
}

pub async fn remove(
    state: &ManagementState,
    creator: &Creator,
    target: &ChannelTarget,
    expected_version: u64,
    cascade: bool,
) -> Result<LoadedChannel, ManagementError> {
    let pointer = open_target(state, target)?;
    let _guard = state.mutation_guard(pointer.record_id.as_str()).await;
    let current = load(state, creator, target).await?;
    check_version(&current, expected_version)?;
    if !cascade && current.definition.consumers().nth(1).is_some() {
        return Err(ManagementError::ConflictReason {
            detail: "consumers_present",
            pointer: None,
        });
    }
    let mut next = current.definition.clone();
    next.routing = None;
    next.secrets.consumer_keys.clear();
    if current.definition.routing.is_some()
        && !next
            .sources
            .iter()
            .any(|source| matches!(source, Source::Custom { .. }))
    {
        next.partition.custom_key = None;
    }
    next.validate().map_err(ManagementError::Invalid)?;
    write_gate::check(Some(&current.definition), &next, state.write_flags())
        .map_err(ManagementError::Invalid)?;
    // Complete every local check before an unbind; failed unbinds keep the
    // record unchanged, and retry treats already-removed bindings as done.
    seal_definition(&state.master_keys, &next)?;
    for consumer in current.definition.consumers().skip(1) {
        let credential = consumer
            .credential
            .ok_or_else(|| ManagementError::Internal("missing consumer credential".into()))?;
        state
            .providers
            .unbind(
                &consumer.provider.slug,
                &consumer.provider.binding_id,
                credential,
            )
            .await
            .map_err(ManagementError::Provider)?;
    }
    persist(state, current, next).await
}

pub(super) fn check_version(current: &LoadedChannel, expected: u64) -> Result<(), ManagementError> {
    if current.record.version != expected {
        return Err(ManagementError::Conflict {
            expected,
            actual: Some(current.record.version),
        });
    }
    Ok(())
}

/// Store a validated definition and publish its new record head after CAS.
pub(super) async fn persist(
    state: &ManagementState,
    current: LoadedChannel,
    mut next: ChannelDefinition,
) -> Result<LoadedChannel, ManagementError> {
    next.version = current.record.version;
    let blob = seal_definition(&state.master_keys, &next)?;
    let version = state
        .records
        .put(
            &next.record_id,
            &next.owner.subject,
            &blob,
            Some(current.record.version),
        )
        .await
        .map_err(ManagementError::Record)?;
    state.invalidate_record(
        &next.record_id,
        Some(current.channel_id.as_str()),
        InvalidationPath::Rest,
    );
    state
        .index
        .changed(&next.record_id, &next.owner.subject, version);
    Ok(LoadedChannel {
        channel_id: current.channel_id,
        pointer: current.pointer,
        definition: next,
        record: RecordValue { blob, version },
    })
}

fn request_error(mut error: ValidationError) -> ManagementError {
    if error.pointer == "/routing" {
        error.pointer = "/".into();
    } else if let Some(pointer) = error.pointer.strip_prefix("/routing/") {
        error.pointer = format!("/{pointer}");
    }
    ManagementError::Invalid(error)
}
