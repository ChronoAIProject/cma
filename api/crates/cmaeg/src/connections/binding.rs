//! Resolve opaque connection references and own their platform webhook lifecycle.
use super::{model::*, recovery, webhook, Error};
use crate::{
    auth::Creator,
    channels::{
        management::{ManagementError, ManagementState},
        ChannelCreateRequest, ChannelUpdateRequest, SourceRequest,
    },
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
        bot_id: material.bot_id.clone(),
        bot_username: Some(material.username.clone()),
        admission: admission.ok_or_else(|| invalid("connection_admission is required"))?,
    })
}
pub async fn expand_create(
    state: &ManagementState,
    creator: &Creator,
    request: &mut ChannelCreateRequest,
) -> Result<(), ManagementError> {
    let Some(id) = request.connection_ref.clone() else {
        return Ok(());
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
        store(state)
            .save_connection(&connection, Some(version))
            .await
            .map_err(error)?;
    }
    Ok(())
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
    let (topic_lifecycle, topic_creation, message_events, channel_posts, admission) =
        match request.sources.as_slice() {
            [] => (
                *topic_lifecycle,
                *topic_creation,
                *message_events,
                *channel_posts,
                admission.clone(),
            ),
            [SourceRequest::Telegram {
                bot_id,
                bot_username,
                topic_lifecycle,
                topic_creation,
                message_events,
                channel_posts,
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
        bot_id: material.bot_id.clone(),
        bot_username: Some(material.username.clone()),
        admission: request.connection_admission.clone().unwrap_or(admission),
    })
}
/// Called before the durable channel's creation.ready flag can become true.
pub async fn activate(
    state: &ManagementState,
    definition: &ChannelDefinition,
    channel_id: &ChannelId,
) -> Result<(), ManagementError> {
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
        return Err(ManagementError::NotFound);
    }
    if record
        .channel_id
        .as_deref()
        .is_some_and(|old| old != channel_id.as_str())
    {
        return Err(invalid("connection channel identity changed"));
    }
    record.channel_id = Some(channel_id.to_string());
    let (record, lease) = webhook::claim(state, record, version, WebhookOperation::Activate)
        .await
        .map_err(error)?;
    let mut mutation_started = false;
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
            .await.map_err(|_| failed())
    }.await;
    let wanted = webhook::finish_activation(state, id, &lease, result.is_ok() || !mutation_started)
        .await
        .map_err(error)?;
    if !wanted {
        // Retirement survives process death. The live caller also reconciles as
        // soon as its own remote operation is known to have finished.
        let _ = recovery::reconcile(state, id).await;
        return Err(error(Error::Conflict));
    }
    result
}

/// Remove only the webhook whose exact retained channel identity is being deleted.
pub async fn disconnect(
    state: &ManagementState,
    definition: &ChannelDefinition,
    channel_id: &ChannelId,
) -> Result<(), ManagementError> {
    let Some(id) = definition.connection_ref.as_deref() else {
        return Ok(());
    };
    let (mut record, version) = store(state).connection(id).await.map_err(error)?;
    if (record.organization
        != (definition.owner.kind == cmaeg_core::profile::NyxIdIdentityKind::Organization))
        || record.owner != definition.owner.subject
        || record.channel_record.as_deref() != Some(definition.record_id.as_str())
        || record
            .channel_id
            .as_deref()
            .is_some_and(|id| id != channel_id.as_str())
    {
        return Err(ManagementError::NotFound);
    }
    // Keep the exact pointer even when deletion wins before activation started.
    record.channel_id = Some(channel_id.to_string());
    record.webhook_retiring = true;
    store(state)
        .save_connection(&record, Some(version))
        .await
        .map_err(error)?;
    match webhook::cleanup(state, id, false).await {
        // A live activation is not a reason to lose the delete intent. It or the
        // durable worker will clean up after the platform operation settles.
        Ok(()) | Err(Error::Conflict) => Ok(()),
        Err(value) => Err(error(value)),
    }
}

pub async fn release(
    state: &ManagementState,
    definition: &ChannelDefinition,
) -> Result<(), ManagementError> {
    let Some(id) = definition.connection_ref.as_deref() else {
        return Ok(());
    };
    let (record, version) = store(state).connection(id).await.map_err(error)?;
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
