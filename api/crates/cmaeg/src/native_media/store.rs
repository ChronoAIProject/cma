//! Immutable confirmed upload and CAS current state share the fixed authority expiry.
use super::{
    model::{Current, Original, Snapshot},
    unavailable,
};
use crate::{records::RecordClient, tools::message_effects::mutation::hash};
use cmaeg_core::{
    ids::RecordId,
    problem::Problem,
    seal::{open_interaction, seal_interaction, MasterKeyRing},
};
use serde::{de::DeserializeOwned, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub struct Store {
    records: RecordClient,
    keys: Arc<MasterKeyRing>,
}
impl Store {
    pub fn new(records: RecordClient, keys: Arc<MasterKeyRing>) -> Self {
        Self {
            records: records.interactions(),
            keys,
        }
    }
    pub(crate) async fn issue(&self, original: &Original) -> Result<String, Problem> {
        let digest = hash(original);
        self.save(
            "original",
            &digest,
            &original.owner,
            original,
            None,
            original.expires_at,
        )
        .await?;
        let state = Current {
            original_sha256: digest.clone(),
            revision: 0,
            snapshot: original.snapshot.clone(),
            last_operation: None,
            expires_at: original.expires_at,
            version: 0,
        };
        self.save(
            "current",
            &digest,
            &original.owner,
            &state,
            None,
            original.expires_at,
        )
        .await?;
        Ok(digest)
    }
    pub(crate) async fn freeze(
        &self,
        original: &Original,
        operation: &str,
        intended: &serde_json::Value,
    ) -> Result<(), Problem> {
        self.save(
            "attempt",
            operation,
            &original.owner,
            intended,
            None,
            original.expires_at,
        )
        .await?;
        Ok(())
    }
    pub(crate) async fn load(&self, digest: &str) -> Result<(Original, Current), Problem> {
        let (original, _): (Original, _) = self.read("original", digest).await?;
        let (mut state, version): (Current, _) = self.read("current", digest).await?;
        if hash(&original) != digest
            || state.original_sha256 != digest
            || state.expires_at != original.expires_at
            || state.expires_at <= super::now()
        {
            return Err(unavailable());
        }
        state.version = version;
        Ok((original, state))
    }
    pub(crate) async fn advance(
        &self,
        original: &Original,
        state: &Current,
        snapshot: Snapshot,
        operation: &str,
    ) -> Result<Current, Problem> {
        let mut next = Current {
            original_sha256: state.original_sha256.clone(),
            revision: state.revision.checked_add(1).ok_or_else(unavailable)?,
            snapshot,
            last_operation: Some(operation.into()),
            expires_at: state.expires_at,
            version: 0,
        };
        next.version = self
            .save(
                "current",
                &state.original_sha256,
                &original.owner,
                &next,
                Some(state.version),
                state.expires_at,
            )
            .await?;
        Ok(next)
    }
    async fn read<T: DeserializeOwned>(
        &self,
        kind: &str,
        digest: &str,
    ) -> Result<(T, u64), Problem> {
        let id = id(kind, digest);
        let url = self
            .records
            .record_url(id.as_str())
            .map_err(|_| unavailable())?;
        let row = self
            .records
            .get(url.as_str())
            .await
            .map_err(|_| unavailable())?;
        let bytes =
            open_interaction(&self.keys, id.as_str(), &row.blob).map_err(|_| unavailable())?;
        let value = serde_json::from_slice(&bytes.payload).map_err(|_| unavailable())?;
        Ok((value, row.version))
    }
    async fn save(
        &self,
        kind: &str,
        digest: &str,
        owner: &str,
        value: &impl Serialize,
        expected: Option<u64>,
        expiry: i64,
    ) -> Result<u64, Problem> {
        let id = id(kind, digest);
        let bytes = serde_json::to_vec(value).map_err(|_| unavailable())?;
        let blob = seal_interaction(&self.keys, id.as_str(), &bytes).map_err(|_| unavailable())?;
        self.records
            .put_expiring(&id, owner, &blob, expected, Some(expiry))
            .await
            .map_err(|_| unavailable())
    }
}
fn id(kind: &str, digest: &str) -> RecordId {
    let hash = Sha256::digest(format!("cmaeg.native-media-{kind}.v1\0{digest}"));
    let mut bytes: [u8; 16] = hash[..16].try_into().unwrap();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    RecordId::new(uuid::Uuid::from_bytes(bytes).to_string()).expect("valid record ID")
}
