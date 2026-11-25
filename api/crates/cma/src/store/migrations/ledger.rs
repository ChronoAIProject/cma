//! Majority-durable attempts, atomically admitted with a renewable server-time lease.
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, DateTime, Document, Timestamp},
    options::{ReadConcern, ReturnDocument},
    Client, Collection, Database,
};
use serde_json::{json, Value};

use super::{
    steps::{unsupported, Step},
    Result, IO,
};
use crate::store::sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome};

pub(super) const COLLECTION: &str = "agent_manager_migrations";
const LOCK: &str = "runner";
const LEASE_MS: i64 = 30_000;

mod abort;
mod completion;

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
                .split_once("@sha256:")
                .is_some_and(|(repository, digest)| {
                    (repository.ends_with("/cma-agent-manager") || repository.ends_with("/cma-api"))
                        && hash(digest, 64)
                })
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
    pub(super) step: Step,
}

impl Ledger {
    #[cfg(test)]
    pub(super) fn new(client: Client, database: Database) -> Self {
        Self::for_step(client, database, Step::Activity)
    }

    pub(super) fn for_step(client: Client, database: Database, step: Step) -> Self {
        Self {
            rows: super::super::durability::majority_collection(&database, COLLECTION),
            client,
            database,
            step,
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
        for step in Step::ALL {
            if let Some(row) = self
                .rows
                .find_one(doc! {"_id":step.manifest()})
                .max_time(IO)
                .await
                .map_err(|_| "migration manifest unavailable")?
            {
                validate_manifest(&row, step)?;
            }
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
        self.abort_barrier(None).await?;
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
        let manifest = doc! {"_id":self.step.manifest(),"schema":1_i64,"kind":"manifest",
        "migration":self.step.id(),"checksum":self.step.checksum(),"completed":false};
        if self.rows.insert_one(manifest).await.is_err() {
            let row = self
                .rows
                .find_one(doc! {"_id":self.step.manifest()})
                .max_time(IO)
                .await
                .map_err(|_| "manifest insertion outcome uncertain")?
                .ok_or("manifest insertion outcome uncertain")?;
            validate_manifest(&row, self.step)?;
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
            .find_one(doc! {"_id":self.step.manifest()})
            .max_time(IO)
            .await
            .map_err(|_| "migration completion unavailable")?;
        let Some(row) = row else {
            return Ok(false);
        };
        if !validate_manifest(&row, self.step)? {
            return Ok(false);
        }
        let receipt = row
            .get_str("completion_attempt")
            .map_err(|_| "completion receipt missing")?;
        if self
            .rows
            .find_one(doc! {"_id":receipt,"schema":1_i64,"kind":"attempt",
            "migration":self.step.id(),"checksum":self.step.checksum(),"outcome":"completed"})
            .max_time(IO)
            .await
            .map_err(|_| "completion receipt unavailable")?
            .is_none()
        {
            return Err("completion receipt missing");
        }
        Ok(true)
    }

    pub(super) async fn claim(&self, identity: &Identity) -> Result<Lease> {
        self.claim_inner(identity, None).await
    }

    async fn claim_inner(&self, identity: &Identity, abort_fence: Option<i64>) -> Result<Lease> {
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
        if self
            .abort_barrier(Some(&mut session))
            .await?
            .is_some_and(|row| row.get_str("state").ok() != Some("settled"))
        {
            return Err(abort::UNSETTLED);
        }
        let manifest = self
            .rows
            .find_one(doc! {"_id":self.step.manifest()})
            .session(&mut session)
            .await
            .map_err(|_| "migration manifest unavailable")?
            .ok_or("migration manifest missing")?;
        if validate_manifest(&manifest, self.step)? {
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
        "migration":self.step.id(),"checksum":self.step.checksum(),
        "source":&identity.source,"image":&identity.image,"fence":fence,
        "started_at":started_at,"outcome":"running"};
        self.rows
            .insert_one(row.clone())
            .session(&mut session)
            .await
            .map_err(|_| "migration attempt unavailable")?;
        let lease = Lease {
            attempt: attempt.clone(),
            fence,
        };
        if let Some(previous_fence) = abort_fence {
            if previous_fence.checked_add(1) != Some(fence) {
                return Err("abort predecessor changed; reconcile before retry");
            }
            self.register_abort(&lease, &mut session).await?;
        }
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
        Ok(lease)
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
            .find(doc! {"kind":"attempt", "migration":self.step.id()})
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
                || changed.is_some_and(|count| !(0..=self.step.max_changes()).contains(&count))
            {
                return Err("migration effect count malformed");
            }
            attempts.push(json!({"fence":row.get_i64("fence").map_err(|_| "migration fence malformed")?,
                "outcome":outcome,"source":source,"image":image,(self.step.effect_field()):changed,
                "started_at":row.get_datetime("started_at")
                    .map_err(|_| "migration start malformed")?.timestamp_millis(),
                "finished_at":row.get_datetime("finished_at").ok().map(|time|time.timestamp_millis())}));
        }
        Ok(
            json!({"migration":self.step.id(),"checksum":self.step.checksum(),
            "completed":self.completed().await?,"recent_attempts":attempts,
            "history_limit":20,"running_means":"unfinished; takeover requires lease expiry and no unsettled destructive DDL",
            "destructive_ddl":self.abort_barrier(None).await?.map(|row| json!({
                "migration":row.get_str("migration").unwrap(), "fence":row.get_i64("fence").unwrap(),
                "state":row.get_str("state").unwrap(),
                "recovery_required":row.get_str("state").unwrap() != "settled",
            }))}),
        )
    }
}

fn validate_manifest(row: &Document, step: Step) -> Result<bool> {
    if row.get_i64("schema").ok() != Some(1)
        || row.get_str("kind").ok() != Some("manifest")
        || row.get_str("migration").ok() != Some(step.id())
        || row.get_str("checksum").ok() != Some(step.checksum().as_str())
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
