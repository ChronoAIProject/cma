//! Majority-durable attempts, atomically admitted with a renewable server-time lease.
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, DateTime, Document, Timestamp},
    options::{ReadConcern, ReturnDocument},
    Client, Collection, Database,
};
use serde_json::{json, Value};

use super::{activity_indexes, Result, IO};
use crate::store::sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome};

pub(super) const COLLECTION: &str = "agent_manager_migrations";
const MANIFEST: &str = "manifest:am-activity-query-indexes-v1";
const LOCK: &str = "runner";
const LEASE_MS: i64 = 30_000;

pub(super) struct Identity {
    source: String,
    image: String,
}

impl Identity {
    pub(super) fn new(source: &str, image: &str) -> Result<Self> {
        let hash = |value: &str, length| {
            value.len() == length
                && value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        if !hash(source, 40)
            || !image
                .strip_prefix("ghcr.io/chronoaiproject/cma-api@sha256:")
                .is_some_and(|value| hash(value, 64))
        {
            return Err("apply requires a release build SHA and immutable API image reference");
        }
        Ok(Self {
            source: source.into(),
            image: image.into(),
        })
    }
}

pub(super) struct Lease {
    pub(super) attempt: String,
    pub(super) fence: i64,
}

pub(super) struct Ledger {
    client: Client,
    database: Database,
    rows: Collection<Document>,
}

fn unsupported() -> Document {
    doc! {"$or":[
        {"schema":{"$ne":1_i64}},
        {"schema":{"$not":{"$type":["int","long"]}}},
        {"schema":{"$type":"array"}},
        {"kind":{"$nin":["manifest","lease","attempt"]}},
        {"kind":{"$not":{"$type":"string"}}},
        {"kind":{"$type":"array"}},
        {"kind":"manifest","_id":{"$ne":MANIFEST}},
        {"kind":"lease","_id":{"$ne":LOCK}},
        {"kind":"attempt","migration":{"$ne":activity_indexes::ID}},
        {"kind":"attempt","checksum":{"$ne":activity_indexes::checksum()}}
    ]}
}

impl Ledger {
    pub(super) fn new(client: Client, database: Database) -> Self {
        Self {
            rows: super::super::durability::majority_collection(&database, COLLECTION),
            client,
            database,
        }
    }

    pub(super) async fn preflight(&self) -> Result<()> {
        if self
            .rows
            .find_one(unsupported())
            .max_time(IO)
            .await
            .map_err(|_| "migration ledger unavailable")?
            .is_some()
        {
            return Err("unknown migration history; older migrator refused");
        }
        if let Some(row) = self
            .rows
            .find_one(doc! {"_id":MANIFEST})
            .max_time(IO)
            .await
            .map_err(|_| "migration manifest unavailable")?
        {
            validate_manifest(&row)?;
        }
        if let Some(row) = self
            .rows
            .find_one(doc! {"_id":LOCK})
            .max_time(IO)
            .await
            .map_err(|_| "migration lease unavailable")?
        {
            validate_lock(&row)?;
        }
        Ok(())
    }

    pub(super) async fn initialize(&self) -> Result<()> {
        self.preflight().await?;
        // An explicit CLI bootstrap only. Never replace existing validators.
        if let Err(error) = self.database.create_collection(COLLECTION).await {
            if !matches!(error.kind.as_ref(), mongodb::error::ErrorKind::Command(command) if command.code == 48)
            {
                return Err("migration ledger bootstrap unavailable");
            }
        }
        let manifest = doc! {"_id":MANIFEST,"schema":1_i64,"kind":"manifest",
        "migration":activity_indexes::ID,"checksum":activity_indexes::checksum(),"completed":false};
        if self.rows.insert_one(manifest).await.is_err() {
            let row = self
                .rows
                .find_one(doc! {"_id":MANIFEST})
                .max_time(IO)
                .await
                .map_err(|_| "manifest insertion outcome uncertain")?
                .ok_or("manifest insertion outcome uncertain")?;
            validate_manifest(&row)?;
        }
        let lock = doc! {"_id":LOCK,"schema":1_i64,"kind":"lease","fence":0_i64,
        "holder":"","expires_at":DateTime::from_millis(0),"acquired_at":DateTime::from_millis(0)};
        if self.rows.insert_one(lock).await.is_err() {
            let row = self
                .rows
                .find_one(doc! {"_id":LOCK})
                .max_time(IO)
                .await
                .map_err(|_| "lease insertion outcome uncertain")?
                .ok_or("lease insertion outcome uncertain")?;
            validate_lock(&row)?;
        }
        Ok(())
    }

    pub(super) async fn completed(&self) -> Result<bool> {
        let row = self
            .rows
            .find_one(doc! {"_id":MANIFEST})
            .max_time(IO)
            .await
            .map_err(|_| "migration completion unavailable")?;
        let Some(row) = row else {
            return Ok(false);
        };
        if !validate_manifest(&row)? {
            return Ok(false);
        }
        let receipt = row
            .get_str("completion_attempt")
            .map_err(|_| "completion receipt missing")?;
        if self.rows.find_one(doc! {"_id":receipt,"schema":1_i64,"kind":"attempt",
            "migration":activity_indexes::ID,"checksum":activity_indexes::checksum(),"outcome":"completed"})
            .max_time(IO).await.map_err(|_| "completion receipt unavailable")?.is_none()
        {
            return Err("completion receipt missing");
        }
        Ok(true)
    }

    pub(super) async fn claim(&self, identity: &Identity) -> Result<Lease> {
        let attempt = format!("attempt:{}", uuid::Uuid::new_v4().simple());
        let mut session = self
            .client
            .start_session()
            .await
            .map_err(|_| "migration session unavailable")?;
        start_transaction(&mut session)
            .await
            .map_err(|_| "migration transaction unavailable")?;
        if self
            .rows
            .find_one(unsupported())
            .max_time(IO)
            .session(&mut session)
            .await
            .map_err(|_| "migration history unavailable")?
            .is_some()
        {
            return Err("unknown migration history; older migrator refused");
        }
        let manifest = self
            .rows
            .find_one(doc! {"_id":MANIFEST})
            .session(&mut session)
            .await
            .map_err(|_| "migration manifest unavailable")?
            .ok_or("migration manifest missing")?;
        if validate_manifest(&manifest)? {
            return Err("migration completed concurrently; verify status");
        }
        let lock = self
            .rows
            .find_one_and_update(
                doc! {"_id":LOCK,"schema":1_i64,"kind":"lease","fence":{"$lt":i64::MAX},
                "$expr":{"$lte":["$expires_at","$$NOW"]}},
                vec![doc! {"$set":{"holder":&attempt,"acquired_at":"$$NOW",
                "expires_at":{"$add":["$$NOW",LEASE_MS]},"fence":{"$add":["$fence",1_i64]}}}],
            )
            .return_document(ReturnDocument::After)
            .session(&mut session)
            .await
            .map_err(|_| "migration lease claim unavailable")?
            .ok_or("migration executor is busy")?;
        validate_lock(&lock)?;
        let fence = lock
            .get_i64("fence")
            .map_err(|_| "migration fence malformed")?;
        let started_at = *lock
            .get_datetime("acquired_at")
            .map_err(|_| "migration timestamp malformed")?;
        let row = doc! {"_id":&attempt,"schema":1_i64,"kind":"attempt",
        "migration":activity_indexes::ID,"checksum":activity_indexes::checksum(),
        "source":&identity.source,"image":&identity.image,"fence":fence,
        "started_at":started_at,"outcome":"running"};
        self.rows
            .insert_one(row.clone())
            .session(&mut session)
            .await
            .map_err(|_| "migration attempt unavailable")?;
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => (),
            CommitOutcome::Unknown(_) => {
                // Read the committed snapshot, not a potentially older majority view.
                let retained = self
                    .read_at(session.operation_time(), doc! {"_id":&attempt})
                    .await?;
                if retained.as_ref() != Some(&row) {
                    return Err("migration admission commit unknown; inspect status");
                }
            }
            _ => return Err("migration admission transaction failed"),
        }
        Ok(Lease { attempt, fence })
    }

    fn owned(lease: &Lease) -> Document {
        doc! {"_id":LOCK,"schema":1_i64,"kind":"lease","holder":&lease.attempt,"fence":lease.fence,
        "$expr":{"$gt":["$expires_at","$$NOW"]}}
    }

    pub(super) async fn renew(&self, lease: &Lease) -> Result<()> {
        let result = self
            .rows
            .update_one(
                Self::owned(lease),
                vec![doc! {"$set":{"expires_at":{"$add":["$$NOW",LEASE_MS]}}}],
            )
            .await
            .map_err(|_| "migration lease renewal unavailable")?;
        if result.matched_count != 1 {
            return Err("migration lease lost");
        }
        Ok(())
    }

    pub(super) async fn finish(&self, lease: &Lease, outcome: Result<i64>) -> Result<()> {
        let mut session = self
            .client
            .start_session()
            .await
            .map_err(|_| "migration session unavailable")?;
        start_transaction(&mut session)
            .await
            .map_err(|_| "migration transaction unavailable")?;
        let result = self
            .rows
            .update_one(
                Self::owned(lease),
                vec![doc! {"$set":{"expires_at":"$$NOW"}}],
            )
            .session(&mut session)
            .await
            .map_err(|_| "migration completion lease unavailable")?;
        if result.matched_count != 1 {
            return Err("migration lease lost before completion");
        }
        let state = match outcome {
            Ok(_) => "completed",
            Err(_) => "failed",
        };
        let result = self
            .rows
            .update_one(
                doc! {"_id":&lease.attempt,"schema":1_i64,
                "kind":"attempt","fence":lease.fence,"outcome":"running",
                "migration":activity_indexes::ID,"checksum":activity_indexes::checksum()},
                doc! {"$set":{"outcome":state,"changed":outcome.ok(),"reason":outcome.err()},
                "$currentDate":{"finished_at":true}},
            )
            .session(&mut session)
            .await
            .map_err(|_| "migration receipt unavailable")?;
        if result.matched_count != 1 {
            return Err("migration attempt drift");
        }
        if outcome.is_ok() {
            let result = self
                .rows
                .update_one(
                    doc! {"_id":MANIFEST,"schema":1_i64,
                    "checksum":activity_indexes::checksum(),"completed":false},
                    doc! {"$set":{"completed":true,"completion_attempt":&lease.attempt}},
                )
                .session(&mut session)
                .await
                .map_err(|_| "migration completion unavailable")?;
            if result.matched_count != 1 {
                return Err("migration completion drift");
            }
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => (),
            CommitOutcome::Unknown(_) => {
                let found = self
                    .read_at(
                        session.operation_time(),
                        doc! {"_id":&lease.attempt,
                        "schema":1_i64,"kind":"attempt","migration":activity_indexes::ID,
                        "checksum":activity_indexes::checksum(),"fence":lease.fence,
                        "outcome":state,"changed":outcome.ok(),"reason":outcome.err()},
                    )
                    .await?;
                if found.is_none() {
                    return Err("migration completion commit unknown; inspect status");
                }
            }
            _ => return Err("migration completion transaction failed"),
        }
        Ok(())
    }

    async fn read_at(&self, time: Option<Timestamp>, query: Document) -> Result<Option<Document>> {
        let time = time.ok_or("migration commit has no reconciliation time")?;
        let mut snapshot = self
            .client
            .start_session()
            .snapshot(true)
            .snapshot_time(time)
            .await
            .map_err(|_| "migration commit snapshot unavailable")?;
        self.rows
            .find_one(query)
            .read_concern(ReadConcern::snapshot())
            .max_time(IO)
            .session(&mut snapshot)
            .await
            .map_err(|_| "migration commit reconciliation unavailable")
    }

    pub(super) async fn status(&self) -> Result<Value> {
        let rows: Vec<_> = self
            .rows
            .find(doc! {"kind":"attempt"})
            .sort(doc! {"started_at":-1,"fence":-1})
            .limit(20)
            .max_time(IO)
            .await
            .map_err(|_| "migration status unavailable")?
            .try_collect()
            .await
            .map_err(|_| "migration status unavailable")?;
        let mut attempts = Vec::new();
        for row in rows {
            let outcome = row
                .get_str("outcome")
                .map_err(|_| "migration outcome malformed")?;
            if !["running", "completed", "failed"].contains(&outcome) {
                return Err("migration outcome unknown");
            }
            let source = row
                .get_str("source")
                .map_err(|_| "migration source missing")?;
            let image = row
                .get_str("image")
                .map_err(|_| "migration image missing")?;
            Identity::new(source, image)?;
            let changed = row.get_i64("changed").ok();
            if (outcome == "completed" && changed.is_none())
                || changed.is_some_and(|count| !(0..=2).contains(&count))
            {
                return Err("migration effect count malformed");
            }
            attempts.push(json!({"fence":row.get_i64("fence").map_err(|_| "migration fence malformed")?,
                "outcome":outcome,"source":source,"image":image,"created_indexes":changed,
                "started_at":row.get_datetime("started_at")
                    .map_err(|_| "migration start malformed")?.timestamp_millis(),
                "finished_at":row.get_datetime("finished_at").ok().map(|time|time.timestamp_millis())}));
        }
        Ok(
            json!({"migration":activity_indexes::ID,"checksum":activity_indexes::checksum(),
            "completed":self.completed().await?,"recent_attempts":attempts,
            "history_limit":20,"running_means":"unfinished; may require takeover after lease expiry"}),
        )
    }
}

fn validate_manifest(row: &Document) -> Result<bool> {
    if row.get_i64("schema").ok() != Some(1)
        || row.get_str("kind").ok() != Some("manifest")
        || row.get_str("migration").ok() != Some(activity_indexes::ID)
        || row.get_str("checksum").ok() != Some(activity_indexes::checksum().as_str())
        || row.keys().any(|key| {
            ![
                "_id",
                "schema",
                "kind",
                "migration",
                "checksum",
                "completed",
                "completion_attempt",
            ]
            .contains(&key.as_str())
        })
    {
        return Err("migration manifest checksum or shape drift");
    }
    let completed = row
        .get_bool("completed")
        .map_err(|_| "migration completion malformed")?;
    if completed != row.contains_key("completion_attempt")
        || (completed && row.get_str("completion_attempt").is_err())
    {
        return Err("migration completion receipt missing or premature");
    }
    Ok(completed)
}

fn validate_lock(row: &Document) -> Result<()> {
    if row.get_i64("schema").ok() != Some(1)
        || row.get_str("kind").ok() != Some("lease")
        || row.get_i64("fence").map_or(true, |value| value < 0)
        || row.get_str("holder").is_err()
        || row.get_datetime("expires_at").is_err()
        || row.get_datetime("acquired_at").is_err()
        || row.keys().any(|key| {
            ![
                "_id",
                "schema",
                "kind",
                "fence",
                "holder",
                "expires_at",
                "acquired_at",
            ]
            .contains(&key.as_str())
        })
    {
        return Err("migration lease malformed");
    }
    Ok(())
}
