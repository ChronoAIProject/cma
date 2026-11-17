//! Bounded sealed-record operations using Mongo server time, never a pod wall clock.
use super::{config::STORE_TIMEOUT, Error};
use crate::records::{
    mongo::{LeaseCondition, ReplicaGuard, ReplicaKind, ReplicaProjection, ReplicaWrite},
    RecordClient,
};
use cmaeg_core::seal::{
    open_connection, open_interaction, seal_connection, seal_interaction, MasterKeyRing,
};
use futures_util::TryStreamExt;
use mongodb::bson::{doc, Document};
use serde::{de::DeserializeOwned, Serialize};
use std::{future::IntoFuture, sync::Arc};
use tokio::time::Instant;

#[derive(Clone)]
pub(crate) struct Store {
    pub records: RecordClient,
    pub keys: Arc<MasterKeyRing>,
}

pub(crate) struct Clock {
    pub millis: i64,
    pub started: Instant,
}

#[derive(Default)]
pub(crate) struct Write<'a> {
    pub expected: Option<u64>,
    pub retained_until: Option<i64>,
    pub condition: Option<LeaseCondition>,
    pub guard: Option<&'a ReplicaGuard>,
    pub channel: Option<(String, u64)>,
    pub projection: Option<ReplicaProjection>,
}

impl Store {
    pub fn new(records: RecordClient, keys: Arc<MasterKeyRing>) -> Result<Self, Error> {
        if records.database().is_none() {
            return Err(Error::Unavailable);
        }
        Ok(Self { records, keys })
    }

    fn client(&self, ephemeral: bool) -> RecordClient {
        if ephemeral {
            self.records.clone().interactions()
        } else {
            self.records.clone().connections()
        }
    }

    pub(super) fn collection(
        &self,
        ephemeral: bool,
    ) -> Result<mongodb::Collection<Document>, Error> {
        let database = self.records.database().ok_or(Error::Unavailable)?;
        let name = if ephemeral {
            crate::records::mongo::INTERACTIONS
        } else {
            crate::records::mongo::CONNECTIONS
        };
        Ok(database.collection_with_options(
            name,
            mongodb::options::CollectionOptions::builder()
                .read_concern(mongodb::options::ReadConcern::majority())
                .build(),
        ))
    }

    pub async fn clock(&self) -> Result<Clock, Error> {
        let database = self.records.database().ok_or(Error::Unavailable)?;
        let started = Instant::now();
        let result = tokio::time::timeout(
            STORE_TIMEOUT,
            database.run_command(doc! {"hello":1}).into_future(),
        )
        .await
        .map_err(|_| Error::Unavailable)?
        .map_err(|_| Error::Unavailable)?;
        let millis = result
            .get_datetime("localTime")
            .map_err(|_| Error::Corrupt)?
            .timestamp_millis();
        Ok(Clock { millis, started })
    }

    pub(super) async fn head_record(
        &self,
        id: &str,
    ) -> Result<Option<(super::partition_model::Head, u64, Option<i64>)>, Error> {
        if let Some((head, version)) = self.read(id, false).await? {
            return Ok(Some((head, version, None)));
        }
        let records = self.collection(true)?;
        let row = tokio::time::timeout(
            STORE_TIMEOUT,
            records
                .find_one(doc! {"_id":id, "$expr":{"$gt":["$expires_at","$$NOW"]}})
                .into_future(),
        )
        .await
        .map_err(|_| Error::Unavailable)?
        .map_err(|_| Error::Unavailable)?;
        row.map(|row| {
            let expiry = row
                .get_datetime("expires_at")
                .map_err(|_| Error::Corrupt)?
                .timestamp_millis()
                .div_euclid(1000);
            let (head, version) = self.decode(id, true, row)?;
            Ok((head, version, Some(expiry)))
        })
        .transpose()
    }

    pub async fn read<T: DeserializeOwned>(
        &self,
        id: &str,
        ephemeral: bool,
    ) -> Result<Option<(T, u64)>, Error> {
        self.client(ephemeral).record_url(id).map_err(Error::from)?;
        let records = self.collection(ephemeral)?;
        let mut filter = doc! {"_id":id};
        if ephemeral {
            filter.insert("$expr", doc! {"$gt":["$expires_at","$$NOW"]});
        }
        let row = tokio::time::timeout(STORE_TIMEOUT, records.find_one(filter).into_future())
            .await
            .map_err(|_| Error::Unavailable)?
            .map_err(|_| Error::Unavailable)?;
        row.map(|row| self.decode(id, ephemeral, row)).transpose()
    }

    pub(super) fn decode<T: DeserializeOwned>(
        &self,
        id: &str,
        ephemeral: bool,
        row: Document,
    ) -> Result<(T, u64), Error> {
        let blob = row.get_binary_generic("blob").map_err(|_| Error::Corrupt)?;
        let opened = if ephemeral {
            open_interaction(&self.keys, id, blob)
        } else {
            open_connection(&self.keys, id, blob)
        }
        .map_err(|_| Error::Corrupt)?;
        let value = serde_json::from_slice(&opened.payload).map_err(|_| Error::Corrupt)?;
        let version = row.get_i64("version").map_err(|_| Error::Corrupt)?;
        if version < 1 {
            return Err(Error::Corrupt);
        }
        Ok((value, version as u64))
    }

    pub fn prepare<T: Serialize>(
        &self,
        id: &str,
        owner: &str,
        value: &T,
        options: Write<'_>,
    ) -> Result<ReplicaWrite, Error> {
        self.records.record_url(id).map_err(Error::from)?;
        let bytes = serde_json::to_vec(value).map_err(|_| Error::Corrupt)?;
        let blob = if options.retained_until.is_some() {
            seal_interaction(&self.keys, id, &bytes)
        } else {
            seal_connection(&self.keys, id, &bytes)
        }
        .map_err(|_| Error::Capacity)?;
        Ok(ReplicaWrite {
            id: id.into(),
            owner: owner.into(),
            blob,
            expected: options.expected,
            expires_at: options.retained_until,
            condition: options.condition,
            guard: options.guard.cloned(),
            channel: options.channel,
            projection: options.projection,
        })
    }

    pub async fn batch(&self, writes: &[ReplicaWrite]) -> Result<Vec<u64>, Error> {
        tokio::time::timeout(STORE_TIMEOUT, self.records.replica_write(writes))
            .await
            .map_err(|_| Error::Unavailable)?
            .map_err(Error::from)
    }

    pub async fn write<T: Serialize>(
        &self,
        id: &str,
        owner: &str,
        value: &T,
        options: Write<'_>,
    ) -> Result<u64, Error> {
        let write = self.prepare(id, owner, value, options)?;
        self.batch(&[write])
            .await?
            .first()
            .copied()
            .ok_or(Error::Corrupt)
    }

    pub(crate) async fn effect_history(
        &self,
        deployment: &str,
        after: Option<&str>,
    ) -> Result<Vec<(String, serde_json::Value, u64)>, Error> {
        let mut query = doc! {"replica.kind":"effect", "replica.deployment":deployment, "$expr":{"$gt":["$expires_at","$$NOW"]}};
        if let Some(after) = after {
            query.insert("_id", doc! {"$gt":after});
        }
        let records = self.collection(true)?;
        let rows: Vec<Document> = tokio::time::timeout(STORE_TIMEOUT, async {
            records
                .find(query)
                .sort(doc! {"_id":1})
                .limit(128)
                .await?
                .try_collect()
                .await
        })
        .await
        .map_err(|_| Error::Unavailable)?
        .map_err(|_| Error::Unavailable)?;
        rows.into_iter()
            .map(|row| {
                let id = row.get_str("_id").map_err(|_| Error::Corrupt)?.to_owned();
                let (value, version) = self.decode(&id, true, row)?;
                Ok((id, value, version))
            })
            .collect()
    }

    pub async fn active<T: DeserializeOwned>(
        &self,
        kind: ReplicaKind,
        deployment: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<(String, T, u64)>, Error> {
        let ephemeral = !matches!(kind, ReplicaKind::Partition);
        self.active_in(kind, deployment, after, limit, ephemeral)
            .await
    }
    pub(crate) async fn active_in<T: DeserializeOwned>(
        &self,
        kind: ReplicaKind,
        deployment: &str,
        after: Option<&str>,
        limit: usize,
        ephemeral: bool,
    ) -> Result<Vec<(String, T, u64)>, Error> {
        if !(1..=128).contains(&limit) {
            return Err(Error::Capacity);
        }
        let mut query = doc! {
            "replica":{"$exists":true},
            "replica.kind":match kind {ReplicaKind::Member=>"member",ReplicaKind::Partition=>"partition",ReplicaKind::Effect=>"effect",ReplicaKind::Admission=>"admission"},
            "replica.deployment":deployment,
            "replica.active":true,
        };
        if let Some(after) = after {
            query.insert("_id", doc! {"$gt":after});
        }
        if ephemeral {
            query.insert("$expr", doc! {"$gt":["$expires_at","$$NOW"]});
        }
        let records = self.collection(ephemeral)?;
        let rows: Vec<Document> = tokio::time::timeout(STORE_TIMEOUT, async {
            records
                .find(query)
                .sort(doc! {"_id":1})
                .limit(limit as i64)
                .await?
                .try_collect()
                .await
        })
        .await
        .map_err(|_| Error::Unavailable)?
        .map_err(|_: mongodb::error::Error| Error::Unavailable)?;
        rows.into_iter()
            .map(|row| {
                let id = row.get_str("_id").map_err(|_| Error::Corrupt)?.to_owned();
                let (value, version) = self.decode(&id, ephemeral, row)?;
                Ok((id, value, version))
            })
            .collect()
    }
}
