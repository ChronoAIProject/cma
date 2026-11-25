//! Policy-only routing edits. Provider identities and credentials are immutable here.
use super::management::{
    load, open_pointer, seal_definition, LoadedChannel, ManagementError, ManagementState,
};
use crate::{auth::Creator, records::RecordValue};
use cmaeg_core::{
    channel::{
        Addressing, AgentEventPolicy, Consumer, ConsumerReply, DefaultConsumer, DefaultReply,
        PublishPolicy, ReplyLabel, RoutingPolicy, RoutingRule,
    },
    ids::{ChannelId, ConsumerId, ConsumerName},
    ValidationError,
};
use serde::Deserialize;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingUpdateRequest {
    pub expected_version: u64,
    pub routing: RoutingPolicyRequest,
}
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingPolicyRequest {
    pub version: u16,
    pub default_consumer: DefaultConsumer,
    #[serde(default)]
    pub consumers: Vec<ConsumerPolicyRequest>,
    #[serde(default)]
    pub rules: Vec<RoutingRule>,
    pub default_targets: Vec<ConsumerId>,
    #[serde(default)]
    pub addressing: Addressing,
    #[serde(default = "default_max_targets")]
    pub max_targets: u8,
    #[serde(default)]
    pub reply_label: ReplyLabel,
    #[serde(default)]
    pub default_reply: Option<DefaultReply>,
    #[serde(default)]
    pub agent_events: AgentEventPolicy,
}
fn default_max_targets() -> u8 {
    4
}
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConsumerPolicyRequest {
    pub id: ConsumerId,
    pub name: ConsumerName,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default = "yes")]
    pub mentionable: bool,
    #[serde(default)]
    pub non_human: Option<bool>,
    #[serde(default)]
    pub reply: ConsumerReply,
    #[serde(default)]
    pub publish: PublishPolicy,
}
fn yes() -> bool {
    true
}

pub async fn update(
    state: &ManagementState,
    creator: &Creator,
    channel_id: &ChannelId,
    request: RoutingUpdateRequest,
) -> Result<LoadedChannel, ManagementError> {
    let pointer = open_pointer(state, channel_id)?;
    let _guard = state.mutation_guard(pointer.record_id.as_str()).await;
    let current = load(state, creator, channel_id).await?;
    super::creation::require_ready(&current.definition)?;
    super::mutation::available(&current.definition, current.record.version)?;
    if current.record.version != request.expected_version
        && request.expected_version.checked_add(1) != Some(current.record.version)
    {
        return Err(ManagementError::Conflict {
            expected: request.expected_version,
            actual: Some(current.record.version),
        });
    }
    let mut definition = current.definition.clone();
    let retained = definition
        .routing
        .as_ref()
        .map_or(&[][..], |routing| routing.consumers.as_slice());
    let mut policy = request.routing;
    let consumers = preserve_consumers(retained, std::mem::take(&mut policy.consumers))?;
    definition.routing = Some(RoutingPolicy {
        version: policy.version,
        default_consumer: policy.default_consumer,
        consumers,
        rules: policy.rules,
        default_targets: policy.default_targets,
        addressing: policy.addressing,
        max_targets: policy.max_targets,
        reply_label: policy.reply_label,
        default_reply: policy.default_reply,
        agent_events: policy.agent_events,
    });
    if current.record.version != request.expected_version {
        // A lost success may be retried with its original CAS. Confirm only
        // the immediately following revision and the exact intended policy.
        // This proves the requested state, never attribution to this request.
        if definition.routing == current.definition.routing {
            return Ok(current);
        }
        return Err(ManagementError::Conflict {
            expected: request.expected_version,
            actual: Some(current.record.version),
        });
    }
    super::definition::validate_definition(
        Some(&current.definition),
        &definition,
        state.write_flags(),
    )
    .map_err(ManagementError::Invalid)?;
    super::custom_execution::validate_providers(&state.providers, &definition)?;
    super::custom_execution::validate_media(state, Some(&current.definition), &definition)?;
    definition.version = current.record.version;
    let blob = seal_definition(&state.master_keys, &definition)?;
    let version = state
        .records
        .put(
            &definition.record_id,
            &creator.subject,
            &blob,
            Some(request.expected_version),
        )
        .await
        .map_err(ManagementError::Record)?;
    state
        .index
        .changed(&definition.record_id, &creator.subject, version);
    state.resolver.invalidate(channel_id.as_str());
    Ok(LoadedChannel {
        channel_id: channel_id.clone(),
        pointer: current.pointer,
        definition,
        record: RecordValue { blob, version },
    })
}

fn preserve_consumers(
    retained: &[Consumer],
    requested: Vec<ConsumerPolicyRequest>,
) -> Result<Vec<Consumer>, ManagementError> {
    // Exact ordered IDs prevent policy edits from provisioning, deleting or
    // retargeting a consumer and preserve the stored delivery order.
    if retained.len() != requested.len()
        || retained
            .iter()
            .zip(&requested)
            .any(|(old, new)| old.id != new.id)
    {
        return Err(ManagementError::Invalid(ValidationError::new(
            "/routing/consumers",
            "must retain the existing consumer ids and order",
        )));
    }
    Ok(retained
        .iter()
        .zip(requested)
        .map(|(old, policy)| Consumer {
            name: policy.name,
            label: policy.label,
            mentionable: policy.mentionable,
            non_human: policy.non_human,
            reply: policy.reply,
            publish: policy.publish,
            ..old.clone()
        })
        .collect())
}
