//! Early boot writers, live members and elected resolver decisions share a fence.
use std::collections::BTreeMap;

use futures_util::TryStreamExt;
use mongodb::bson::{DateTime, Document};
use mongodb::options::ReturnDocument;

use super::*;

const WRITER_LEASE_MS: i64 = 45_000;
const RESOLVER_LEASE_MS: i64 = 60_000;
const MAX_WRITERS: i64 = 64;

#[derive(Clone)]
pub(crate) struct BootWriter {
    pub(crate) incarnation: uuid::Uuid,
    pub(crate) pod_uid: uuid::Uuid,
    pub(crate) build_digest: String,
}

#[derive(Clone)]
pub(crate) struct ResolverLease {
    pub(crate) lane: ReleasedLane,
    holder: String,
    epoch: String,
}

#[derive(Deserialize)]
struct Writer {
    pod_uid: String,
    build_digest: String,
    ready_since: Option<DateTime>,
    expires_at: DateTime,
    boot_effect_until: Option<DateTime>,
}

impl MongoReleasedRuntimes {
    fn coordination(&self) -> Collection<Document> {
        majority_collection(&self.database, COLLECTION)
    }

    fn coordination_id(&self) -> String {
        format!("{}:writers", self.deployment)
    }

    pub(crate) async fn publish_boot(&self, writer: &BootWriter) -> Result<(), SandboxStoreError> {
        let path = format!("writers.{}", writer.incarnation);
        let now = self.server_now().await?;
        let mut filter = doc! {"_id": self.coordination_id()};
        filter.insert(&path, doc! {"$exists": false});
        let mut update = doc! {"document_schema_version": SCHEMA_VERSION};
        update.insert(
            path,
            doc! {
                "pod_uid": writer.pod_uid.to_string(),
                "build_digest": &writer.build_digest,
                "expires_at": DateTime::from_millis(now + WRITER_LEASE_MS),
            },
        );
        self.coordination()
            .update_one(filter, doc! {"$set": update})
            .upsert(true)
            .await
            .map_err(unavailable)?;
        Ok(())
    }

    pub(crate) async fn renew_writer(
        &self,
        writer: &BootWriter,
        ready: bool,
    ) -> Result<(), SandboxStoreError> {
        let path = format!("writers.{}", writer.incarnation);
        let expiry = format!("{path}.expires_at");
        let ready_since = format!("{path}.ready_since");
        let mut filter =
            doc! {"_id": self.coordination_id(), "document_schema_version": SCHEMA_VERSION};
        filter.insert(format!("{path}.pod_uid"), writer.pod_uid.to_string());
        filter.insert(format!("{path}.build_digest"), &writer.build_digest);
        filter.insert("$expr", doc! {"$gt": [format!("${expiry}"), "$$NOW"]});
        let mut fields = doc! {};
        fields.insert(expiry, doc! {"$add": ["$$NOW", WRITER_LEASE_MS]});
        fields.insert(
            ready_since.clone(),
            if ready {
                mongodb::bson::Bson::Document(
                    doc! {"$ifNull": [format!("${ready_since}"), "$$NOW"]},
                )
            } else {
                mongodb::bson::Bson::Null
            },
        );
        let write = self
            .coordination()
            .update_one(filter, vec![doc! {"$set": fields}])
            .await
            .map_err(unavailable)?;
        matched(write.matched_count)
    }

    pub(crate) async fn protect_boot_effect(
        &self,
        writer: &BootWriter,
    ) -> Result<(), SandboxStoreError> {
        let path = format!("writers.{}", writer.incarnation);
        let mut filter =
            doc! {"_id": self.coordination_id(), "document_schema_version": SCHEMA_VERSION};
        filter.insert(format!("{path}.pod_uid"), writer.pod_uid.to_string());
        filter.insert(format!("{path}.build_digest"), &writer.build_digest);
        filter.insert(format!("{path}.ready_since"), mongodb::bson::Bson::Null);
        filter.insert(
            "$expr",
            doc! {"$gt": [format!("${path}.expires_at"), "$$NOW"]},
        );
        let mut fields = doc! {};
        fields.insert(
            format!("{path}.boot_effect_until"),
            doc! {"$add":["$$NOW", 60_000_i64]},
        );
        fields.insert(
            format!("{path}.expires_at"),
            doc! {"$add":["$$NOW", WRITER_LEASE_MS]},
        );
        let result = self
            .coordination()
            .update_one(filter, vec![doc! {"$set": fields}])
            .await
            .map_err(unavailable)?;
        matched(result.matched_count)
    }

    pub(crate) async fn remove_writer(&self, writer: &BootWriter) -> Result<(), SandboxStoreError> {
        let mut fields = doc! {};
        fields.insert(format!("writers.{}", writer.incarnation), "");
        self.coordination()
            .update_one(
                doc! {"_id": self.coordination_id(), "$expr": {"$lte": [
                    {"$ifNull": [format!("$writers.{}.boot_effect_until", writer.incarnation), DateTime::from_millis(0)]},
                    "$$NOW",
                ]}},
                doc! {"$unset": fields},
            )
            .await
            .map_err(unavailable)?;
        Ok(())
    }

    pub(crate) async fn elect(
        &self,
        writer: &BootWriter,
        lane: ReleasedLane,
    ) -> Result<Option<ResolverLease>, SandboxStoreError> {
        let holder = writer.incarnation.to_string();
        let epoch = uuid::Uuid::new_v4().to_string();
        let leader = format!("leaders.{}", lane_key(lane));
        let mut fields = doc! {};
        fields.insert(&leader, doc! {
            "holder": &holder, "epoch": &epoch, "expires_at": {"$add": ["$$NOW", RESOLVER_LEASE_MS]},
        });
        let now = self.server_now().await?;
        let mut missing = doc! {};
        missing.insert(&leader, doc! {"$exists": false});
        let mut expired = doc! {};
        expired.insert(
            format!("{leader}.expires_at"),
            doc! {"$lte": DateTime::from_millis(now)},
        );
        let mut own = doc! {};
        own.insert(format!("{leader}.holder"), &holder);
        let filter = doc! {
            "_id": self.coordination_id(),
            "document_schema_version": SCHEMA_VERSION,
            "$or": [missing, expired, own],
        };
        let updated = self
            .coordination()
            .find_one_and_update(filter, vec![doc! {"$set": fields}])
            .return_document(ReturnDocument::After)
            .await
            .map_err(unavailable)?;
        Ok(updated.map(|_| ResolverLease {
            lane,
            holder,
            epoch,
        }))
    }

    pub(crate) async fn fence_writers(
        &self,
        lease: &ResolverLease,
        session: &mut ClientSession,
    ) -> Result<i64, SandboxStoreError> {
        let now = self.server_now().await?;
        let leader = format!("leaders.{}", lane_key(lease.lane));
        let mut filter =
            doc! {"_id": self.coordination_id(), "document_schema_version": SCHEMA_VERSION};
        filter.insert(format!("{leader}.holder"), &lease.holder);
        filter.insert(format!("{leader}.epoch"), &lease.epoch);
        filter.insert(
            "$expr",
            doc! {"$gt": [format!("${leader}.expires_at"), "$$NOW"]},
        );
        let coordination = self
            .coordination()
            .find_one_and_update(filter, doc! {"$inc": {"decision_fence": 1_i64}})
            .session(&mut *session)
            .await
            .map_err(unavailable)?
            .ok_or(SandboxStoreError::Conflict)?;
        let writers: BTreeMap<String, Writer> = mongodb::bson::deserialize_from_document(
            coordination
                .get_document("writers")
                .map_err(corrupt)?
                .clone(),
        )
        .map_err(corrupt)?;
        let build = &writers
            .get(&lease.holder)
            .ok_or(SandboxStoreError::Conflict)?
            .build_digest;
        let members: Collection<Document> =
            majority_collection(&self.database, super::super::agent_ownership::MEMBERS);
        let mut cursor = members
            .find(doc! {"expires_at": {"$gt": DateTime::from_millis(now)}})
            .limit(MAX_WRITERS + 1)
            .session(&mut *session)
            .await
            .map_err(unavailable)?;
        let members: Vec<_> = cursor
            .stream(&mut *session)
            .try_collect()
            .await
            .map_err(unavailable)?;
        if members.is_empty() || members.len() as i64 > MAX_WRITERS {
            return Err(SandboxStoreError::Conflict);
        }
        let mut live = writers
            .iter()
            .filter(|(_, writer)| {
                writer.expires_at.timestamp_millis() > now
                    || writer
                        .boot_effect_until
                        .is_some_and(|until| until.timestamp_millis() > now)
            })
            .collect::<BTreeMap<_, _>>();
        let mut pods = std::collections::BTreeSet::new();
        let mut holder_ready = false;
        for member in members {
            let id = member.get_str("_id").map_err(corrupt)?.to_owned();
            let writer = live.remove(&id).ok_or(SandboxStoreError::Conflict)?;
            holder_ready |= id == lease.holder;
            if member.get_bool("draining").map_err(corrupt)?
                || member.get_i64("protocol").map_err(corrupt)?
                    != i64::from(layer2_core::agent_ownership::PROTOCOL_VERSION)
                || &writer.build_digest != build
                || !pods.insert(&writer.pod_uid)
                || writer.ready_since.is_none_or(|since| {
                    now.saturating_sub(since.timestamp_millis())
                        < crate::warm_pools::RELEASE_SETTLE.as_millis() as i64
                })
            {
                return Err(SandboxStoreError::Conflict);
            }
            let fenced = members_collection(&self.database)
                .update_one(
                    doc! {
                        "_id": id, "draining": false,
                        "protocol": i64::from(layer2_core::agent_ownership::PROTOCOL_VERSION),
                        "$expr": {"$gt": ["$expires_at", "$$NOW"]},
                    },
                    doc! {"$inc": {"runtime_release_fence": 1_i64}},
                )
                .session(&mut *session)
                .await
                .map_err(unavailable)?;
            matched(fenced.matched_count)?;
        }
        if !holder_ready || !live.is_empty() {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(now)
    }

    pub(super) async fn server_now(&self) -> Result<i64, SandboxStoreError> {
        self.database
            .run_command(doc! {"hello": 1})
            .await
            .map_err(unavailable)?
            .get_datetime("localTime")
            .map(|time| time.timestamp_millis())
            .map_err(corrupt)
    }
}

fn members_collection(database: &Database) -> Collection<Document> {
    majority_collection(database, super::super::agent_ownership::MEMBERS)
}

fn matched(count: u64) -> Result<(), SandboxStoreError> {
    if count == 1 {
        Ok(())
    } else {
        Err(SandboxStoreError::Conflict)
    }
}

fn corrupt(error: impl std::fmt::Display) -> SandboxStoreError {
    SandboxStoreError::Corrupt(error.to_string())
}
