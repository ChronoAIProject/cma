//! Read-only discovery over canonical Agent IDs; one bounded page per invocation.
//! Candidates are inputs to exact-cohort census, never recovery authorization.
use super::{digest, files, Member, Request as CensusRequest, Result, INVALID, MAX_SOURCE};
use futures_util::TryStreamExt;
use layer2_core::sandbox::ledger_census::LedgerFailureCause;
use mongodb::{
    bson::{doc, Document},
    options::{Collation, Hint},
    Client, Database,
};
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};

const PAGE_SIZE: u32 = 64;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store) struct Request {
    pub format: String,
    pub after_agent_id: Option<String>,
}

impl Request {
    fn validate(&self) -> Result<()> {
        if self.format != "runtime-ledger-discovery-request-v1"
            || self
                .after_agent_id
                .as_deref()
                .is_some_and(|id| !valid_id(id))
        {
            return Err(INVALID);
        }
        Ok(())
    }
}

#[derive(Debug, Serialize)]
pub(in crate::store) struct Unknown {
    agent_id: String,
    reason: &'static str,
}

#[derive(Debug, Serialize)]
pub(in crate::store) struct Page {
    format: &'static str,
    id_scope: &'static str,
    snapshot_scope: &'static str,
    observed_at: String,
    request_sha256: String,
    source_sha256: String,
    scanned: usize,
    non_failed: usize,
    other_failed: usize,
    pub unknown: Vec<Unknown>,
    pub candidates: CensusRequest,
    pub next: Option<String>,
    canonical_id_range_exhausted: bool,
    recovery_authorized: bool,
    writes: bool,
}

pub(crate) async fn command(request: &Path, output: &Path) -> Result<()> {
    let bytes = files::read(request)?;
    let request: Request = serde_json::from_slice(&bytes).map_err(|_| INVALID)?;
    request.validate()?;
    files::require_parent(output)?;
    let page = tokio::time::timeout(Duration::from_secs(60), async {
        let (client, database) = super::connect().await?;
        observe(&client, &database, &request, digest(&bytes)).await
    })
    .await
    .map_err(|_| INVALID)??;
    let bytes = serde_json::to_vec(&page).map_err(|_| INVALID)?;
    files::publish(output, &bytes)?;
    println!("{}", summary(&page, &bytes));
    Ok(())
}

pub(in crate::store) async fn observe(
    client: &Client,
    database: &Database,
    request: &Request,
    request_sha256: String,
) -> Result<Page> {
    request.validate()?;
    let mut session = client.start_session().await.map_err(|_| INVALID)?;
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .map_err(|_| INVALID)?;
    let result = async {
        // BSON type bracketing deliberately scopes this cursor to string Agent
        // IDs. It is not proof of whole-fleet/schema completeness or the original
        // 63-member cohort; refresh every original identity with exact census.
        let filter = doc! {"_id":{"$gt":request.after_agent_id.as_deref().unwrap_or("")}};
        let mut cursor = database
            .collection::<Document>("agents")
            .find(filter)
            .sort(doc! {"_id":1})
            .hint(Hint::Name("_id_".into()))
            .collation(Collation::builder().locale("simple").build())
            .projection(
                doc! {"_id":1,"document_schema_version":1,"status":1,"sandbox_id":1,
                "record.id":1,"record.status":1,"record.sandbox_id":1,"record.failure":1,
                crate::store::agent_log_integrity::QUARANTINE_FIELD:1},
            )
            // A single batch may stop at Mongo's byte bound before the row
            // limit. Exhaustion is proved only after consuming the cursor.
            .limit(i64::from(PAGE_SIZE))
            .batch_size(PAGE_SIZE)
            .max_time(Duration::from_secs(5))
            .session(&mut session)
            .await
            .map_err(|_| INVALID)?;
        let mut rows = Vec::new();
        let mut used = 0;
        while let Some(row) = cursor
            .stream(&mut session)
            .try_next()
            .await
            .map_err(|_| INVALID)?
        {
            used += mongodb::bson::serialize_to_vec(&row)
                .map_err(|_| INVALID)?
                .len();
            if used > MAX_SOURCE || rows.len() == PAGE_SIZE as usize {
                return Err(INVALID);
            }
            rows.push(row);
        }
        classify(request, request_sha256, rows)
    }
    .await;
    // There is no committed transaction, DDL, runtime call or lifecycle write.
    session.abort_transaction().await.map_err(|_| INVALID)?;
    result
}

fn classify(request: &Request, request_sha256: String, rows: Vec<Document>) -> Result<Page> {
    if rows.len() > PAGE_SIZE as usize {
        return Err(INVALID);
    }
    let mut page = Page {
        format: "runtime-ledger-discovery-v1",
        observed_at: layer2_core::now_rfc3339(),
        id_scope: "canonical_string_ids",
        snapshot_scope: "one_page_only",
        request_sha256,
        source_sha256: digest(
            &mongodb::bson::serialize_to_vec(&doc! {"rows":&rows}).map_err(|_| INVALID)?,
        ),
        scanned: rows.len(),
        non_failed: 0,
        other_failed: 0,
        unknown: Vec::new(),
        candidates: CensusRequest {
            format: "runtime-ledger-census-request-v1".into(),
            agents: Vec::new(),
        },
        next: None,
        canonical_id_range_exhausted: rows.len() < PAGE_SIZE as usize,
        recovery_authorized: false,
        writes: false,
    };
    let mut previous = request.after_agent_id.as_deref().unwrap_or("").to_owned();
    for row in rows {
        let id = row.get_str("_id").map_err(|_| INVALID)?;
        if !valid_id(id) || id <= previous.as_str() {
            return Err(INVALID);
        }
        previous = id.into();
        match candidate(&row) {
            Ok(Some(member)) => page.candidates.agents.push(member),
            Ok(None) if row.get_str("status").ok() == Some("failed") => page.other_failed += 1,
            Ok(None) => page.non_failed += 1,
            Err(reason) => page.unknown.push(Unknown {
                agent_id: id.into(),
                reason,
            }),
        }
    }
    if !page.canonical_id_range_exhausted {
        page.next = Some(previous);
    }
    Ok(page)
}

fn candidate(row: &Document) -> std::result::Result<Option<Member>, &'static str> {
    if row.contains_key(crate::store::agent_log_integrity::QUARANTINE_FIELD) {
        return Err("quarantined_preserve");
    }
    let schema = row
        .get_i64("document_schema_version")
        .ok()
        .or_else(|| row.get_i32("document_schema_version").ok().map(i64::from));
    if schema != Some(crate::store::sandbox_mongo::DOCUMENT_SCHEMA_VERSION) {
        return Err("unsupported_schema_preserve");
    }
    let record = row
        .get_document("record")
        .map_err(|_| "unreadable_preserve")?;
    if record.get_str("id").ok() != row.get_str("_id").ok() {
        return Err("identity_mismatch_preserve");
    }
    let status = row.get_str("status").map_err(|_| "unreadable_preserve")?;
    if record.get_str("status").ok() != Some(status) {
        return Err("projection_mismatch_preserve");
    }
    let sandbox = row
        .get_str("sandbox_id")
        .map_err(|_| "unreadable_preserve")?;
    if !valid_id(sandbox) || record.get_str("sandbox_id").ok() != Some(sandbox) {
        return Err("lineage_mismatch_preserve");
    }
    serde_json::from_value::<layer2_core::sandbox::AgentStatus>(serde_json::Value::String(
        status.into(),
    ))
    .map_err(|_| "unknown_status_preserve")?;
    if status != "failed" {
        return Ok(None);
    }
    let failure = record
        .get_document("failure")
        .map_err(|_| "failure_unknown_preserve")?;
    let detail = failure
        .get_str("detail")
        .map_err(|_| "failure_unknown_preserve")?;
    if LedgerFailureCause::from_diagnostic(detail) == LedgerFailureCause::OtherOrUnknown {
        return Ok(None);
    }
    Ok(Some(Member {
        agent_id: row
            .get_str("_id")
            .map_err(|_| "unreadable_preserve")?
            .into(),
        sandbox_id: sandbox.into(),
    }))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256 && id.trim() == id && !id.chars().any(char::is_control)
}

fn summary(page: &Page, bytes: &[u8]) -> serde_json::Value {
    serde_json::json!({"format":"runtime-ledger-discovery-summary-v1",
        "artifact_sha256":digest(bytes), "source_sha256":page.source_sha256,
        "scanned":page.scanned, "candidates":page.candidates.agents.len(),
        "unknown":page.unknown.len(), "non_failed":page.non_failed, "other_failed":page.other_failed,
        "more":page.next.is_some(), "id_scope":page.id_scope, "snapshot_scope":page.snapshot_scope,
        "writes":false, "recovery_authorized":false})
}

#[cfg(test)]
mod tests;
