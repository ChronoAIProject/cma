//! Resolve opaque connection references and own their platform webhook lifecycle.
use super::{model::*, store::Store, Error};
use crate::{
    auth::Creator,
    channels::{
        management::{ManagementError, ManagementState},
        ChannelCreateRequest, ChannelUpdateRequest, SourceRequest,
    },
    telegram::TelegramClient,
};
use cmaeg_core::{
    channel::{AdmissionPolicy, ChannelDefinition, Secret},
    ids::{ChannelId, RecordId},
    ValidationError,
};
use std::time::Duration;

fn store(state: &ManagementState) -> Store {
    Store::new((*state.records).clone(), state.master_keys.clone())
}
fn client(state: &ManagementState) -> Result<TelegramClient, ManagementError> {
    match &state.telegram {
        Some(client) => Ok((**client).clone()),
        None => TelegramClient::production(Duration::from_secs(8)).map_err(|_| failed()),
    }
}
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
        Error::Invalid(message) => invalid(message),
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
    if record.status != Status::Ready {
        return Err(invalid("connection is not ready"));
    }
    Ok((record, version))
}
fn source(
    material: &Material,
    admission: Option<AdmissionPolicy>,
) -> Result<SourceRequest, ManagementError> {
    Ok(SourceRequest::Telegram {
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
    record_id: &RecordId,
    current_ref: Option<&str>,
    request: &mut ChannelUpdateRequest,
) -> Result<(), ManagementError> {
    if request.connection_ref.as_deref() != current_ref {
        return Err(invalid("channel connection is immutable"));
    }
    let Some(id) = request.connection_ref.as_deref() else {
        return Ok(());
    };
    if !request.sources.is_empty() {
        return Err(invalid("connection_ref replaces platform sources"));
    }
    if request.credentials.as_ref().is_some_and(|patch| {
        patch.telegram_bot_token.is_some() || patch.telegram_webhook_secret.is_some()
    }) {
        return Err(invalid("connection credentials are managed by the gateway"));
    }
    let (record, _) = owned(state, creator, id).await?;
    if record.channel_record.as_deref() != Some(record_id.as_str()) {
        return Err(ManagementError::NotFound);
    }
    request.sources = vec![source(
        record.material.as_ref().ok_or_else(failed)?,
        request.connection_admission.clone(),
    )?];
    Ok(())
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
    if record.channel_id.is_none() {
        record.channel_id = Some(channel_id.to_string());
        store(state)
            .save_connection(&record, Some(version))
            .await
            .map_err(error)?;
    }
    let material = record.material.as_ref().ok_or_else(failed)?;
    let token = Secret::new(material.token.clone());
    let telegram = client(state)?;
    if telegram.get_me(&token).await.map_err(|_| failed())?.id != material.bot_id {
        return Err(invalid("platform credential identity changed"));
    }
    let expected = format!("{}/v1/telegram/{channel_id}", state.public_url);
    let current = telegram
        .get_webhook_url(&token)
        .await
        .map_err(|_| failed())?;
    if !current.is_empty() && current != expected {
        return Err(invalid(
            "this bot already has a webhook; disconnect its previous integration before linking it",
        ));
    }
    telegram
        .set_webhook(
            &token,
            &expected,
            &Secret::new(material.webhook_secret.clone()),
        )
        .await
        .map_err(|_| failed())
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
    let (record, _) = store(state).connection(id).await.map_err(error)?;
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
    let material = record.material.as_ref().ok_or_else(failed)?;
    let token = Secret::new(material.token.clone());
    let telegram = client(state)?;
    let expected = format!("{}/v1/telegram/{channel_id}", state.public_url);
    let current = telegram
        .get_webhook_url(&token)
        .await
        .map_err(|_| failed())?;
    if current == expected {
        telegram
            .delete_webhook(&token)
            .await
            .map_err(|_| failed())?;
    }
    Ok(())
}
pub async fn release(
    state: &ManagementState,
    definition: &ChannelDefinition,
) -> Result<(), ManagementError> {
    let Some(id) = definition.connection_ref.as_deref() else {
        return Ok(());
    };
    let (mut record, version) = store(state).connection(id).await.map_err(error)?;
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
    record.channel_record = None;
    record.channel_id = None;
    store(state)
        .save_connection(&record, Some(version))
        .await
        .map_err(error)?;
    Ok(())
}
