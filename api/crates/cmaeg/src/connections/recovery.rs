//! Recover only connection reservations whose exact channel record is absent.
use super::{model::*, ConnectionService, Error};
use crate::records::RecordError;
use cmaeg_core::{
    channel::{ChannelDefinition, Secret},
    seal::open_blob,
};

impl ConnectionService {
    pub async fn channel_exists(&self, record: &Connection) -> Result<bool, Error> {
        let Some(id) = &record.channel_record else {
            return Ok(false);
        };
        let url = self
            .management
            .records
            .record_url(id)
            .map_err(|_| Error::Unavailable)?;
        let value = match self.management.records.get(url.as_str()).await {
            Ok(value) => value,
            Err(RecordError::NotFound) => return Ok(false),
            Err(_) => return Err(Error::Unavailable),
        };
        let bytes = open_blob(&self.management.master_keys, &value.blob)
            .map_err(|_| Error::Unavailable)?
            .payload;
        let definition: ChannelDefinition =
            serde_json::from_slice(&bytes).map_err(|_| Error::Unavailable)?;
        Ok(definition.connection_ref.as_deref() == Some(&record.id))
    }
    pub async fn remove_orphan_webhook(&self, record: &Connection) -> Result<(), Error> {
        let Some(id) = &record.channel_id else {
            return Ok(());
        };
        let material = record.material.as_ref().ok_or(Error::Unavailable)?;
        let token = Secret::new(material.token.clone());
        let client = self.telegram()?;
        let expected = format!("{}/v1/telegram/{id}", self.management.public_url);
        let actual = client
            .get_webhook_url(&token)
            .await
            .map_err(|_| Error::Unavailable)?;
        if actual == expected {
            client
                .delete_webhook(&token)
                .await
                .map_err(|_| Error::Unavailable)?;
        }
        Ok(())
    }
    pub async fn recover_deleted_channel(
        &self,
        mut record: Connection,
        version: u64,
    ) -> Result<(), Error> {
        if record.channel_id.is_none() || self.channel_exists(&record).await? {
            return Ok(());
        }
        self.remove_orphan_webhook(&record).await?;
        record.channel_record = None;
        record.channel_id = None;
        self.store.save_connection(&record, Some(version)).await?;
        Ok(())
    }
}
