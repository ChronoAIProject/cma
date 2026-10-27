use super::{model::*, Journal};
use crate::records::{RecordError, RecordValue};
use cmaeg_core::{
    ids::RecordId,
    profile::canonical_json,
    seal::{open_interaction, seal_interaction, MAX_BLOB_BYTES},
};
use serde::{de::DeserializeOwned, Serialize};
use sha2::{Digest, Sha256};

pub(super) fn bytes(value: &impl Serialize) -> Result<Vec<u8>, ExecutionError> {
    Ok(
        canonical_json(serde_json::to_value(value).map_err(|_| ExecutionError::Invalid)?)
            .into_bytes(),
    )
}
pub(super) fn hash(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}
pub(super) fn identifier(value: &impl Serialize) -> String {
    let digest = Sha256::digest(bytes(&("ordinary-route-execution-body-v1", value)).unwrap());
    let mut id: [u8; 16] = digest[..16].try_into().unwrap();
    id[6] = (id[6] & 0x0f) | 0x40;
    id[8] = (id[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(id).to_string()
}
impl Journal {
    pub(super) async fn read<T: DeserializeOwned>(
        &self,
        id: &str,
    ) -> Result<Option<(T, u64)>, ExecutionError> {
        let url = self
            .records
            .record_url(id)
            .map_err(|_| ExecutionError::Invalid)?;
        let RecordValue { blob, version } = match self.records.get(url.as_str()).await {
            Ok(value) => value,
            Err(RecordError::NotFound) => return Ok(None),
            Err(_) => return Err(ExecutionError::Unavailable),
        };
        let bytes = open_interaction(&self.keys, id, &blob)
            .map_err(|_| ExecutionError::Corrupt)?
            .payload;
        let value = serde_json::from_slice(&bytes).map_err(|_| ExecutionError::Corrupt)?;
        Ok(Some((value, version)))
    }
    pub(super) async fn write(
        &self,
        id: &str,
        owner: &str,
        retained_until: i64,
        value: &impl Serialize,
        expected: Option<u64>,
    ) -> Result<u64, ExecutionError> {
        let next_version = expected
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(ExecutionError::Corrupt)?;
        let plaintext = bytes(value)?;
        if plaintext.len() > MAX_BLOB_BYTES {
            return Err(ExecutionError::Invalid);
        }
        let sealed = seal_interaction(&self.keys, id, &plaintext)
            .map_err(|_| ExecutionError::Unavailable)?;
        let result = self
            .records
            .put_expiring(
                &RecordId::new(id).map_err(|_| ExecutionError::Invalid)?,
                owner,
                &sealed,
                expected,
                Some(retained_until),
            )
            .await;
        match result {
            Ok(version) if version == next_version => Ok(version),
            Ok(_) => Err(ExecutionError::Corrupt),
            Err(
                RecordError::Conflict { .. }
                | RecordError::Unavailable(_)
                | RecordError::Malformed(_),
            ) => {
                // A lost CAS response is success only when the exact next value
                // and expected next version are visible. Never infer ownership.
                match self.read::<serde_json::Value>(id).await {
                    Ok(Some((stored, version)))
                        if version == next_version && bytes(&stored)? == plaintext =>
                    {
                        Ok(version)
                    }
                    Ok(Some(_)) => Err(ExecutionError::LostLease),
                    _ => Err(ExecutionError::Uncertain),
                }
            }
            Err(_) => Err(ExecutionError::Unavailable),
        }
    }
}
