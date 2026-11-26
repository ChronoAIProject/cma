//! Permanent worker-use exclusion. A key's owner RBAC never erases its custody.
//! No negative cache: every owner authentication reads both live authorities.
use std::{
    collections::BTreeSet,
    fmt::Debug,
    sync::{Arc, Mutex},
};

use cmaeg_core::{
    channel::ChannelDefinition,
    seal::{open_retained_channel, MasterKeyRing},
};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};

use crate::records::RecordError;

pub use crate::config::owner_keys as config;
pub mod cutover;
pub mod mongo;

pub use config::{CUTOVER_ID, OWNER_KEYS, SERVICE_ID, TRACKING};

/// Runtime cannot write this operator attestation. It says nothing about lost
/// pre-cutover history; only later-issued parent keys may enter the owner lane.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cutover {
    #[serde(rename = "_id")]
    pub id: String,
    pub attestation_id: String,
    pub not_before: i64,
    pub revision: i64,
    pub writer_release: String,
    pub all_writers_tracked: bool,
    pub retained_records_backfilled: bool,
}

impl Cutover {
    pub fn admits(&self, expected_id: &str, issued_at: u64) -> bool {
        self.id == "owner-lane"
            && self.attestation_id == expected_id
            && self.revision > 0
            && self.not_before > 0
            && self.all_writers_tracked
            && self.retained_records_backfilled
            && self.writer_release.len() == 40
            && self
                .writer_release
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            && issued_at > self.not_before as u64
    }
}

pub trait WorkerHistory: Send + Sync + Debug {
    fn record<'a>(&'a self, sha256: &'a str) -> BoxFuture<'a, Result<(), RecordError>>;
    fn was_worker<'a>(&'a self, sha256: &'a str) -> BoxFuture<'a, Result<bool, RecordError>>;
    fn cutover(&self) -> BoxFuture<'_, Result<Option<Cutover>, RecordError>>;
}

pub fn canonical_digest(value: &str) -> Result<&str, RecordError> {
    let bare = value.strip_prefix("sha256:").unwrap_or(value);
    if bare.len() != 64
        || !bare
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(RecordError::Rejected(
            "canonical SHA-256 digest required".into(),
        ));
    }
    Ok(bare)
}

#[derive(Clone, Debug)]
pub struct WorkerCredentials {
    history: Arc<dyn WorkerHistory>,
    keys: Arc<MasterKeyRing>,
    durable_known: Arc<Mutex<BTreeSet<String>>>,
}

impl WorkerCredentials {
    pub fn new(history: Arc<dyn WorkerHistory>, keys: Arc<MasterKeyRing>) -> Self {
        Self {
            history,
            keys,
            durable_known: Arc::new(Mutex::new(BTreeSet::new())),
        }
    }

    pub async fn from_environment(db: &mongodb::Database) -> Result<Option<Self>, RecordError> {
        let settings = config::Settings::from_env()?;
        if !settings.tracking {
            mongo::refuse_untracked_after_cutover(db).await?;
            return Ok(None);
        }
        let key_spec = crate::config::secrets::read(
            crate::config::CMAEG_MASTER_KEYS,
            crate::CMAEG_MASTER_KEYS_FILE,
        )
        .map_err(|_| {
            RecordError::Rejected("worker tracking requires the gateway key ring".into())
        })?;
        let keys = MasterKeyRing::parse(&key_spec)
            .map_err(|_| RecordError::Rejected("worker tracking key ring is invalid".into()))?;
        let history = mongo::MongoWorkerHistory::open(db.clone()).await?;
        Ok(Some(Self::new(Arc::new(history), Arc::new(keys))))
    }

    pub fn history(&self) -> Arc<dyn WorkerHistory> {
        self.history.clone()
    }

    pub async fn record_key(&self, key: &str) -> Result<(), RecordError> {
        if key.is_empty() || key.len() > 16_384 {
            return Err(RecordError::Rejected("invalid worker credential".into()));
        }
        self.record_digest(canonical_digest(
            &crate::channels::definition::hash_secret(key),
        )?)
        .await
    }

    async fn record_digest(&self, hash: &str) -> Result<(), RecordError> {
        // A permanently retained positive fact can skip repeated writes during
        // replay/media. Owner authentication never consults this cache.
        let unavailable =
            || RecordError::Unavailable("worker registration cache unavailable".into());
        if self
            .durable_known
            .lock()
            .map_err(|_| unavailable())?
            .contains(hash)
        {
            return Ok(());
        }
        self.history.record(hash).await?;
        let mut known = self.durable_known.lock().map_err(|_| unavailable())?;
        if known.len() < 4096 {
            known.insert(hash.to_owned());
        }
        Ok(())
    }

    /// Used at the persistence boundary, including offline import and rekeying.
    /// Retain default/additional/pending hashes even after rotation or removal.
    pub async fn record_blob(
        &self,
        record_id: &str,
        version: u64,
        blob: &[u8],
    ) -> Result<(), RecordError> {
        let (payload, _) =
            open_retained_channel(&self.keys, record_id, version, blob).map_err(|_| {
                RecordError::Rejected("cannot classify sealed channel credentials".into())
            })?;
        let definition: ChannelDefinition = serde_json::from_slice(&payload.payload)
            .map_err(|_| RecordError::Rejected("cannot classify channel definition".into()))?;
        definition
            .validate()
            .map_err(|_| RecordError::Rejected("invalid channel credential admission".into()))?;
        let mut hashes = BTreeSet::new();
        for consumer in definition.consumers() {
            hashes.insert(canonical_digest(consumer.agent_key_hash)?.to_owned());
            if let Some(key) = consumer.credential {
                hashes.insert(
                    canonical_digest(&crate::channels::definition::hash_secret(key.as_str()))?
                        .to_owned(),
                );
            }
        }
        for (_, key) in &definition.secrets.consumer_keys {
            hashes.insert(
                canonical_digest(&crate::channels::definition::hash_secret(key.as_str()))?
                    .to_owned(),
            );
        }
        if let Some(revisions) = &definition.secrets.profile_revisions {
            for retained in &revisions.retained {
                hashes.insert(
                    canonical_digest(&crate::channels::definition::hash_secret(
                        retained.credential.as_str(),
                    ))?
                    .to_owned(),
                );
            }
        }
        for hash in hashes {
            self.record_digest(&hash).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests;
