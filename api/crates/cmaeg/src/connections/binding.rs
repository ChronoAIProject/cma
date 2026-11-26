//! Resolve opaque connection references and own their platform webhook lifecycle.
use super::{model::*, recovery, webhook, Error};
use crate::{
    auth::Creator,
    channels::{
        management::{ManagementError, ManagementState},
        ChannelCreateRequest, ChannelUpdateRequest, SourceRequest,
    },
    records::CreateConnectionClaim,
};
use cmaeg_core::{
    channel::{AdmissionPolicy, ChannelDefinition, Secret, Source},
    ids::ChannelId,
    ValidationError,
};
use webhook::store;

fn failed() -> ManagementError {
    ManagementError::Internal("connection platform operation unavailable".into())
}
fn error(value: Error) -> ManagementError {
    match value {
        Error::NotFound => ManagementError::NotFound,
        Error::Conflict => ManagementError::Conflict {
            expected: 0,
            actual: None,
        },
        // Only connection creation refuses by name; a bound connection read
        // never does, so the refusal is reported as an invalid reference.
        Error::Invalid(message) | Error::Forbidden(message) => invalid(message),
        Error::Unavailable => failed(),
        Error::RateLimited(seconds) => {
            ManagementError::Provider(crate::providers::ProviderError::Busy {
                retry_after: Some(std::time::Duration::from_secs(seconds)),
            })
        }
    }
}
fn invalid(message: &str) -> ManagementError {
    ManagementError::Invalid(ValidationError::new("/connection_ref", message))
}
async fn owned(
    state: &ManagementState,
    creator: &Creator,
    id: &str,
) -> Result<(Connection, u64), ManagementError> {
    let (record, version) = store(state).connection(id).await.map_err(error)?;
    if record.owner != creator.subject || record.organization != creator.organization {
        return Err(ManagementError::NotFound);
    }
    if record.status != Status::Ready || record.webhook_retiring {
        return Err(invalid("connection is not ready"));
    }
    Ok((record, version))
}
fn source(
    material: &Material,
    admission: Option<AdmissionPolicy>,
) -> Result<SourceRequest, ManagementError> {
    Ok(SourceRequest::Telegram {
        topic_lifecycle: false,
        topic_creation: false,
        message_events: false,
        channel_posts: false,
        member_join_chat_ids: None,
        bot_id: material.bot_id.clone(),
        bot_username: Some(material.username.clone()),
        admission: admission.ok_or_else(|| invalid("connection_admission is required"))?,
    })
}
pub(crate) async fn expand_create(
    state: &ManagementState,
    creator: &Creator,
    request: &mut ChannelCreateRequest,
) -> Result<Option<CreateConnectionClaim>, ManagementError> {
    let Some(id) = request.connection_ref.clone() else {
        return Ok(None);
    };
    if !request.sources.is_empty()
        || request.credentials.telegram_bot_token.is_some()
        || request.credentials.discord_bot_token.is_some()
        || request.credentials.telegram_webhook_secret.is_some()
    {
        return Err(invalid(
            "connection_ref replaces platform sources and credentials",
        ));
    }
    let record_id = request
        .record_id
        .as_ref()
        .ok_or_else(|| invalid("record_id is required with connection_ref"))?;
    let (mut connection, version) = owned(state, creator, &id).await?;
    if connection
        .channel_record
        .as_deref()
        .is_some_and(|current| current != record_id.as_str())
    {
        return Err(invalid("connection is already bound to another channel"));
    }
    if connection.channel_record.is_none() && !state.managed_telegram_setup_enabled {
        return Err(invalid("managed_telegram_setup_disabled"));
    }
    let material = connection.material.as_ref().ok_or_else(failed)?;
    request.sources = vec![source(material, request.connection_admission.clone())?];
    request.credentials.telegram_bot_token = Some(Secret::new(material.token.clone()));
    request.credentials.telegram_webhook_secret =
        Some(Secret::new(material.webhook_secret.clone()));
    crate::channels::definition::validate_create_request(request)
        .map_err(ManagementError::Invalid)?;
    if request.profile.metadata.owner.subject != creator.subject
        || (request.profile.metadata.owner.kind
            == cmaeg_core::profile::NyxIdIdentityKind::Organization)
            != creator.organization
    {
        return Err(invalid("profile owner must match authenticated owner"));
    }
    if connection.channel_record.is_none() {
        connection.channel_record = Some(record_id.to_string());
        if state.records.channel_creates_fenced() {
            return store(state)
                .prepare_create_claim(connection, version)
                .map(Some)
                .map_err(error);
        }
        store(state)
            .save_connection(&connection, Some(version))
            .await
            .map_err(error)?;
    }
    Ok(None)
}
pub async fn expand_update(
    state: &ManagementState,
    creator: &Creator,
    current: &ChannelDefinition,
    request: &mut ChannelUpdateRequest,
) -> Result<(), ManagementError> {
    if request.connection_ref != current.connection_ref {
        return Err(invalid("channel connection is immutable"));
    }
    let Some(id) = request.connection_ref.as_deref() else {
        return Ok(());
    };
    if request.credentials.as_ref().is_some_and(|patch| {
        patch.telegram_bot_token.is_some()
            || patch.discord_bot_token.is_some()
            || patch.telegram_webhook_secret.is_some()
    }) {
        return Err(invalid("connection credentials are managed by the gateway"));
    }
    let (record, _) = owned(state, creator, id).await?;
    if record.channel_record.as_deref() != Some(current.record_id.as_str()) {
        return Err(ManagementError::NotFound);
    }
    request.sources = vec![updated_source(
        record.material.as_ref().ok_or_else(failed)?,
        current,
        request,
    )?];
    Ok(())
}

/// A connection owns bot identity and credentials; the owner controls consent.
fn updated_source(
    material: &Material,
    current: &ChannelDefinition,
    request: &ChannelUpdateRequest,
) -> Result<SourceRequest, ManagementError> {
    let [Source::Telegram {
        bot_id,
        topic_lifecycle,
        topic_creation,
        message_events,
        channel_posts,
        admission,
        ..
    }] = current.sources.as_slice()
    else {
        return Err(invalid(
            "managed connection requires its original Telegram source",
        ));
    };
    if bot_id != &material.bot_id {
        return Err(invalid("managed connection bot identity changed"));
    }
    let (
        topic_lifecycle,
        topic_creation,
        message_events,
        channel_posts,
        member_join_chat_ids,
        admission,
    ) = match request.sources.as_slice() {
        [] => (
            *topic_lifecycle,
            *topic_creation,
            *message_events,
            *channel_posts,
            None,
            admission.clone(),
        ),
        [SourceRequest::Telegram {
            bot_id,
            bot_username,
            topic_lifecycle,
            topic_creation,
            message_events,
            channel_posts,
            member_join_chat_ids,
            admission,
        }] if bot_id == &material.bot_id
            && bot_username
                .as_ref()
                .is_none_or(|name| name == &material.username) =>
        {
            (
                *topic_lifecycle,
                *topic_creation,
                *message_events,
                *channel_posts,
                member_join_chat_ids.clone(),
                admission.clone(),
            )
        }
        _ => {
            return Err(invalid(
                "managed connection accepts only its original Telegram source",
            ))
        }
    };
    Ok(SourceRequest::Telegram {
        topic_lifecycle,
        topic_creation,
        message_events,
        channel_posts,
        member_join_chat_ids,
        bot_id: material.bot_id.clone(),
        bot_username: Some(material.username.clone()),
        admission: request.connection_admission.clone().unwrap_or(admission),
    })
}
/// Called before the durable channel's creation.ready flag can become true.
/// Compatibility entry point for callers that do not need custody metadata.
#[allow(dead_code)]
pub async fn activate(
    state: &ManagementState,
    definition: &ChannelDefinition,
    channel_id: &ChannelId,
) -> Result<(), ManagementError> {
    activate_custodied(state, definition, channel_id)
        .await
        .map_err(|value| value.error)
}
pub(crate) async fn activate_custodied(
    state: &ManagementState,
    definition: &ChannelDefinition,
    channel_id: &ChannelId,
) -> Result<(), CustodyError> {
    let Some(id) = definition.connection_ref.as_deref() else {
        return Ok(());
    };
    let (mut record, version) = store(state).connection(id).await.map_err(error)?;
    if (record.organization
        != (definition.owner.kind == cmaeg_core::profile::NyxIdIdentityKind::Organization))
        || record.owner != definition.owner.subject
        || record.channel_record.as_deref() != Some(definition.record_id.as_str())
        || record.status != Status::Ready
    {
        return Err(ManagementError::NotFound.into());
    }
    if record
        .channel_id
        .as_deref()
        .is_some_and(|old| old != channel_id.as_str())
    {
        return Err(invalid("connection channel identity changed").into());
    }
    record.channel_id = Some(channel_id.to_string());
    let (record, lease) = webhook::claim(state, record, version, WebhookOperation::Activate)
        .await
        .map_err(error)?;
    let mut mutation_started = false;
    let mut terminal_refusal = false;
    let result = async {
        let material = record.material.as_ref().ok_or_else(failed)?;
        let token = Secret::new(material.token.clone());
        let telegram = webhook::client(state).map_err(error)?;
        if telegram.get_me(&token).await.map_err(|_| failed())?.id != material.bot_id {
            return Err(invalid("platform credential identity changed"));
        }
        let expected = format!("{}/v1/telegram/{channel_id}", state.advertised_api_url());
        let current = telegram.get_webhook_url(&token).await.map_err(|_| failed())?;
        if !current.is_empty() && !state.public_api_bases().any(|base| current == format!("{base}/v1/telegram/{channel_id}")) {
            return Err(invalid("this bot already has a webhook; disconnect its previous integration before linking it"));
        }
        if !webhook::may_activate(state, id, channel_id.as_str(), &lease).await.map_err(error)? {
            return Err(error(Error::Conflict));
        }
        // Existing valid webhook secrets and URLs remain installed across domain migration.
        if !current.is_empty() && current != expected {
            return Ok(());
        }
        mutation_started = true;
        telegram.set_webhook(&token, &expected, &Secret::new(material.webhook_secret.clone()))
            .await.map_err(|value| {
                terminal_refusal = value.settled();
                error(webhook::failure(value))
            })
    }.await;
    let wanted = webhook::finish_activation(
        state,
        id,
        &lease,
        result.is_ok() || !mutation_started || terminal_refusal,
    )
    .await
    .map_err(error)?;
    if !wanted {
        // Retirement survives process death. The live caller also reconciles as
        // soon as its own remote operation is known to have finished.
        let _ = recovery::reconcile(state, id).await;
        return Err(CustodyError {
            error: error(Error::Conflict),
            unconfirmed: result.is_err() && mutation_started && !terminal_refusal,
        });
    }
    result.map_err(|error| CustodyError {
        error,
        unconfirmed: mutation_started && !terminal_refusal,
    })
}

/// Remove only the webhook whose exact retained channel identity is being deleted.
/// Compatibility entry point for callers that do not need custody metadata.
#[allow(dead_code)]
pub async fn disconnect(
    state: &ManagementState,
    definition: &ChannelDefinition,
    channel_id: &ChannelId,
) -> Result<(), ManagementError> {
    disconnect_custodied(state, definition, channel_id)
        .await
        .map_err(|value| value.error)
}
pub(crate) async fn disconnect_custodied(
    state: &ManagementState,
    definition: &ChannelDefinition,
    channel_id: &ChannelId,
) -> Result<(), CustodyError> {
    disconnect_owned(state, definition, Some(channel_id)).await
}

pub(crate) async fn disconnect_owned(
    state: &ManagementState,
    definition: &ChannelDefinition,
    channel_id: Option<&ChannelId>,
) -> Result<(), CustodyError> {
    let Some(id) = definition.connection_ref.as_deref() else {
        return Ok(());
    };
    let (mut record, version) = match store(state).connection(id).await {
        Ok(value) => value,
        Err(Error::NotFound) if channel_id.is_none() => return Ok(()),
        Err(value) => return Err(CustodyError::before_send(error(value))),
    };
    if (record.organization
        != (definition.owner.kind == cmaeg_core::profile::NyxIdIdentityKind::Organization))
        || record.owner != definition.owner.subject
        || record.channel_record.as_deref() != Some(definition.record_id.as_str())
    {
        return Err(CustodyError::before_send(ManagementError::NotFound));
    }
    if let Some(channel_id) = channel_id {
        if record
            .channel_id
            .as_deref()
            .is_some_and(|id| id != channel_id.as_str())
        {
            return Err(CustodyError::before_send(ManagementError::NotFound));
        }
        // Keep the exact pointer even when deletion wins before activation started.
        record.channel_id = Some(channel_id.to_string());
    } else if let Some(retained) = &record.channel_id {
        let retained = ChannelId::new(retained.as_str())
            .map_err(|_| CustodyError::before_send(ManagementError::NotFound))?;
        crate::console::channel_ids::opens_to(
            &retained,
            &definition.record_id,
            &state.master_keys,
            &state.records,
        )
        .map_err(|_| CustodyError::before_send(ManagementError::NotFound))?;
    }
    record.webhook_retiring = true;
    store(state)
        .save_connection(&record, Some(version))
        .await
        .map_err(|value| CustodyError::before_send(error(value)))?;
    match webhook::cleanup_custodied(state, id, false).await {
        // A live activation is not a reason to lose the delete intent. It or the
        // durable worker will clean up after the platform operation settles.
        Ok(()) => Ok(()),
        Err(value) if channel_id.is_some() && matches!(value.error, Error::Conflict) => Ok(()),
        Err(value) => Err(CustodyError {
            error: error(value.error),
            unconfirmed: value.unconfirmed,
        }),
    }
}

pub async fn release(
    state: &ManagementState,
    definition: &ChannelDefinition,
) -> Result<(), ManagementError> {
    release_owned(state, definition, false).await
}

pub(crate) async fn release_owned(
    state: &ManagementState,
    definition: &ChannelDefinition,
    legacy: bool,
) -> Result<(), ManagementError> {
    let Some(id) = definition.connection_ref.as_deref() else {
        return Ok(());
    };
    let (record, version) = match store(state).connection(id).await {
        Ok(value) => value,
        Err(Error::NotFound) if legacy => return Ok(()),
        Err(value) => return Err(error(value)),
    };
    if (record.organization
        != (definition.owner.kind == cmaeg_core::profile::NyxIdIdentityKind::Organization))
        || record.owner != definition.owner.subject
    {
        return Err(ManagementError::NotFound);
    }
    if record.channel_record.is_none() {
        return Ok(());
    }
    if record.channel_record.as_deref() != Some(definition.record_id.as_str()) {
        return Err(ManagementError::NotFound);
    }
    webhook::retire(state, record, version)
        .await
        .map_err(error)?;
    match recovery::reconcile(state, id).await {
        Ok(()) | Err(Error::Conflict) => Ok(()),
        Err(value) => Err(error(value)),
    }
}

pub(crate) struct CustodyError {
    pub(crate) error: ManagementError,
    pub(crate) unconfirmed: bool,
}
impl CustodyError {
    fn before_send(error: ManagementError) -> Self {
        Self {
            error,
            unconfirmed: false,
        }
    }
}

impl From<ManagementError> for CustodyError {
    fn from(error: ManagementError) -> Self {
        Self {
            error,
            unconfirmed: true,
        }
    }
}
