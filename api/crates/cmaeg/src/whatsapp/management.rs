//! Owner-authenticated source authoring verifies custody before any stored mutation.
use crate::channels::management::{ManagementError, ManagementState};
use cmaeg_core::{
    channel::{ChannelDefinition, Source},
    ValidationError,
};

pub(crate) async fn verify(
    state: &ManagementState,
    previous: Option<&ChannelDefinition>,
    next: &ChannelDefinition,
) -> Result<(), ManagementError> {
    let Some(source) = next
        .sources
        .iter()
        .find(|source| matches!(source, Source::Whatsapp { .. }))
    else {
        return Ok(());
    };
    let credentials = next
        .secrets
        .whatsapp
        .as_ref()
        .ok_or_else(|| invalid("native WhatsApp credentials required"))?;
    if previous.is_some_and(|old| {
        old.sources
            .iter()
            .any(|old| super::groups::retained_or_reduced(old, source))
            && old.secrets.whatsapp.as_ref() == Some(credentials)
    }) {
        return Ok(());
    }
    let client = match &state.whatsapp {
        Some(client) => (**client).clone(),
        None => super::Client::production()
            .map_err(|_| invalid("native WhatsApp credential verification unavailable"))?,
    };
    let result = async {
        client.verify_custody(source, credentials).await?;
        client.verify_source_groups(source, credentials).await
    }
    .await;
    result.map_err(|error| {
        invalid(match error {
            super::Error::CustodyUnproved => {
                "Meta did not prove the app, WABA phone binding or opted-in group custody"
            }
            super::Error::Rejected | super::Error::Invalid => {
                "Meta rejected native WhatsApp credentials or permissions"
            }
            _ => "Meta custody verification is unavailable; retry without changing the source",
        })
    })
}

pub(crate) fn initialize(
    state: &ManagementState,
    definition: &mut ChannelDefinition,
) -> Result<(), ManagementError> {
    if !definition
        .sources
        .iter()
        .any(|source| matches!(source, Source::Whatsapp { .. }))
    {
        return Ok(());
    }
    if !state.native_whatsapp_admission_enabled || !state.multi_consumer_writes_enabled {
        return Err(invalid(
            "native WhatsApp requires durable admission, recovery and routing authoring",
        ));
    }
    if !definition
        .consumers()
        .filter(|consumer| !consumer.pending)
        .all(|consumer| {
            state
                .providers
                .supports_response_replay(&consumer.provider.slug)
                .unwrap_or(false)
        })
    {
        return Err(invalid(
            "native WhatsApp requires a provider with durable response recovery",
        ));
    }
    if definition.routing.is_none() {
        definition.routing = Some(serde_json::from_value(serde_json::json!({
            "version":1,"default_consumer":{"name":"default"},"consumers":[],"default_targets":["default"]
        })).map_err(|_| invalid("native WhatsApp routing initialization failed"))?);
    }
    Ok(())
}
fn invalid(message: &'static str) -> ManagementError {
    ManagementError::Invalid(ValidationError::new("/sources/whatsapp", message))
}
