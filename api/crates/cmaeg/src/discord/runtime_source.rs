//! Restore the original public channel handle, never mint a replacement identity.
use super::{journal_model::Authority, GatewayError};
use crate::{
    channels::resolver::{ChannelResolver, ResolvedChannel},
    records::{RecordClient, RecordError},
};
use cmaeg_core::{
    channel::ChannelDefinition,
    seal::{open_blob, MasterKeyRing},
};
use std::sync::Arc;

pub(super) struct RuntimeSource {
    pub channel: Arc<ResolvedChannel>,
    pub revision: String,
}

pub(super) async fn resolve(
    records: &RecordClient,
    keys: &MasterKeyRing,
    resolver: &ChannelResolver,
    record: &str,
) -> Result<RuntimeSource, GatewayError> {
    let url = records
        .record_url(record)
        .map_err(|_| GatewayError::Protocol)?;
    let row = records
        .get(url.as_str())
        .await
        .map_err(|error| match error {
            RecordError::NotFound => GatewayError::Authority,
            _ => GatewayError::Unavailable,
        })?;
    let opened = open_blob(keys, &row.blob).map_err(|_| GatewayError::Authority)?;
    let definition: ChannelDefinition =
        serde_json::from_slice(&opened.payload).map_err(|_| GatewayError::Authority)?;
    if definition.record_id.as_str() != record {
        return Err(GatewayError::Authority);
    }
    let creation = definition
        .creation
        .as_ref()
        .filter(|creation| creation.ready)
        .ok_or(GatewayError::InvalidConfiguration)?;
    let channel = resolver
        .resolve_current(creation.channel_id.as_str())
        .await
        .map_err(|error| match error {
            crate::channels::resolver::ResolveError::Unavailable => GatewayError::Unavailable,
            _ => GatewayError::Authority,
        })?;
    if channel.definition.record_id.as_str() != record {
        return Err(GatewayError::Authority);
    }
    let authority = Authority::from_definition(&channel.definition)?;
    Ok(RuntimeSource {
        channel,
        revision: authority.revision,
    })
}
