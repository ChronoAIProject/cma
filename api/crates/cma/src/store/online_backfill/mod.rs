//! Bounded, durable observations of existing Agent Manager backfills.
use mongodb::{
    bson::{doc, DateTime, Document, Timestamp},
    ClientSession, Collection, Database,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use super::{
    durability::majority_collection,
    sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome},
};

mod cli;
mod snapshot;
mod sources;
pub(crate) use cli::{command, is_command};
use snapshot::Snapshot;
use sources::Cohort;

const COLLECTION: &str = "online_backfill_progress";
const PAGE_SIZE: i64 = 12;
const IO: std::time::Duration = std::time::Duration::from_secs(5);
const UNAVAILABLE: &str = "backfill progress is unavailable";
const INVALID: &str = "backfill progress record is invalid";
type Result<T> = std::result::Result<T, &'static str>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Observation {
    Changed,
    Retained,
    Skipped,
    Pending,
    Error,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Counts {
    scanned: i64,
    changed: i64,
    retained: i64,
    skipped: i64,
    pending: i64,
    errors: i64,
}

impl Counts {
    fn valid(&self) -> bool {
        [
            self.changed,
            self.retained,
            self.skipped,
            self.pending,
            self.errors,
        ]
        .into_iter()
        .try_fold(0_i64, |total, count| match count {
            0.. => total.checked_add(count),
            _ => None,
        }) == Some(self.scanned)
    }

    fn observe(&mut self, observation: Observation) -> Result<()> {
        let count = match observation {
            Observation::Changed => &mut self.changed,
            Observation::Retained => &mut self.retained,
            Observation::Skipped => &mut self.skipped,
            Observation::Pending => &mut self.pending,
            Observation::Error => &mut self.errors,
        };
        *count = count.checked_add(1).ok_or(INVALID)?;
        self.scanned = self.scanned.checked_add(1).ok_or(INVALID)?;
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Scan {
    #[serde(rename = "_id")]
    run: String,
    document_schema_version: i64,
    cohort: String,
    scope_digest: String,
    started_at: DateTime,
    revision: i64,
    snapshot_time: Timestamp,
    cursor: Option<String>,
    counts: Counts,
    unobserved: i64,
    finished_at: Option<DateTime>,
    routing_initialized: Option<bool>,
    completion_proved: bool,
}

impl Scan {
    fn scope(&self, cohort: &Cohort) -> Result<()> {
        if self.cohort != cohort.name() || self.scope_digest != cohort.digest()? {
            return Err("backfill run belongs to a different cohort");
        }
        Ok(())
    }

    fn document(&self) -> Result<Document> {
        mongodb::bson::serialize_to_document(self).map_err(|_| INVALID)
    }

    fn public_view(&self) -> Value {
        json!({
            "run": self.run,
            "cohort": self.cohort,
            "scope_digest": self.scope_digest,
            "revision": self.revision,
            "started_at": self.started_at,
            "finished_at": self.finished_at,
            "counts": self.counts,
            "unobserved": self.unobserved,
            "routing_initialized": self.routing_initialized,
            "complete": self.completion_proved,
        })
    }
}

fn rows(database: &Database) -> Collection<Document> {
    majority_collection(database, COLLECTION)
}

fn decode(document: Document) -> Result<Scan> {
    let scan: Scan = mongodb::bson::deserialize_from_document(document).map_err(|_| INVALID)?;
    if scan.document_schema_version != 1
        || Uuid::parse_str(&scan.run).is_err()
        || scan.revision < 0
        || !scan.counts.valid()
        || scan.unobserved < 0
        || scan.snapshot_time.time == 0
        || scan.cursor.as_ref().is_some_and(String::is_empty)
        || (scan.finished_at.is_some() && scan.cursor.is_some())
        || (scan.routing_initialized.is_some() && scan.finished_at.is_none())
        || (scan.completion_proved
            && (scan.finished_at.is_none()
                || scan.counts.pending != 0
                || scan.counts.errors != 0
                || scan.unobserved != 0
                || scan.routing_initialized == Some(false)))
    {
        return Err(INVALID);
    }
    Ok(scan)
}

async fn current(database: &Database, run: &str) -> Result<Scan> {
    read(database, run)
        .await?
        .ok_or("backfill run does not exist")
}

async fn read(database: &Database, run: &str) -> Result<Option<Scan>> {
    let row = rows(database)
        .find_one(doc! {"_id": run})
        .max_time(IO)
        .await
        .map_err(|_| UNAVAILABLE)?;
    row.map(decode).transpose()
}

async fn start(database: &Database, run: &str, cohort: &Cohort) -> Result<Value> {
    if let Some(scan) = read(database, run).await? {
        scan.scope(cohort)?;
        return Ok(scan.public_view());
    }
    let mut session = database
        .client()
        .start_session()
        .snapshot(true)
        .await
        .map_err(|_| UNAVAILABLE)?;
    let _: Option<Document> = database
        .collection(cohort.source())
        .find_one(doc! {})
        .projection(doc! {"_id": 1})
        .max_time(IO)
        .session(&mut session)
        .await
        .map_err(|_| UNAVAILABLE)?;
    let snapshot_time = session.snapshot_time().ok_or(UNAVAILABLE)?;
    let scan = Scan {
        run: run.into(),
        document_schema_version: 1,
        cohort: cohort.name().into(),
        scope_digest: cohort.digest()?,
        started_at: DateTime::now(),
        revision: 0,
        snapshot_time,
        cursor: None,
        counts: Counts::default(),
        unobserved: 0,
        finished_at: None,
        routing_initialized: None,
        completion_proved: false,
    };
    let mut fields = scan.document()?;
    fields.remove("_id");
    rows(database)
        .update_one(doc! {"_id": run}, doc! {"$setOnInsert": fields})
        .upsert(true)
        .await
        .map_err(|_| UNAVAILABLE)?;
    let scan = current(database, run).await?;
    scan.scope(cohort)?;
    Ok(scan.public_view())
}

async fn step(
    database: &Database,
    run: &str,
    cohort: &Cohort,
    expected_revision: i64,
) -> Result<Value> {
    let mut session = database
        .client()
        .start_session()
        .await
        .map_err(|_| UNAVAILABLE)?;
    start_transaction(&mut session)
        .await
        .map_err(|_| UNAVAILABLE)?;
    let scan = advance(database, run, cohort, expected_revision, &mut session).await;
    let scan = match scan {
        Ok(scan) => scan,
        Err(error) => {
            use super::transaction_terminal::GuardedTerminal;
            if session.abort_guarded().await.is_err() {
                tracing::warn!("backfill progress transaction abort was not acknowledged");
            }
            return Err(error);
        }
    };
    match commit_transaction(&mut session).await {
        CommitOutcome::Committed => Ok(scan.public_view()),
        _ => Err(
            "backfill checkpoint was not acknowledged. Read the current revision before retrying",
        ),
    }
}

async fn advance(
    database: &Database,
    run: &str,
    cohort: &Cohort,
    expected_revision: i64,
    session: &mut ClientSession,
) -> Result<Scan> {
    let collection = rows(database);
    let mut scan = decode(
        collection
            .find_one(doc! {"_id": run})
            .session(&mut *session)
            .await
            .map_err(|_| UNAVAILABLE)?
            .ok_or("backfill run does not exist")?,
    )?;
    scan.scope(cohort)?;
    if scan.revision > expected_revision || scan.finished_at.is_some() {
        return Ok(scan);
    }
    if scan.revision != expected_revision {
        return Err("backfill checkpoint revision does not match");
    }
    let cursor = scan.cursor.as_deref().unwrap_or("");
    let mut selection = doc! {"_id": {"$type": "string", "$gt": cursor}};
    cohort.restrict(&mut selection)?;
    // A stable ID bound still permits insertion or deletion behind the cursor.
    // Pin both the membership and the migration evidence to the same cut.
    let snapshot = Snapshot::new(database, scan.snapshot_time);
    let page = snapshot.page(cohort.source(), selection, PAGE_SIZE).await?;
    let mut last = cursor;
    for source_row in &page {
        let id = source_row.get_str("_id").map_err(|_| INVALID)?;
        if id <= last {
            return Err(INVALID);
        }
        scan.counts
            .observe(cohort.observe(&snapshot, source_row).await?)?;
        last = id;
    }
    if page.is_empty() {
        scan.cursor = None;
        scan.finished_at = Some(DateTime::now());
        scan.unobserved = (cohort.selected_count() - scan.counts.scanned).max(0);
        scan.routing_initialized = cohort.routing_initialized(&snapshot).await?;
        scan.completion_proved = scan.counts.pending == 0
            && scan.counts.errors == 0
            && scan.unobserved == 0
            && scan.routing_initialized != Some(false);
    } else {
        scan.cursor = Some(last.to_owned());
    }
    let previous_revision = scan.revision;
    scan.revision = scan.revision.checked_add(1).ok_or(INVALID)?;
    let result = collection
        .replace_one(
            doc! {"_id": run, "revision": previous_revision},
            scan.document()?,
        )
        .session(&mut *session)
        .await
        .map_err(|_| UNAVAILABLE)?;
    if result.matched_count != 1 {
        return Err(
            "backfill checkpoint advanced concurrently. Read the current revision before retrying",
        );
    }
    Ok(scan)
}

#[cfg(test)]
mod tests;
