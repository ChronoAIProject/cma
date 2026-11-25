//! Reconcile durable webhook retirement before releasing channel reservations.
use super::{model::*, webhook, ConnectionService, Error};
use crate::{channels::management::ManagementState, records::RecordError};
use cmaeg_core::{channel::ChannelDefinition, seal::open_retained_channel};

async fn channel_exists(state: &ManagementState, record: &Connection) -> Result<bool, Error> {
    let Some(id) = &record.channel_record else {
        return Ok(false);
    };
    let url = state
        .records
        .record_url(id)
        .map_err(|_| Error::Unavailable)?;
    let value = match state.records.get(url.as_str()).await {
        Ok(value) => value,
        Err(RecordError::NotFound) => return Ok(false),
        Err(_) => return Err(Error::Unavailable),
    };
    let (opened, _) = open_retained_channel(&state.master_keys, id, value.version, &value.blob)
        .map_err(|_| Error::Unavailable)?;
    let definition: ChannelDefinition =
        serde_json::from_slice(&opened.payload).map_err(|_| Error::Unavailable)?;
    Ok(definition.connection_ref.as_deref() == Some(&record.id))
}

pub(super) async fn reconcile(state: &ManagementState, id: &str) -> Result<(), Error> {
    let (record, version) = webhook::store(state).connection(id).await?;
    // An unactivated reservation is deliberately preserved by automatic recovery.
    if record.channel_id.is_none() && !record.webhook_retiring {
        return Ok(());
    }
    let exists = channel_exists(state, &record).await?;
    if exists && !record.webhook_retiring {
        return Ok(());
    }
    webhook::retire(state, record, version).await?;
    webhook::cleanup(state, id, !exists).await
}

impl ConnectionService {
    pub async fn channel_exists(&self, record: &Connection) -> Result<bool, Error> {
        channel_exists(&self.management, record).await
    }
    /// Cancellation first saves its terminal status and retirement intent. This
    /// method then releases its pointers only after platform cleanup is confirmed.
    pub async fn reconcile_webhook_retirement(&self, id: &str) -> Result<(), Error> {
        reconcile(&self.management, id).await
    }
    pub async fn recover_deleted_channel(
        &self,
        record: Connection,
        _version: u64,
    ) -> Result<(), Error> {
        reconcile(&self.management, &record.id).await
    }
}
