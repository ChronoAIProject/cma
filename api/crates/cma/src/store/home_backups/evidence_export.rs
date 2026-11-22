//! Explicit snapshot export; no service initialization or backup effects.
pub(super) mod authority;
pub(crate) mod cli;
pub(super) mod projection;

use super::{BACKUPS_COLLECTION, SCHEDULES_COLLECTION};
use mongodb::{
    bson::{doc, Document},
    options::{Collation, CollectionOptions, Hint, ReadConcern},
    Client, ClientSession, Database,
};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, time::Duration};

pub(super) type Result<T> = std::result::Result<T, &'static str>;
const COLLECTIONS: [&str; 3] = ["sandboxes", SCHEDULES_COLLECTION, BACKUPS_COLLECTION];
const MAX_OUTPUT: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy)]
pub(super) struct Limits {
    pub(super) page_size: u32,
    pub(super) pages: usize,
    pub(super) rows: usize,
    pub(super) source_bytes: usize,
    pub(super) output_bytes: usize,
    pub(super) timeout: Duration,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            page_size: 128,
            pages: 4096,
            rows: 100_000,
            source_bytes: 64 * 1024 * 1024,
            output_bytes: MAX_OUTPUT,
            timeout: Duration::from_secs(60),
        }
    }
}

#[derive(Clone, Serialize)]
pub(super) struct Identity {
    pub(super) source_revision_claim: String,
    pub(super) executable_sha256: String,
}
impl Identity {
    fn validate(&self) -> Result<()> {
        let hex = |s: &str, n| {
            s.len() == n
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        if !hex(&self.source_revision_claim, 40) || !hex(&self.executable_sha256, 64) {
            return Err("catalog_export_identity_required");
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct CollectionExport {
    pub(super) rows: Vec<Value>,
    count: usize,
    pub(super) pages: usize,
    eof: bool,
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Default)]
struct Budget {
    rows: usize,
    pages: usize,
    source_bytes: usize,
    output_bytes: usize,
}
impl Budget {
    fn row(&mut self, row: &Document, projected: &Value, limits: Limits) -> Result<()> {
        self.rows += 1;
        self.source_bytes += mongodb::bson::serialize_to_vec(row)
            .map_err(|_| "catalog_export_invalid_record")?
            .len();
        self.output_bytes += serde_json::to_vec(projected)
            .map_err(|_| "catalog_export_invalid_record")?
            .len();
        if self.rows > limits.rows
            || self.source_bytes > limits.source_bytes
            || self.output_bytes > limits.output_bytes
        {
            return Err("catalog_export_limit_exhausted");
        }
        Ok(())
    }
}

pub(super) async fn observe(
    client: &Client,
    database: &Database,
    identity: Identity,
    limits: Limits,
    #[cfg(test)] hook: &dyn PageHook,
) -> Result<Vec<u8>> {
    identity.validate()?;
    if limits.page_size == 0 || limits.page_size > 128 {
        return Err("catalog_export_invalid_limits");
    }
    tokio::time::timeout(limits.timeout, async {
        authority::verify(database).await?;
        super::schema_v1::require_prepared(database).await.map_err(|_| "catalog_export_schema_unprepared")?;
        crate::store::schema::index_contract_v1::inspect_collection(database, "sandboxes")
            .await.map_err(|_| "catalog_export_schema_unprepared")?.ok_or("catalog_export_schema_unprepared")?;
        let scope_sha256 = scope_identity(database).await?;
        let started = layer2_core::now_rfc3339();
        let mut session = client.start_session().snapshot(true).await.map_err(|_| "catalog_export_snapshot_unavailable")?;
        let mut collections = BTreeMap::new();
        let mut budget = Budget::default();
        for collection in COLLECTIONS {
            let rows = enumerate(database, collection, &mut session, limits, &mut budget,
                #[cfg(test)] hook,
            ).await?;
            collections.insert(collection, rows);
        }
        let cut = session.snapshot_time().ok_or("catalog_export_snapshot_unavailable")?;
        super::schema_v1::require_prepared(database).await.map_err(|_| "catalog_export_schema_unprepared")?;
        if scope_identity(database).await? != scope_sha256 {
            return Err("catalog_export_scope_changed");
        }
        let session_bytes = mongodb::bson::serialize_to_vec(session.id()).map_err(|_| "catalog_export_snapshot_unavailable")?;
        let payload = serde_json::json!({
            "format": "home-backup-catalog-payload-v1", "source_kind": "mongo_snapshot",
            "run_id": uuid::Uuid::new_v4().to_string(), "identity": identity,
            "provenance": "operator_claim_requires_external_verification",
            "observed_start": started, "observed_end": layer2_core::now_rfc3339(),
            "snapshot": {"time": cut.time, "increment": cut.increment, "session_sha256": digest(&session_bytes), "scope_sha256": scope_sha256},
            "scope": "all_records_in_three_collections", "page_size": limits.page_size, "collections": collections,
        });
        let payload_json = serde_json::to_string(&payload).map_err(|_| "catalog_export_encoding_failed")?;
        let bytes = serde_json::to_vec(&serde_json::json!({
            "format": "home-backup-catalog-export-v1", "payload_sha256": digest(payload_json.as_bytes()), "payload_json": payload_json,
        })).map_err(|_| "catalog_export_encoding_failed")?;
        if bytes.len() > limits.output_bytes { return Err("catalog_export_limit_exhausted"); }
        Ok(bytes)
    }).await.map_err(|_| "catalog_export_timeout")?
}

async fn scope_identity(database: &Database) -> Result<String> {
    let mut identity = Sha256::new();
    identity.update(database.name().as_bytes());
    for collection in COLLECTIONS {
        let result = database
            .run_command(doc! {"listCollections": 1, "filter": {"name": collection},
            "cursor": {"batchSize": 2}, "maxTimeMS": 5000})
            .await
            .map_err(|_| "catalog_export_schema_unprepared")?;
        let rows = crate::store::schema::index_contract_v1::entries(&result)
            .map_err(|_| "catalog_export_schema_unprepared")?;
        if rows.len() != 1
            || rows[0].get_str("name").ok() != Some(collection)
            || rows[0].get_str("type").ok() != Some("collection")
        {
            return Err("catalog_export_schema_unprepared");
        }
        let uuid = rows[0]
            .get_document("info")
            .map_err(|_| "catalog_export_schema_unprepared")?
            .get("uuid")
            .ok_or("catalog_export_schema_unprepared")?;
        if !matches!(uuid, mongodb::bson::Bson::Binary(_)) {
            return Err("catalog_export_schema_unprepared");
        }
        identity.update(
            mongodb::bson::serialize_to_vec(&doc! {"name": collection, "uuid": uuid.clone()})
                .map_err(|_| "catalog_export_schema_unprepared")?,
        );
    }
    Ok(format!("{:x}", identity.finalize()))
}

async fn enumerate(
    database: &Database,
    name: &str,
    session: &mut ClientSession,
    limits: Limits,
    budget: &mut Budget,
    #[cfg(test)] hook: &dyn PageHook,
) -> Result<CollectionExport> {
    let collection = database.collection_with_options::<Document>(
        name,
        CollectionOptions::builder()
            .read_concern(ReadConcern::snapshot())
            .build(),
    );
    let mut result = CollectionExport {
        rows: Vec::new(),
        count: 0,
        pages: 0,
        eof: false,
    };
    let mut last: Option<String> = None;
    let mut cursor = collection
        .find(doc! {})
        .sort(doc! {"_id": 1})
        .collation(Collation::builder().locale("simple").build())
        .hint(Hint::Name("_id_".into()))
        .batch_size(limits.page_size)
        .max_time(Duration::from_secs(5))
        .session(&mut *session)
        .await
        .map_err(|_| "catalog_export_read_failed")?;
    loop {
        budget.pages += 1;
        result.pages += 1;
        if budget.pages > limits.pages {
            return Err("catalog_export_limit_exhausted");
        }
        for _ in 0..limits.page_size {
            let Some(row) = cursor.next(&mut *session).await else {
                result.eof = true;
                break;
            };
            let row = row.map_err(|_| "catalog_export_read_failed")?;
            let id = row
                .get_str("_id")
                .map_err(|_| "catalog_export_invalid_identity")?;
            if id.is_empty()
                || last
                    .as_ref()
                    .is_some_and(|previous| id <= previous.as_str())
            {
                return Err("catalog_export_duplicate_or_unordered_identity");
            }
            let projected = projection::project(name, row.clone())?;
            budget.row(&row, &projected, limits)?;
            last = Some(id.to_owned());
            result.rows.push(projected);
        }
        #[cfg(test)]
        hook.after_page(name, result.pages).await?;
        if result.eof {
            break;
        }
    }
    result.count = result.rows.len();
    Ok(result)
}

#[cfg(test)]
#[async_trait::async_trait]
pub(super) trait PageHook: Send + Sync {
    async fn after_page(&self, _collection: &str, _page: usize) -> Result<()> {
        Ok(())
    }
}
#[cfg(test)]
#[async_trait::async_trait]
impl PageHook for () {}
