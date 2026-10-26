//! Sealed native Discord records over the production CAS record client.
use super::GatewayError;
use crate::records::{RecordClient, RecordError};
use cmaeg_core::{
    ids::RecordId,
    seal::{open_connection, open_interaction, seal_connection, seal_interaction, MasterKeyRing},
};
use serde::{de::DeserializeOwned, Serialize};
use std::sync::Arc;

#[derive(Clone)]
pub(super) struct Store {
    pub channels: RecordClient,
    pub keys: Arc<MasterKeyRing>,
    pub clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}
impl Store {
    pub fn new(channels: RecordClient, keys: Arc<MasterKeyRing>) -> Self {
        Self {
            channels,
            keys,
            clock: Arc::new(|| {
                (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64
            }),
        }
    }
    fn records(&self, inbox: bool) -> RecordClient {
        if inbox {
            self.channels.clone().interactions()
        } else {
            self.channels.clone().connections()
        }
    }
    pub async fn load<T: DeserializeOwned>(
        &self,
        id: &str,
        inbox: bool,
    ) -> Result<Option<(T, u64)>, GatewayError> {
        let records = self.records(inbox);
        let url = records.record_url(id).map_err(|_| GatewayError::Protocol)?;
        let record = match records.get(url.as_str()).await {
            Ok(value) => value,
            Err(RecordError::NotFound) => return Ok(None),
            Err(_) => return Err(GatewayError::Unavailable),
        };
        let bytes = if inbox {
            open_interaction(&self.keys, id, &record.blob)
        } else {
            open_connection(&self.keys, id, &record.blob)
        }
        .map_err(|_| GatewayError::Protocol)?
        .payload;
        let value = serde_json::from_slice(&bytes).map_err(|_| GatewayError::Protocol)?;
        Ok(Some((value, record.version)))
    }
    pub async fn put<T: Serialize>(
        &self,
        id: &str,
        owner: &str,
        value: &T,
        expected: Option<u64>,
        expiry: Option<i64>,
    ) -> Result<u64, GatewayError> {
        let bytes = serde_json::to_vec(value).map_err(|_| GatewayError::Protocol)?;
        let blob = if expiry.is_some() {
            seal_interaction(&self.keys, id, &bytes)
        } else {
            seal_connection(&self.keys, id, &bytes)
        }
        .map_err(|_| GatewayError::Protocol)?;
        self.records(expiry.is_some())
            .put_expiring(
                &RecordId::new(id).map_err(|_| GatewayError::Protocol)?,
                owner,
                &blob,
                expected,
                expiry,
            )
            .await
            .map_err(|error| match error {
                RecordError::Conflict { .. } => GatewayError::Authority,
                _ => GatewayError::Unavailable,
            })
    }
    pub async fn immutable<T: Serialize>(
        &self,
        id: &str,
        owner: &str,
        value: &T,
        expiry: i64,
    ) -> Result<(), GatewayError> {
        let bytes = serde_json::to_value(value).map_err(|_| GatewayError::Protocol)?;
        match self.put(id, owner, value, None, Some(expiry)).await {
            Ok(1) => Ok(()),
            Ok(_) => Err(GatewayError::Protocol),
            Err(error) => match self.load::<serde_json::Value>(id, true).await? {
                Some((stored, 1)) if stored == bytes => Ok(()),
                Some(_) => Err(GatewayError::Protocol),
                None => Err(error),
            },
        }
    }
}
