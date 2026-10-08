//! Provider binding and the single ready-CAS reconciliation for new consumers.
use super::{
    consumer_build,
    management::load,
    routing_policy::persist,
    state::{LoadedChannel, ManagementError, ManagementState},
    target::ChannelTarget,
};
use crate::{
    auth::Creator,
    providers::{BindingRequest, BindingResponse, ToolsAdvertisement},
    records::RecordError,
};
use cmaeg_core::{
    channel::{ChannelDefinition, Consumer, ConsumerView, Secret},
    ids::ConsumerId,
};

pub(super) async fn finish_add(
    state: &ManagementState,
    creator: &Creator,
    pending: LoadedChannel,
    id: &ConsumerId,
) -> Result<(LoadedChannel, BindingResponse), ManagementError> {
    let snapshot = pending
        .definition
        .routing
        .as_ref()
        .unwrap()
        .consumers
        .iter()
        .find(|consumer| &consumer.id == id)
        .expect("just stored consumer")
        .clone();
    let credential = credential(pending.definition.consumer(Some(id)).unwrap())?.clone();
    let target = ChannelTarget::Id(pending.channel_id.clone());
    let pointer = pending.pointer.clone();
    let binding = bind(state, &pending.definition, id, "bind").await?;
    let mut next = pending.definition.clone();
    ready(&mut next, id, binding.level)?;
    match persist(state, pending, next).await {
        Ok(loaded) => return Ok((loaded, binding)),
        Err(ManagementError::Record(RecordError::Conflict { .. })) => {}
        Err(error) => return Err(error),
    }
    let fresh = match load(state, creator, &target).await {
        Ok(fresh) => fresh,
        Err(ManagementError::NotFound) => {
            // Ownership hiding also returns NotFound. Only the trusted
            // record store's absence proves a new binding is orphaned.
            match state.records.get(&pointer.record_url).await {
                Err(RecordError::NotFound) => {
                    compensate(state, &pointer.record_id, &snapshot, &credential).await?;
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(record_id = %pointer.record_id, consumer_id = %id,
                        "cannot confirm record deletion after consumer bind");
                    return Err(ManagementError::Record(error));
                }
            }
            return Err(conflict());
        }
        Err(error) => return Err(error),
    };
    let Some(retained) = fresh.definition.consumer(Some(id)) else {
        compensate(state, &pointer.record_id, &snapshot, &credential).await?;
        return Err(conflict());
    };
    if !same_binding(retained, &snapshot, &credential) {
        return Err(conflict());
    }
    // A different replica may have changed routing or presentation while
    // bind ran. Only readiness and provider level belong to this completion.
    let mut next = fresh.definition.clone();
    ready(&mut next, id, binding.level)?;
    consumer_build::validate(state, &fresh.definition, &next, "")?;
    Ok((persist(state, fresh, next).await?, binding))
}

pub(super) async fn rebind(
    state: &ManagementState,
    current: LoadedChannel,
    mut next: ChannelDefinition,
    id: &ConsumerId,
) -> Result<(LoadedChannel, BindingResponse), ManagementError> {
    state.invalidate_record(
        &current.pointer.record_id,
        Some(current.channel_id.as_str()),
        super::state::InvalidationPath::Rest,
    );
    let binding = bind(state, &next, id, "rebind").await?;
    ready(&mut next, id, binding.level)?;
    // A rebind updates a retained identity. CAS failure must not unbind it;
    // the owner retries against the new version to reconcile provider state.
    Ok((persist(state, current, next).await?, binding))
}

async fn bind(
    state: &ManagementState,
    definition: &ChannelDefinition,
    id: &ConsumerId,
    operation: &'static str,
) -> Result<BindingResponse, ManagementError> {
    let consumer = definition
        .consumer(Some(id))
        .ok_or(ManagementError::NotFound)?;
    let request = BindingRequest {
        schema_version: definition.schema_version,
        target: consumer.provider.target.clone(),
        profile: consumer.profile.clone(),
        profile_digest: consumer.profile_digest.into(),
        agent_key: credential(consumer)?.clone(),
        tools: ToolsAdvertisement {
            native_mcp_url: format!("{}/mcp", state.public_url),
            nyxid_service_slug: state.tools_nyxid_slug.clone(),
        },
    };
    let result = state
        .providers
        .bind(
            &consumer.provider.slug,
            &consumer.provider.binding_id,
            &request,
        )
        .await;
    tracing::info!(record_id = %definition.record_id, consumer_id = %id,
        slug = %consumer.provider.slug, binding_id = %consumer.provider.binding_id,
        operation, ok = result.is_ok(), "consumer provider binding completed");
    result.map_err(ManagementError::Provider)
}

pub(super) async fn unbind(
    state: &ManagementState,
    definition: &ChannelDefinition,
    consumer: ConsumerView<'_>,
) -> Result<(), ManagementError> {
    let result = state
        .providers
        .unbind(
            &consumer.provider.slug,
            &consumer.provider.binding_id,
            credential(consumer)?,
        )
        .await;
    tracing::info!(record_id = %definition.record_id, consumer_id = %consumer.id,
        slug = %consumer.provider.slug, binding_id = %consumer.provider.binding_id,
        ok = result.is_ok(), "consumer provider unbind completed");
    result.map_err(ManagementError::Provider)
}

async fn compensate(
    state: &ManagementState,
    record: &cmaeg_core::ids::RecordId,
    snapshot: &Consumer,
    credential: &Secret,
) -> Result<(), ManagementError> {
    let result = state
        .providers
        .unbind(
            &snapshot.provider.slug,
            &snapshot.provider.binding_id,
            credential,
        )
        .await;
    if result.is_err() {
        tracing::error!(record_id = %record, consumer_id = %snapshot.id,
            slug = %snapshot.provider.slug, binding_id = %snapshot.provider.binding_id,
            "orphan consumer binding compensation failed");
    } else {
        tracing::info!(record_id = %record, consumer_id = %snapshot.id,
            slug = %snapshot.provider.slug, binding_id = %snapshot.provider.binding_id,
            "orphan consumer binding compensated");
    }
    result.map_err(ManagementError::Provider)
}

fn same_binding(view: ConsumerView<'_>, snapshot: &Consumer, credential: &Secret) -> bool {
    view.id == &snapshot.id
        && view.provider.slug == snapshot.provider.slug
        && view.provider.target == snapshot.provider.target
        && view.provider.binding_id == snapshot.provider.binding_id
        && view.profile == &snapshot.profile
        && view.profile_digest == snapshot.profile_digest
        && view.agent_key_hash == snapshot.agent_key_hash
        && view.credential == Some(credential)
}

fn ready(
    definition: &mut ChannelDefinition,
    id: &ConsumerId,
    level: cmaeg_core::channel::ConformanceLevel,
) -> Result<(), ManagementError> {
    let consumer = definition
        .routing
        .as_mut()
        .and_then(|routing| {
            routing
                .consumers
                .iter_mut()
                .find(|consumer| &consumer.id == id)
        })
        .ok_or(ManagementError::NotFound)?;
    consumer.pending = false;
    consumer.provider.level = level;
    Ok(())
}

fn credential(consumer: ConsumerView<'_>) -> Result<&Secret, ManagementError> {
    consumer
        .credential
        .ok_or_else(|| ManagementError::Internal("missing consumer credential".into()))
}

fn conflict() -> ManagementError {
    ManagementError::ConflictReason {
        detail: "consumer_changed_during_bind",
        pointer: None,
    }
}
