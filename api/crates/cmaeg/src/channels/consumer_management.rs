//! Versioned owner operations for additional and default consumers.
use super::{
    consumer_binding, consumer_build,
    consumer_requests::{ConsumerCreateRequest, ConsumerUpdateRequest},
    management::{load, seal_definition},
    mutation_metrics::{observe, Operation},
    routing_policy::{check_version, persist},
    state::{LoadedChannel, ManagementError, ManagementState},
    target::{open_target, ChannelTarget},
    write_gate,
};
use crate::{auth::Creator, providers::BindingResponse};
use cmaeg_core::{channel::ChannelDefinition, ids::ConsumerId};

#[derive(Debug)]
pub struct ManagedConsumer {
    pub channel: LoadedChannel,
    pub consumer_id: ConsumerId,
    pub binding: Option<BindingResponse>,
}

pub async fn add(
    state: &ManagementState,
    creator: &Creator,
    target: &ChannelTarget,
    request: ConsumerCreateRequest,
) -> Result<ManagedConsumer, ManagementError> {
    observe(Operation::Add, async {
        let pointer = open_target(state, target)?;
        let _guard = state.mutation_guard(pointer.record_id.as_str()).await;
        let current = load(state, creator, target).await?;
        check_version(&current, request.expected_version)?;
        enabled(state, &current.definition)?;
        let (next, id) = consumer_build::add(state, &current.definition, request)?;
        let pending = persist(state, current, next).await?;
        let (channel, binding) = consumer_binding::finish_add(state, creator, pending, &id).await?;
        Ok(ManagedConsumer {
            channel,
            consumer_id: id,
            binding: Some(binding),
        })
    })
    .await
}

pub async fn update(
    state: &ManagementState,
    creator: &Creator,
    target: &ChannelTarget,
    id: &ConsumerId,
    request: ConsumerUpdateRequest,
) -> Result<ManagedConsumer, ManagementError> {
    observe(Operation::Update, async {
        let pointer = open_target(state, target)?;
        let _guard = state.mutation_guard(pointer.record_id.as_str()).await;
        let current = load(state, creator, target).await?;
        check_version(&current, request.expected_version)?;
        enabled(state, &current.definition)?;
        let next = consumer_build::update(state, &current.definition, id, request)?;
        let (channel, binding) = if id.is_default() {
            (persist(state, current, next).await?, None)
        } else {
            let (channel, binding) = consumer_binding::rebind(state, current, next, id).await?;
            (channel, Some(binding))
        };
        Ok(ManagedConsumer {
            channel,
            consumer_id: id.clone(),
            binding,
        })
    })
    .await
}

pub async fn remove(
    state: &ManagementState,
    creator: &Creator,
    target: &ChannelTarget,
    id: &ConsumerId,
    expected_version: u64,
) -> Result<LoadedChannel, ManagementError> {
    observe(Operation::Remove, async {
        let pointer = open_target(state, target)?;
        let _guard = state.mutation_guard(pointer.record_id.as_str()).await;
        let current = load(state, creator, target).await?;
        check_version(&current, expected_version)?;
        if id.is_default() {
            return Err(ManagementError::InvalidReason("default_consumer_required"));
        }
        let consumer = current
            .definition
            .consumer(Some(id))
            .ok_or(ManagementError::NotFound)?;
        if let Some(pointer) = reference(&current.definition, id) {
            return Err(ManagementError::ConflictReason {
                detail: "consumer_in_use",
                pointer: Some(pointer),
            });
        }
        let mut next = current.definition.clone();
        next.routing
            .as_mut()
            .unwrap()
            .consumers
            .retain(|consumer| &consumer.id != id);
        next.secrets
            .consumer_keys
            .retain(|(consumer, _)| consumer != id);
        next.validate().map_err(ManagementError::Invalid)?;
        write_gate::check(Some(&current.definition), &next, state.write_flags())
            .map_err(ManagementError::Invalid)?;
        seal_definition(&state.master_keys, &next)?;
        consumer_binding::unbind(state, &current.definition, consumer).await?;
        persist(state, current, next).await
    })
    .await
}

fn enabled(state: &ManagementState, definition: &ChannelDefinition) -> Result<(), ManagementError> {
    if !state.routing.multi_consumer_enabled {
        return Err(ManagementError::InvalidReason(
            write_gate::MULTI_CONSUMER_DISABLED,
        ));
    }
    if definition.routing.is_none() {
        return Err(ManagementError::InvalidReason("routing_required"));
    }
    Ok(())
}

fn reference(definition: &ChannelDefinition, id: &ConsumerId) -> Option<String> {
    let routing = definition.routing.as_ref()?;
    let position = |targets: &[ConsumerId], prefix: String| {
        targets
            .iter()
            .position(|target| target == id)
            .map(|index| format!("{prefix}/{index}"))
    };
    if let Some(pointer) = position(&routing.default_targets, "/routing/default_targets".into()) {
        return Some(pointer);
    }
    for (index, rule) in routing.rules.iter().enumerate() {
        if let Some(pointer) = position(&rule.targets, format!("/routing/rules/{index}/targets")) {
            return Some(pointer);
        }
    }
    if let Some(pointer) = position(
        &routing.default_consumer.publish.targets,
        "/routing/default_consumer/publish/targets".into(),
    ) {
        return Some(pointer);
    }
    for (index, consumer) in routing
        .consumers
        .iter()
        .enumerate()
        .filter(|(_, consumer)| &consumer.id != id)
    {
        if let Some(pointer) = position(
            &consumer.publish.targets,
            format!("/routing/consumers/{index}/publish/targets"),
        ) {
            return Some(pointer);
        }
    }
    None
}
