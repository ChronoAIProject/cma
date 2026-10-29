//! Management operations over sealed definitions and CMA records.
use super::definition::{
    build_definition, build_update, channel_pointer, validate_create_request,
    validate_update_request, ChannelCreateRequest, ChannelUpdateRequest, SourceRequest,
};
pub use super::lifecycle::{delete, rekey};
pub use super::state::{
    CreatedChannel, LoadedChannel, ManagementError, ManagementState, RecordedChannel,
};
pub use super::test_turn::{test_consumer, test_turn, TestResult};
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
    mut request: ChannelCreateRequest,
) -> Result<CreatedChannel, ManagementError> {
    custom_attachments_allowed(state, &request.sources)?;
    crate::connections::binding::expand_create(state, creator, &mut request).await?;
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
    if request.sources.iter().any(|source| {
        matches!(
            source,
            SourceRequest::NativeLark(_)
                | SourceRequest::Discord { .. }
                | SourceRequest::Whatsapp { .. }
        )
    }) && !request.record_id.as_ref().is_some_and(|record| {
        uuid::Uuid::parse_str(record.as_str())
            .is_ok_and(|id| id.get_version_num() == 4 && id.to_string() == record.as_str())
    }) {
        return Err(ManagementError::Invalid(ValidationError::new("/record_id", "native Lark, Discord or WhatsApp creation requires a stable canonical UUID v4 record_id for recoverable activation")));
    }
    if let Some(record_id) = request.record_id.clone() {
        return super::creation::create(state, creator, request, record_id).await;
    }
    let provider = request.provider.slug.clone();
    let record_id = RecordId::new(Uuid::new_v4().to_string()).map_err(ManagementError::Invalid)?;
    let binding_id = BindingId::new(format!("bnd_{}", Uuid::new_v4().simple()))
        .map_err(ManagementError::Invalid)?;
    let agent_key = request.credentials.agent_key.clone();
    let mut definition = build_definition(
        &request,
        &creator.subject,
        record_id.clone(),
        binding_id.clone(),
        ConformanceLevel::L1,
        state.write_flags(),
    )
    .map_err(ManagementError::Invalid)?;
    super::custom_execution::validate_providers(&state.providers, &definition)?;
    super::custom_execution::validate_media(state, None, &definition)?;
    super::native_telegram::initialize(state, None, &mut definition)?;
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
        Ok(version) => {
            state.index.changed(&record_id, &creator.subject, version);
            version
        }
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

/// Refuse a custom source's attachments setting while inbound media is off.
///
/// An image older than this setting cannot parse a stored definition that
/// carries it, so it is accepted only once the operator has enabled media on
/// every replica. Removing the setting is always allowed. Origins that are IP
/// literals must be public; a DNS name is checked against public addresses on
/// every later fetch, so only the obviously private case is refused here.
pub(super) fn custom_attachments_allowed(
    state: &ManagementState,
    sources: &[SourceRequest],
) -> Result<(), ManagementError> {
    for (index, source) in sources.iter().enumerate() {
        let SourceRequest::Custom {
            attachments: Some(config),
            ..
        } = source
        else {
            continue;
        };
        let pointer = format!("/sources/{index}/attachments");
        if !state.media_enabled {
            tracing::info!(
                source = index,
                "custom attachments refused while media is disabled"
            );
            return Err(ManagementError::Invalid(ValidationError::new(
                pointer,
                "custom_attachments_disabled",
            )));
        }
        for (position, origin) in config.origins.iter().enumerate() {
            // A malformed origin is left to the structural validation of the
            // complete definition, which reports its own exact pointer.
            let address = match url::Url::parse(origin)
                .ok()
                .and_then(|url| url.host().map(|host| host.to_owned()))
            {
                Some(url::Host::Ipv4(address)) => Some(std::net::IpAddr::V4(address)),
                Some(url::Host::Ipv6(address)) => Some(std::net::IpAddr::V6(address)),
                _ => None,
            };
            if address.is_some_and(|address| !crate::net::policy::public_ip(address)) {
                return Err(ManagementError::Invalid(ValidationError::new(
                    format!("{pointer}/origins/{position}"),
                    "attachment_origin_not_public",
                )));
            }
        }
    }
    Ok(())
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

/// The channel `record_id` stores, read for `creator` only: the
/// record-keyed counterpart of [`load`], with the same owner check, for the
/// console's summaries. Another owner's record is `NotFound`. A definition
/// that opens but fails validation is returned with `valid: false` so the
/// console can name it; nothing may act on such a definition.
pub async fn load_by_record(
    state: &ManagementState,
    creator: &Creator,
    record_id: &RecordId,
) -> Result<RecordedChannel, ManagementError> {
    let pointer =
        channel_pointer(&state.cma_base_url, record_id).map_err(ManagementError::Invalid)?;
    let record = state
        .records
        .get(&pointer.record_url)
        .await
        .map_err(|error| match error {
            RecordError::NotFound => ManagementError::NotFound,
            other => ManagementError::Record(other),
        })?;
    let opened = open_blob(state.master_keys.as_ref(), &record.blob)
        .map_err(|_| ManagementError::Internal("channel blob cannot be opened".into()))?;
    let definition: ChannelDefinition = serde_json::from_slice(&opened.payload)
        .map_err(|_| ManagementError::Internal("channel definition cannot be read".into()))?;
    if definition.record_id != *record_id || definition.owner.subject != creator.subject {
        return Err(ManagementError::NotFound);
    }
    let valid = definition.validate().is_ok();
    Ok(RecordedChannel {
        definition,
        version: record.version,
        record_bytes: record.blob.len(),
        valid,
    })
}

pub async fn update(
    state: &ManagementState,
    creator: &Creator,
    channel_id: &ChannelId,
    mut request: ChannelUpdateRequest,
) -> Result<LoadedChannel, ManagementError> {
    let pointer = open_pointer(state, channel_id)?;
    let _guard = state.mutation_guard(pointer.record_id.as_str()).await;
    let current = load(state, creator, channel_id).await?;
    crate::connections::binding::expand_update(state, creator, &current.definition, &mut request)
        .await?;
    validate_update_request(&request).map_err(ManagementError::Invalid)?;
    custom_attachments_allowed(state, &request.sources)?;
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
    // The write gate runs inside the build, before the provider sees anything.
    let mut definition = build_update(&request, &current.definition, state.write_flags())
        .map_err(ManagementError::Invalid)?;
    super::native_telegram::initialize(state, Some(&current.definition), &mut definition)?;
    super::custom_execution::validate_providers(&state.providers, &definition)?;
    super::custom_execution::validate_media(state, Some(&current.definition), &definition)?;
    definition.version = current.record.version;
    if definition.routing.is_none()
        || definition.secrets.whatsapp != current.definition.secrets.whatsapp
        || !definition
            .sources
            .iter()
            .filter(|source| matches!(source, cmaeg_core::channel::Source::Whatsapp { .. }))
            .eq(current
                .definition
                .sources
                .iter()
                .filter(|source| matches!(source, cmaeg_core::channel::Source::Whatsapp { .. })))
    {
        crate::whatsapp::management::initialize(state, &mut definition)?;
    }
    crate::native_lark::management::initialize(state, Some(&current.definition), &mut definition)?;
    crate::native_lark::management::verify(state, Some(&current.definition), &definition).await?;
    crate::whatsapp::management::verify(state, Some(&current.definition), &definition).await?;
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

pub(super) fn open_pointer(
    state: &ManagementState,
    channel_id: &ChannelId,
) -> Result<ChannelPointer, ManagementError> {
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
    Ok(pointer)
}

pub(super) async fn load_unowned(
    state: &ManagementState,
    channel_id: &ChannelId,
) -> Result<LoadedChannel, ManagementError> {
    let pointer = open_pointer(state, channel_id)?;
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
            native_mcp_url: format!("{}/mcp", state.advertised_api_url()),
            nyxid_service_slug: state.tools_nyxid_slug.clone(),
        },
    })
}
