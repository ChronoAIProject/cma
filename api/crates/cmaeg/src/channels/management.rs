//! Management operations over sealed definitions and CMA records.
use super::definition::{
    build_definition, build_update, channel_pointer, validate_create_request,
    validate_update_request, ChannelCreateRequest, ChannelUpdateRequest,
};
pub use super::state::{CreatedChannel, LoadedChannel, ManagementError, ManagementState};
pub use super::test_turn::{test_turn, TestResult};
use crate::{
    auth::Creator,
    providers::{BindingRequest, ToolsAdvertisement},
    records::{RecordError, RecordValue},
};
use cmaeg_core::{
    channel::{ChannelDefinition, ConformanceLevel},
    ids::{BindingId, ChannelId, RecordId},
    profile::canonical_json,
    seal::{open_blob, seal_blob, ChannelPointer, MasterKeyRing, OpenError},
    ValidationError,
};
use uuid::Uuid;

pub async fn create(
    state: &ManagementState,
    creator: &Creator,
    request: ChannelCreateRequest,
) -> Result<CreatedChannel, ManagementError> {
    validate_create_request(&request).map_err(ManagementError::Invalid)?;
    if request.profile.metadata.owner.subject != creator.subject {
        return Err(ManagementError::Invalid(ValidationError::new(
            "/profile/metadata/owner/subject",
            "must match the authenticated NyxID subject",
        )));
    }
    if (request.profile.metadata.owner.kind == cmaeg_core::profile::NyxIdIdentityKind::Organization)
        != creator.organization
    {
        return Err(ManagementError::Invalid(ValidationError::new(
            "/profile/metadata/owner/kind",
            "must match the authenticated NyxID subject type",
        )));
    }
    if let Some(record_id) = request.record_id.clone() {
        return super::creation::create(state, creator, request, record_id).await;
    }
    let provider = request.provider.slug.clone();
    let record_id = RecordId::new(Uuid::new_v4().to_string()).map_err(ManagementError::Invalid)?;
    let binding_id = BindingId::new(format!("bnd_{}", Uuid::new_v4().simple()))
        .map_err(ManagementError::Invalid)?;
    let agent_key = request.credentials.agent_key.clone();
    let definition = build_definition(
        &request,
        &creator.subject,
        record_id.clone(),
        binding_id.clone(),
        ConformanceLevel::L1,
    )
    .map_err(ManagementError::Invalid)?;
    let pointer =
        channel_pointer(&state.cma_base_url, &record_id).map_err(ManagementError::Invalid)?;
    let channel_id = pointer
        .seal_id(&state.master_keys)
        .map_err(|_| ManagementError::Internal("cannot seal channel pointer".into()))?;
    seal_definition(&state.master_keys, &definition)?;
    let binding = state
        .providers
        .bind(
            &provider,
            &binding_id,
            &binding_request(state, &definition)?,
        )
        .await
        .map_err(ManagementError::Provider)?;
    let mut definition = definition;
    definition.provider.level = binding.level;
    let saved = async {
        let blob = seal_definition(&state.master_keys, &definition)?;
        state
            .records
            .put(&record_id, &creator.subject, &blob, None)
            .await
            .map_err(ManagementError::Record)
    }
    .await;
    let version = match saved {
        Ok(version) => version,
        Err(error) => {
            if state
                .providers
                .unbind(&provider, &binding_id, &agent_key)
                .await
                .is_err()
            {
                tracing::warn!(provider = %provider, "channel creation rollback could not unbind provider");
            }
            return Err(error);
        }
    };
    Ok(CreatedChannel {
        channel_id,
        record_id,
        definition,
        version,
        binding,
    })
}

pub async fn load(
    state: &ManagementState,
    creator: &Creator,
    channel_id: &ChannelId,
) -> Result<LoadedChannel, ManagementError> {
    let loaded = load_unowned(state, channel_id).await?;
    if loaded.definition.owner.subject != creator.subject {
        return Err(ManagementError::NotFound);
    }
    Ok(loaded)
}

pub async fn update(
    state: &ManagementState,
    creator: &Creator,
    channel_id: &ChannelId,
    request: ChannelUpdateRequest,
) -> Result<LoadedChannel, ManagementError> {
    validate_update_request(&request).map_err(ManagementError::Invalid)?;
    let _guard = state.mutation_guard(channel_id.as_str()).await;
    let current = load(state, creator, channel_id).await?;
    if current
        .definition
        .creation
        .as_ref()
        .is_some_and(|value| !value.ready)
    {
        return Err(ManagementError::Conflict {
            expected: request.expected_version,
            actual: Some(current.record.version),
        });
    }
    if current.record.version != request.expected_version {
        return Err(ManagementError::Conflict {
            expected: request.expected_version,
            actual: Some(current.record.version),
        });
    }
    // Provider identity belongs to the channel binding and cannot be migrated by an update.
    if request.provider.slug != current.definition.provider.slug {
        return Err(ManagementError::Invalid(ValidationError::new(
            "/provider/slug",
            "cannot change an existing channel provider",
        )));
    }
    state.resolver.invalidate(channel_id.as_str());
    let mut definition =
        build_update(&request, &current.definition).map_err(ManagementError::Invalid)?;
    definition.version = current.record.version;
    // Validate the complete definition and its storage bound before the provider sees a mutation.
    seal_definition(&state.master_keys, &definition)?;
    // A previous PUT may have updated the provider but failed to persist the
    // record. Even unchanged policy must reconcile the retained credentials;
    // comparing two copies of the record cannot establish provider state.
    let binding = state
        .providers
        .bind(
            &definition.provider.slug,
            &definition.provider.binding_id,
            &binding_request(state, &definition)?,
        )
        .await
        .map_err(ManagementError::Provider)?;
    definition.provider.level = binding.level;
    let blob = seal_definition(&state.master_keys, &definition)?;
    // A failed CAS must never unbind a retained channel. The provider and CMA record store
    // do not share a transaction; retry with a freshly read version to reconcile a rebind.
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
    state.resolver.invalidate(channel_id.as_str());
    Ok(LoadedChannel {
        channel_id: channel_id.clone(),
        pointer: current.pointer,
        definition,
        record: RecordValue { blob, version },
    })
}

pub async fn delete(
    state: &ManagementState,
    creator: &Creator,
    channel_id: &ChannelId,
    expected_version: u64,
) -> Result<(), ManagementError> {
    let _guard = state.mutation_guard(channel_id.as_str()).await;
    let current = load(state, creator, channel_id).await?;
    if current.record.version != expected_version {
        return Err(ManagementError::Conflict {
            expected: expected_version,
            actual: Some(current.record.version),
        });
    }
    state.resolver.invalidate(channel_id.as_str());
    let credential = current
        .definition
        .secrets
        .provider_credential
        .as_ref()
        .ok_or_else(|| {
            ManagementError::Internal("channel provider credential is unavailable".into())
        })?;
    state
        .providers
        .unbind(
            &current.definition.provider.slug,
            &current.definition.provider.binding_id,
            credential,
        )
        .await
        .map_err(ManagementError::Provider)?;
    match state
        .records
        .delete(&current.definition.record_id, expected_version)
        .await
    {
        Ok(true) | Ok(false) => {
            state.resolver.invalidate(channel_id.as_str());
            Ok(())
        }
        Err(RecordError::Conflict { expected, actual }) => {
            Err(ManagementError::Conflict { expected, actual })
        }
        Err(error) => Err(ManagementError::Record(error)),
    }
}

pub async fn rekey(state: &ManagementState, channel_id: &ChannelId) -> Result<u8, ManagementError> {
    let _guard = state.mutation_guard(channel_id.as_str()).await;
    let loaded = load_unowned(state, channel_id).await?;
    state.resolver.invalidate(channel_id.as_str());
    let mut definition = loaded.definition;
    definition.version = loaded.record.version;
    let blob = seal_definition(state.master_keys.as_ref(), &definition)?;
    state
        .records
        .put(
            &definition.record_id,
            &definition.owner.subject,
            &blob,
            Some(loaded.record.version),
        )
        .await
        .map_err(|error| match error {
            RecordError::Conflict { expected, actual } => {
                ManagementError::Conflict { expected, actual }
            }
            other => ManagementError::Record(other),
        })?;
    state.resolver.invalidate(channel_id.as_str());
    Ok(state.master_keys.current_kid())
}

async fn load_unowned(
    state: &ManagementState,
    channel_id: &ChannelId,
) -> Result<LoadedChannel, ManagementError> {
    let pointer = ChannelPointer::open_id(channel_id, state.master_keys.as_ref())
        .map_err(|error| match error {
            OpenError::Format | OpenError::WrongKind => ManagementError::Invalid(
                ValidationError::new("/channel_id", "malformed channel pointer"),
            ),
            _ => ManagementError::NotFound,
        })?
        .payload;
    if state
        .records
        .record_url(pointer.record_id.as_str())
        .map_err(ManagementError::Record)?
        .as_str()
        != pointer.record_url
    {
        return Err(ManagementError::NotFound);
    }
    let record = state
        .records
        .get(&pointer.record_url)
        .await
        .map_err(|error| match error {
            RecordError::NotFound => ManagementError::NotFound,
            other => ManagementError::Record(other),
        })?;
    let opened = open_blob(state.master_keys.as_ref(), &record.blob).map_err(|error| {
        ManagementError::Internal(format!("channel blob cannot be opened: {error}"))
    })?;
    let definition: ChannelDefinition =
        serde_json::from_slice(&opened.payload).map_err(|error| {
            ManagementError::Internal(format!("channel definition is malformed: {error}"))
        })?;
    if definition.record_id != pointer.record_id {
        return Err(ManagementError::NotFound);
    }
    definition
        .validate()
        .map_err(|_| ManagementError::Internal("retained channel is invalid".into()))?;
    Ok(LoadedChannel {
        channel_id: channel_id.clone(),
        pointer,
        definition,
        record,
    })
}

pub(super) fn seal_definition(
    ring: &MasterKeyRing,
    definition: &ChannelDefinition,
) -> Result<Vec<u8>, ManagementError> {
    let json = definition.to_blob_json();
    seal_blob(ring, canonical_json(json).as_bytes()).map_err(|error| match error {
        cmaeg_core::seal::SealError::TooLarge => ManagementError::Invalid(ValidationError::new(
            "/",
            "sealed channel exceeds the record size limit",
        )),
        _ => ManagementError::Internal("channel blob cannot be sealed".into()),
    })
}

pub(super) fn binding_request(
    state: &ManagementState,
    definition: &ChannelDefinition,
) -> Result<BindingRequest, ManagementError> {
    Ok(BindingRequest {
        schema_version: definition.schema_version,
        target: definition.provider.target.clone(),
        profile: definition.profile.clone(),
        profile_digest: definition.profile_digest.clone(),
        agent_key: definition
            .secrets
            .provider_credential
            .clone()
            .ok_or_else(|| ManagementError::Internal("missing provider credential".into()))?,
        tools: ToolsAdvertisement {
            native_mcp_url: format!("{}/mcp", state.public_url),
            nyxid_service_slug: state.tools_nyxid_slug.clone(),
        },
    })
}
