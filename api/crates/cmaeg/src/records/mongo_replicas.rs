//! Exact version and server-time lease checks compose with existing sealed journals.
use super::{
    replica_retry::{self, AttemptError},
    MongoRecords, Record, RecordError, CONNECTIONS, INTERACTIONS,
};
use mongodb::{
    bson::{doc, spec::BinarySubtype, Binary, DateTime, Document},
    options::{Acknowledgment, ReadConcern, WriteConcern},
    ClientSession,
};
use serde::{Deserialize, Serialize};

/// A bounded discovery projection. Encrypted journal contents remain authoritative.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReplicaProjection {
    pub kind: ReplicaKind,
    pub deployment: String,
    pub active: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReplicaKind {
    Member,
    Partition,
    Effect,
    Admission,
}

/// Constructed only after opening a sealed record and matching the captured token.
#[derive(Clone)]
pub(crate) struct ReplicaGuard {
    pub id: String,
    pub owner: String,
    pub version: u64,
    pub until_ms: i64,
}

#[derive(Clone, Copy)]
pub(crate) enum LeaseCondition {
    Live(i64),
    Expired(i64),
}

pub(crate) struct ReplicaWrite {
    pub id: String,
    pub owner: String,
    pub blob: Vec<u8>,
    pub expected: Option<u64>,
    pub expires_at: Option<i64>,
    pub condition: Option<LeaseCondition>,
    pub guard: Option<ReplicaGuard>,
    pub channel: Option<(String, u64)>,
    pub projection: Option<ReplicaProjection>,
}

impl MongoRecords {
    /// Queue insertion/completion and the head advance commit together. The
    /// retained item prevents an admission retry from appending a completed item.
    pub(crate) async fn replica_write(
        &self,
        writes: &[ReplicaWrite],
    ) -> Result<Vec<u64>, RecordError> {
        if writes.is_empty() || writes.len() > 8 {
            return Err(RecordError::Rejected(
                "invalid replica transaction size".into(),
            ));
        }
        replica_retry::run(|| self.replica_attempt(writes)).await
    }

    async fn replica_attempt(&self, writes: &[ReplicaWrite]) -> Result<Vec<u64>, AttemptError> {
        #[cfg(test)]
        if let Some(fixture) = &self.replica_attempt_fixture {
            return fixture.attempt(writes).await;
        }
        let mut session = self
            .db
            .client()
            .start_session()
            .await
            .map_err(AttemptError::from)?;
        session
            .start_transaction()
            .read_concern(ReadConcern::snapshot())
            .write_concern(WriteConcern::builder().w(Acknowledgment::Majority).build())
            .await
            .map_err(AttemptError::from)?;
        let mut versions = Vec::with_capacity(writes.len());
        for write in writes {
            versions.push(self.replica_record(write, &mut session).await?);
        }
        // Ambiguous commits are surfaced; callers retain custody and retry the
        // same admission rather than guessing whether an external effect occurred.
        session
            .commit_transaction()
            .await
            .map_err(AttemptError::from)?;
        Ok(versions)
    }

    async fn replica_record(
        &self,
        write: &ReplicaWrite,
        session: &mut ClientSession,
    ) -> Result<u64, AttemptError> {
        if write.id.is_empty()
            || write.id.len() > 512
            || write.owner.is_empty()
            || write.owner.len() > 512
            || write.blob.is_empty()
            || write.blob.len() > 65_536
            || write.expected == Some(0)
            || write.projection.as_ref().is_some_and(|value| {
                value.deployment.len() != 64
                    || !value
                        .deployment
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit())
            })
        {
            return Err(RecordError::Rejected("invalid replica journal write".into()).into());
        }
        let version = write
            .expected
            .unwrap_or(0)
            .checked_add(1)
            .and_then(|value| i64::try_from(value).ok())
            .ok_or_else(|| RecordError::Rejected("replica journal version exhausted".into()))?;
        let expires_at = write
            .expires_at
            .map(|value| {
                value
                    .checked_mul(1000)
                    .map(DateTime::from_millis)
                    .ok_or_else(|| RecordError::Rejected("invalid replica retention".into()))
            })
            .transpose()?;
        let collection = if expires_at.is_some() {
            INTERACTIONS
        } else {
            CONNECTIONS
        };
        let records = self.db.collection::<Record>(collection);
        if let Some((id, version)) = &write.channel {
            let version_i64 = i64::try_from(*version)
                .ok()
                .filter(|version| *version > 0)
                .ok_or_else(|| RecordError::Rejected("invalid source guard version".into()))?;
            let changed = self
                .db
                .collection::<Document>(super::RECORDS)
                .update_one(
                    doc! {"_id":id,"owner":&write.owner,"version":version_i64},
                    doc! {"$inc":{"guard_revision":1_i64}},
                )
                .session(&mut *session)
                .await
                .map_err(AttemptError::from)?;
            if changed.matched_count != 1 {
                return Err(RecordError::GuardConflict {
                    scope: "channel",
                    expected: *version,
                }
                .into());
            }
        }
        if let Some(guard) = &write.guard {
            let guarded_version = i64::try_from(guard.version)
                .map_err(|_| RecordError::Rejected("invalid replica guard version".into()))?;
            let mut filter = doc! {"_id":&guard.id,"owner":&guard.owner,"version":guarded_version};
            condition(&mut filter, LeaseCondition::Live(guard.until_ms));
            let result = self
                .db
                .collection::<Document>(CONNECTIONS)
                .update_one(filter, doc! {"$inc":{"guard_revision":1_i64}})
                .session(&mut *session)
                .await
                .map_err(AttemptError::from)?;
            if result.matched_count != 1 {
                return Err(RecordError::GuardConflict {
                    scope: "replica_partition",
                    expected: guard.version,
                }
                .into());
            }
        }
        let record = Record {
            id: write.id.clone(),
            owner: write.owner.clone(),
            blob: Binary {
                subtype: BinarySubtype::Generic,
                bytes: write.blob.to_vec(),
            },
            version,
            schema_version: 1,
            guard_revision: 0,
            expires_at,
            replica: write.projection.clone(),
        };
        if let Some(expected) = write.expected {
            let mut filter = doc! {"_id":&write.id,"owner":&write.owner,"version":version-1};
            if expires_at.is_some() {
                filter.insert("$expr", doc! {"$gt":["$expires_at","$$NOW"]});
            }
            if let Some(required) = write.condition {
                condition(&mut filter, required);
            }
            let mut fields = doc! {"blob":&record.blob,"version":version};
            if let Some(expiry) = expires_at {
                fields.insert("expires_at", expiry);
            }
            if let Some(projection) = &record.replica {
                fields.insert(
                    "replica",
                    doc! {"kind":match projection.kind {ReplicaKind::Member=>"member",ReplicaKind::Partition=>"partition",ReplicaKind::Effect=>"effect",ReplicaKind::Admission=>"admission"},
                        "deployment":&projection.deployment,"active":projection.active},
                );
            }
            let result = records
                .update_one(filter, doc! {"$set":fields})
                .session(&mut *session)
                .await
                .map_err(AttemptError::from)?;
            if result.matched_count != 1 {
                return Err(RecordError::Conflict {
                    expected,
                    actual: None,
                }
                .into());
            }
        } else {
            records.insert_one(record).session(&mut *session).await.map_err(|error| {
                if matches!(error.kind.as_ref(), mongodb::error::ErrorKind::Write(mongodb::error::WriteFailure::WriteError(value)) if value.code == 11000) {
                    AttemptError::Record(RecordError::Conflict { expected: 0, actual: None })
                } else {
                    AttemptError::from(error)
                }
            })?;
        }
        Ok(version as u64)
    }
}

fn condition(filter: &mut Document, required: LeaseCondition) {
    let (operator, deadline) = match required {
        LeaseCondition::Live(deadline) => ("$lt", deadline),
        LeaseCondition::Expired(deadline) => ("$gte", deadline),
    };
    let mut comparison = Document::new();
    comparison.insert(
        operator,
        vec![
            mongodb::bson::Bson::String("$$NOW".into()),
            DateTime::from_millis(deadline).into(),
        ],
    );
    if let Some(existing) = filter.remove("$expr") {
        filter.insert(
            "$expr",
            doc! {"$and":[existing, mongodb::bson::Bson::Document(comparison)]},
        );
    } else {
        filter.insert("$expr", comparison);
    }
}
