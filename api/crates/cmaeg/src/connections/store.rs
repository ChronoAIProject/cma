//! Durable CAS records; no gateway replica is an authority for pending setup.
use super::{model::*, Error};
use crate::records::{CreateConnectionClaim, ReadFailure, RecordClient, RecordError};
use cmaeg_core::{
    ids::RecordId,
    seal::{open_connection, seal_connection, MasterKeyRing},
};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Clone)]
pub struct Store {
    records: RecordClient,
    keys: Arc<MasterKeyRing>,
}
impl Store {
    pub fn new(records: RecordClient, keys: Arc<MasterKeyRing>) -> Self {
        Self {
            records: records.connections(),
            keys,
        }
    }
    pub async fn load(&self, id: &str) -> Result<(Stored, u64), Error> {
        let url = self.records.record_url(id).map_err(|_| Error::NotFound)?;
        let value = self
            .records
            .get_observed(url.as_str())
            .await
            .map_err(|failure| self.read_error("get", failure))?;
        let bytes = open_connection(&self.keys, id, &value.blob)
            .map_err(|_| {
                self.read_failed("decrypt", "envelope", None);
                Error::Unavailable
            })?
            .payload;
        let value_json: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| {
            self.read_failed("decode", "record_decode", None);
            Error::Unavailable
        })?;
        // Other stores (Discord inbox, reaction and fence state) keep records in
        // this kind under the same owner. A JSON object without this store's tag
        // is theirs, not a corrupt connection; an unknown tag still fails closed.
        if value_json
            .as_object()
            .is_some_and(|object| !object.contains_key("record_kind"))
        {
            return Err(Error::NotFound);
        }
        let record = serde_json::from_value(value_json).map_err(|_| {
            self.read_failed("decode", "record_decode", None);
            Error::Unavailable
        })?;
        Ok((record, value.version))
    }
    pub async fn connection(&self, id: &str) -> Result<(Connection, u64), Error> {
        match self.load(id).await? {
            (Stored::Connection(record), version) if record.id == id => Ok((*record, version)),
            _ => Err(Error::NotFound),
        }
    }
    pub async fn save(
        &self,
        id: &str,
        owner: &str,
        value: &Stored,
        expected: Option<u64>,
    ) -> Result<u64, Error> {
        let blob = self.seal(id, value)?;
        self.records
            .put(
                &RecordId::new(id).map_err(|_| Error::Invalid("invalid record ID"))?,
                owner,
                &blob,
                expected,
            )
            .await
            .map_err(map_error)
    }
    fn seal(&self, id: &str, value: &Stored) -> Result<Vec<u8>, Error> {
        let bytes = serde_json::to_vec(value).map_err(|_| Error::Unavailable)?;
        seal_connection(&self.keys, id, &bytes).map_err(|_| Error::Unavailable)
    }

    pub(crate) fn prepare_create_claim(
        &self,
        value: Connection,
        expected_revision: u64,
    ) -> Result<CreateConnectionClaim, Error> {
        let record_id = value.id.clone();
        let sealed_payload = self.seal(&record_id, &Stored::Connection(Box::new(value)))?;
        Ok(CreateConnectionClaim {
            record_id,
            expected_revision,
            sealed_payload,
        })
    }

    pub async fn save_connection(
        &self,
        value: &Connection,
        expected: Option<u64>,
    ) -> Result<u64, Error> {
        self.save(
            &value.id,
            &value.owner,
            &Stored::Connection(Box::new(value.clone())),
            expected,
        )
        .await
    }
    pub async fn page(
        &self,
        owner: Option<&str>,
        after: Option<&str>,
    ) -> Result<Vec<crate::records::RecordHead>, Error> {
        self.records
            .list_observed(owner, after, 100)
            .await
            .map_err(|failure| self.read_error("page", failure))
    }
    fn read_error(&self, stage: &'static str, failure: ReadFailure) -> Error {
        self.read_failed(stage, failure.class.as_str(), failure.class.http_status());
        map_error(failure.error)
    }
    fn read_failed(
        &self,
        stage: &'static str,
        error_class: &'static str,
        http_status: Option<u16>,
    ) {
        tracing::warn!(
            stage,
            backend = self.records.read_backend().as_str(),
            error_class,
            http_status,
            "connection record read failed"
        );
    }
    pub async fn claim_bot(
        &self,
        bot_id: &str,
        owner: &str,
        connection_id: &str,
    ) -> Result<(), Error> {
        let id = index_id("platform-bot", &["telegram", bot_id]);
        match self
            .save(
                &id,
                owner,
                &Stored::BotClaim(BotClaim {
                    owner: owner.into(),
                    connection_id: connection_id.into(),
                    released: false,
                }),
                None,
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(Error::Conflict) => match self.load(&id).await? {
                (Stored::BotClaim(claim), version) if claim.released => {
                    self.save(
                        &id,
                        owner,
                        &Stored::BotClaim(BotClaim {
                            owner: owner.into(),
                            connection_id: connection_id.into(),
                            released: false,
                        }),
                        Some(version),
                    )
                    .await?;
                    Ok(())
                }
                (Stored::BotClaim(claim), _)
                    if claim.owner == owner && claim.connection_id == connection_id =>
                {
                    Ok(())
                }
                _ => Err(Error::Conflict),
            },
            Err(e) => Err(e),
        }
    }
}

/// Domain-separated deterministic UUIDs provide unique reservations across replicas.
/// The UUID is an index, never a capability or secret.
pub fn index_id(domain: &str, parts: &[&str]) -> String {
    let mut hash = Sha256::new();
    for part in std::iter::once(domain).chain(parts.iter().copied()) {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part.as_bytes());
    }
    let digest = hash.finalize();
    let mut bytes: [u8; 16] = digest[..16].try_into().unwrap();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}
fn map_error(error: RecordError) -> Error {
    match error {
        RecordError::NotFound => Error::NotFound,
        RecordError::Conflict { .. } => Error::Conflict,
        _ => Error::Unavailable,
    }
}

impl Store {
    pub async fn release_bot_claim(
        &self,
        bot_id: &str,
        owner: &str,
        connection_id: &str,
    ) -> Result<(), Error> {
        let id = index_id("platform-bot", &["telegram", bot_id]);
        match self.load(&id).await {
            Ok((Stored::BotClaim(mut claim), version))
                if claim.owner == owner
                    && claim.connection_id == connection_id
                    && !claim.released =>
            {
                claim.released = true;
                self.save(&id, owner, &Stored::BotClaim(claim), Some(version))
                    .await?;
                Ok(())
            }
            Ok(_) | Err(Error::NotFound) => Ok(()),
            Err(e) => Err(e),
        }
    }
}
