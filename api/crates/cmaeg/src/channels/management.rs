//! Management operations over sealed definitions and CMA records.
use super::definition::{
    build_definition, build_update, channel_pointer, validate_create_request,
    validate_update_request, ChannelCreateRequest, ChannelUpdateRequest, SourceRequest,
};
pub(crate) use super::inspection::inspect;
pub use super::inspection::{load, load_by_record};
pub(crate) use super::lifecycle::retire_unready;
pub use super::lifecycle::{delete, delete_legacy, rekey};
pub use super::state::{
    CreatedChannel, LoadedChannel, ManagementError, ManagementState, RecordedChannel,
};
pub use super::test_turn::{test_consumer, test_turn, TestResult};
use crate::{
    auth::Creator,
    providers::{BindingRequest, ProviderError, ToolsAdvertisement},
    records::RecordValue,
};
use cmaeg_core::{
    channel::{ChannelDefinition, ConformanceLevel},
    ids::{BindingId, ChannelId, RecordId},
    profile::canonical_json,
    seal::{seal_blob, ChannelPointer, MasterKeyRing, OpenError},
    ValidationError,
};
use uuid::Uuid;

pub async fn create(
    state: &ManagementState,
    creator: &Creator,
    mut request: ChannelCreateRequest,
) -> Result<CreatedChannel, ManagementError> {
    custom_attachments_allowed(state, &request.sources)?;
    let claim = crate::connections::binding::expand_create(state, creator, &mut request).await?;
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
    if (state.records.channel_mutation_custody() || state.records.channel_creates_fenced())
        && request.record_id.is_none()
    {
        request.record_id =
            Some(RecordId::new(Uuid::new_v4().to_string()).map_err(ManagementError::Invalid)?);
    }
    if let Some(record_id) = request.record_id.clone() {
        return super::creation::create(state, creator, request, record_id, claim.as_ref()).await;
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
    super::github_sources::validate(state, None, &definition).await?;
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
    definition.provider.capabilities = binding.capabilities.clone();
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
    super::creation::require_ready(&current.definition)?;
    if super::mutation::original_version(&current.definition, current.record.version)
        != request.expected_version
    {
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
    // The write gate runs inside the build, before the provider sees anything.
    let mut definition = build_update(&request, &current.definition, state.write_flags())
        .map_err(ManagementError::Invalid)?;
    if current.definition.mutation.is_none()
        && current.definition.provider.target.is_profile()
        && state
            .providers
            .has_cma_binding_contract(&current.definition.provider.slug)
    {
        let retained = &current.definition.profile;
        let requested = &definition.profile;
        let adds_service = requested
            .permission
            .service_scope
            .iter()
            .chain(std::iter::once(&requested.model.service))
            .any(|service| {
                service != &retained.model.service
                    && !retained.permission.service_scope.contains(service)
            });
        if adds_service {
            return Err(ManagementError::Provider(ProviderError::Rejected {
                status: 409,
                code: "binding_reconfiguration_unsupported".into(),
            }));
        }
    }
    state.resolver.invalidate(channel_id.as_str());
    definition.mutation = None;
    let fingerprint = super::mutation::fingerprint(&definition);
    super::native_telegram::initialize(state, Some(&current.definition), &mut definition)?;
    super::github_sources::validate(state, Some(&current.definition), &definition).await?;
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
    let mut reservation = super::mutation::Reservation::acquire(
        &state.records,
        &state.master_keys,
        &current.definition,
        current.record.version,
        super::mutation::Intent {
            operation: cmaeg_core::channel::ChannelMutationKind::Update,
            expected_version: request.expected_version,
            fingerprint,
            desired: &definition,
        },
    )
    .await?;
    definition = reservation.desired.clone();
    let step = format!(
        "bind:{}:{}",
        definition.provider.slug, definition.provider.binding_id
    );
    if !reservation.done(&step) {
        let provider_request = binding_request(state, &definition)?;
        reservation
            .effect(&state.records, &state.master_keys, step)
            .await?;
        let binding = match state
            .providers
            .bind_custodied(
                &definition.provider.slug,
                &definition.provider.binding_id,
                &provider_request,
            )
            .await
        {
            Ok(binding) => binding,
            Err(error) => {
                if !error.unconfirmed {
                    reservation
                        .retryable(&state.records, &state.master_keys)
                        .await?;
                }
                return Err(ManagementError::Provider(error.error));
            }
        };
        definition.provider.level = binding.level;
        definition.provider.capabilities = binding.capabilities;
        reservation
            .confirmed(&state.records, &state.master_keys, definition.clone())
            .await?;
    }
    let (blob, version) = reservation
        .publish(&state.records, &state.master_keys, &definition)
        .await?;
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
    state
        .records
        .record_url(pointer.record_id.as_str())
        .map_err(ManagementError::Record)?;
    if !state
        .records
        .names_record(&pointer.record_url, pointer.record_id.as_str())
    {
        return Err(ManagementError::NotFound);
    }
    Ok(pointer)
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
