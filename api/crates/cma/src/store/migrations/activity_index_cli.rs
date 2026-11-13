//! Explicit production inspection and online Activity index-build commands.
//!
//! These commands are separate from startup and HTTP reads. Sizing is read-only;
//! the build path uses the durable migration lease and manifest with a bounded DDL.

use std::time::{Duration, Instant};

use mongodb::{
    bson::{doc, Document},
    options::ClientOptions,
    Client, Database,
};
use serde_json::{json, Value};

use super::{
    activity_inventory, activity_page_index, activity_plan, ledger, steps::Step, Result, IO,
};

const COLLECTION: &str = "activity";
const SIZE_DEADLINE: Duration = Duration::from_secs(30);
const DEFAULT_BUILD_TIMEOUT_SECONDS: u64 = 2_100;
const MIN_BUILD_TIMEOUT_SECONDS: u64 = 60;
const MAX_BUILD_TIMEOUT_SECONDS: u64 = 3_600;
const PROGRESS_INTERVAL: Duration = Duration::from_secs(30);
const LEASE_RENEWAL_INTERVAL: Duration = Duration::from_secs(5);

enum Operation {
    Size,
    Build {
        checksum: String,
        image: String,
        timeout_seconds: u64,
    },
}

pub(super) fn is_command(arguments: &[String]) -> bool {
    matches!(
        arguments.first().map(String::as_str),
        Some("--activity-index-size") | Some("--activity-index-build")
    )
}

pub(super) async fn command(arguments: &[String]) -> Result<Value> {
    let operation = parse(arguments)?;
    match operation {
        Operation::Size => tokio::time::timeout(SIZE_DEADLINE, size())
            .await
            .map_err(|_| "activity sizing deadline exceeded; no write was attempted")?,
        Operation::Build {
            checksum,
            image,
            timeout_seconds,
        } => tokio::time::timeout(
            Duration::from_secs(timeout_seconds.saturating_add(60)),
            build(&checksum, &image, timeout_seconds),
        )
        .await
        .map_err(|_| "activity index deadline exceeded; inspect index and migration status")?,
    }
}

fn parse(arguments: &[String]) -> Result<Operation> {
    match arguments
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["--activity-index-size"] => Ok(Operation::Size),
        ["--activity-index-build", checksum, image] => Ok(Operation::Build {
            checksum: (*checksum).to_owned(),
            image: (*image).to_owned(),
            timeout_seconds: DEFAULT_BUILD_TIMEOUT_SECONDS,
        }),
        ["--activity-index-build", checksum, image, timeout] => Ok(Operation::Build {
            checksum: (*checksum).to_owned(),
            image: (*image).to_owned(),
            timeout_seconds: parse_timeout(timeout)?,
        }),
        _ => Err("unsupported Activity index command or arguments"),
    }
}

fn parse_timeout(value: &str) -> Result<u64> {
    let seconds = value
        .parse::<u64>()
        .map_err(|_| "Activity index timeout must be an integer number of seconds")?;
    if !(MIN_BUILD_TIMEOUT_SECONDS..=MAX_BUILD_TIMEOUT_SECONDS).contains(&seconds) {
        return Err("Activity index timeout must be between 60 and 3600 seconds");
    }
    Ok(seconds)
}

async fn open_database() -> Result<(
    Client,
    Database,
    cma_service_auth::mongodb_authority::MongoAuthorityMode,
)> {
    let name = std::env::var("CHRONO_MONGODB_DATABASE").map_err(|_| "database is required")?;
    validate_database(&name)?;
    let uri = std::env::var("CHRONO_MONGODB_URI").map_err(|_| "database URI is required")?;
    let mode = cma_service_auth::mongodb_authority::MongoAuthorityMode::from_env(
        "CHRONO_MONGODB_AUTHORITY_MODE",
    )?;
    let mut options = ClientOptions::parse(uri)
        .await
        .map_err(|_| "database configuration")?;
    options.server_selection_timeout = Some(IO);
    options.connect_timeout = Some(IO);
    let client = Client::with_options(options).map_err(|_| "database configuration")?;
    Ok((client.clone(), client.database(&name), mode))
}

fn validate_database(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 63
        || name
            .chars()
            .any(|c| c.is_whitespace() || "/\\.\"$\0".contains(c))
    {
        return Err("invalid database name");
    }
    Ok(())
}

async fn size() -> Result<Value> {
    let (_client, database, mode) = open_database().await?;
    let status = database
        .run_command(doc! {"connectionStatus":1,"showPrivileges":true})
        .await
        .map_err(|_| "database authority unavailable")?;
    crate::store::isolation::verify_activity_index_authority(&status, database.name(), mode, false)
        .map_err(|_| "Activity sizing authority is insufficient")?;
    let stats = database
        .run_command(doc! {"collStats":COLLECTION,"maxTimeMS":5000_i32})
        .await
        .map_err(|_| "Activity collection metadata unavailable")?;
    let indexes = database
        .run_command(doc! {"listIndexes":COLLECTION,"cursor":{"batchSize":64},"maxTimeMS":5000_i32})
        .await
        .map_err(|_| "Activity index inventory unavailable")?;
    let explain = explain_first_page(&database).await?;
    Ok(json!({
        "schema":"activity-index-sizing-v1",
        "collection":COLLECTION,
        "read_only":true,
        "server_bound_ms":5000,
        "metadata": {
            "document_count": number(&stats, "count")?,
            "logical_bytes": number(&stats, "size")?,
            "storage_bytes": number(&stats, "storageSize")?,
            "index_bytes": number(&stats, "totalIndexSize")?,
        },
        "indexes": sanitize_indexes(&indexes)?,
        "first_page_explain": explain,
    }))
}

async fn explain_first_page(database: &Database) -> Result<Value> {
    let query = doc! {
        "find":COLLECTION,
        "filter":{},
        "sort":{"recorded_at":-1,"_id":-1},
        "limit":51_i64,
        "batchSize":52_i32,
        "maxTimeMS":5000_i32,
        "projection":{
            "_id":1,"document_schema_version":1,"recorded_at":1,
            "record.id":1,"record.at":1,"record.actor":1,"record.action":1,
            "record.target":1,"record.outcome":1,
        },
    };
    let explained = database
        .run_command(doc! {"explain":query,"verbosity":"executionStats","maxTimeMS":5000_i32})
        .await
        .map_err(|_| "Activity first-page explain unavailable")?;
    let planner = explained
        .get_document("queryPlanner")
        .map_err(|_| "Activity explain planner malformed")?;
    let stats = explained
        .get_document("executionStats")
        .map_err(|_| "Activity explain execution stats malformed")?;
    let plan = planner
        .get_document("winningPlan")
        .map_err(|_| "Activity explain winning plan malformed")?;
    Ok(json!({
        "winning_plan": activity_plan::sanitize(plan),
        "execution_time_ms": number(stats, "executionTimeMillis")?,
        "returned": number(stats, "nReturned")?,
        "keys_examined": number(stats, "totalKeysExamined")?,
        "documents_examined": number(stats, "totalDocsExamined")?,
    }))
}

fn sanitize_indexes(response: &Document) -> Result<Vec<Value>> {
    let cursor = response
        .get_document("cursor")
        .map_err(|_| "Activity index cursor malformed")?;
    if cursor_id(cursor)? != 0 {
        return Err("Activity index inventory exceeds bound");
    }
    cursor
        .get_array("firstBatch")
        .map_err(|_| "Activity index batch malformed")?
        .iter()
        .map(|value| {
            let index = value
                .as_document()
                .ok_or("Activity index entry malformed")?;
            let name = index
                .get_str("name")
                .map_err(|_| "Activity index name malformed")?;
            let key = index
                .get_document("key")
                .map_err(|_| "Activity index key malformed")?;
            Ok(json!({
                "name":name,
                "key":serde_json::to_value(key).map_err(|_| "Activity index key encoding")?,
                "unique":index.get_bool("unique").unwrap_or(false),
                "sparse":index.get_bool("sparse").unwrap_or(false),
                "hidden":index.get_bool("hidden").unwrap_or(false),
                "has_partial_filter":index.contains_key("partialFilterExpression"),
                "has_collation":index.contains_key("collation"),
            }))
        })
        .collect()
}

fn cursor_id(cursor: &Document) -> Result<i64> {
    cursor
        .get_i64("id")
        .or_else(|_| cursor.get_i32("id").map(i64::from))
        .map_err(|_| "Activity index cursor id malformed")
}

fn number(document: &Document, key: &str) -> Result<i64> {
    document
        .get_i64(key)
        .or_else(|_| document.get_i32(key).map(i64::from))
        .map_err(|_| "Activity sizing number malformed")
}

async fn build(checksum: &str, image: &str, timeout_seconds: u64) -> Result<Value> {
    if checksum != activity_page_index::checksum() {
        return Err("Activity index manifest checksum mismatch");
    }
    let source = option_env!("CHRONO_BUILD_SHA").unwrap_or("development");
    let identity = ledger::Identity::new(source, image)?;
    let (client, database, mode) = open_database().await?;
    let status = database
        .run_command(doc! {"connectionStatus":1,"showPrivileges":true})
        .await
        .map_err(|_| "database authority unavailable")?;
    crate::store::isolation::verify_activity_index_authority(&status, database.name(), mode, true)
        .map_err(|_| "Activity index build authority is insufficient")?;
    crate::store::sandbox_mongo::require_transaction_topology(&database)
        .await
        .map_err(|_| "Activity index build requires a supported replica set or mongos")?;
    let ledger = ledger::Ledger::for_step(client, database.clone(), Step::ActivityPage);
    ledger.preflight().await?;
    ledger.initialize().await?;
    if ledger.completed().await? {
        if !activity_inventory::strict_pending(&database)
            .await?
            .is_empty()
        {
            return Err("completed Activity index manifest drifted");
        }
        return Ok(json!({
            "migration":Step::ActivityPage.id(),
            "outcome":"already_completed",
            "online":true,
        }));
    }
    let lease = ledger.claim(&identity).await?;
    tracing::info!(
        migration = Step::ActivityPage.id(),
        phase = "inventory",
        timeout_seconds,
        "Activity index build admitted"
    );
    let outcome = async {
        let pending = activity_inventory::strict_pending(&database).await?;
        if pending.len() > 1 {
            return Err("Activity index manifest has no single pending index");
        }
        if pending.is_empty() {
            tracing::info!(
                migration = Step::ActivityPage.id(),
                phase = "verified",
                created = false,
                "Activity index already exists; completing reconciled manifest"
            );
            return Ok(0);
        }
        ledger.renew(&lease).await?;
        let started = Instant::now();
        tracing::info!(
            migration = Step::ActivityPage.id(),
            phase = "build_started",
            timeout_seconds,
            "Activity index online build started"
        );
        let created = create_online(
            &ledger,
            &lease,
            &database,
            pending[0].clone(),
            timeout_seconds,
            started,
        )
        .await?;
        let remaining = activity_inventory::strict_pending(&database).await?;
        if !remaining.is_empty() {
            return Err("Activity index postcondition unavailable");
        }
        tracing::info!(
            migration = Step::ActivityPage.id(),
            phase = "verified",
            elapsed_ms = started.elapsed().as_millis() as u64,
            "Activity index online build verified"
        );
        Ok(i64::from(created))
    }
    .await;
    ledger.finish(&lease, outcome).await?;
    let created = outcome?;
    Ok(json!({
        "migration":Step::ActivityPage.id(),
        "outcome":"completed",
        "created_indexes":created,
        "online":true,
        "timeout_seconds":timeout_seconds,
        "progress":"inventory,build_started,verified emitted as sanitized log phases",
    }))
}

async fn create_online(
    ledger: &ledger::Ledger,
    lease: &ledger::Lease,
    database: &Database,
    index: Document,
    timeout_seconds: u64,
    started: Instant,
) -> Result<bool> {
    let timeout_ms = i32::try_from(timeout_seconds.saturating_mul(1_000))
        .map_err(|_| "Activity index timeout is too large")?;
    let command = async {
        database
            .run_command(doc! {
                "createIndexes":COLLECTION,
                "indexes":[index],
                "maxTimeMS":timeout_ms,
                "writeConcern":{"w":"majority","wtimeout":timeout_ms},
                // Modern MongoDB builds indexes online; omit removed `background`.
                "comment":"cma-activity-page-index-v1",
            })
            .await
    };
    tokio::pin!(command);
    let mut progress = tokio::time::interval(PROGRESS_INTERVAL);
    let mut renewal = tokio::time::interval(LEASE_RENEWAL_INTERVAL);
    // Skip the immediate progress tick so heartbeats mean the command is in flight.
    progress.tick().await;
    let result = loop {
        tokio::select! {
            result = &mut command => {
                break result.map_err(|_| "Activity index build outcome uncertain; inspect and resume")?;
            }
            _ = progress.tick() => {
                tracing::info!(
                    migration = Step::ActivityPage.id(),
                    phase = "build_progress",
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    timeout_seconds,
                    "Activity index online build is still running"
                );
            }
            _ = renewal.tick() => {
                // Renew long builds so another executor cannot claim the migration.
                ledger.renew(lease).await?;
            }
        }
    };
    if result.contains_key("writeConcernError") {
        return Err("Activity index acknowledgement uncertain; inspect and resume");
    }
    let before = number(&result, "numIndexesBefore")?;
    let after = number(&result, "numIndexesAfter")?;
    if before < 0 || after < before || after > before + 1 {
        return Err("Activity index result count inconsistent");
    }
    tracing::info!(
        migration = Step::ActivityPage.id(),
        phase = "build_finished",
        elapsed_ms = started.elapsed().as_millis() as u64,
        created = after > before,
        "Activity index build command returned"
    );
    Ok(after > before)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_keeps_sizing_read_only_and_bounds_build_timeout() {
        assert!(matches!(
            parse(&["--activity-index-size".into()]),
            Ok(Operation::Size)
        ));
        assert!(matches!(
            parse(&[
                "--activity-index-build".into(),
                "a".repeat(64),
                "ghcr.io/chronoaiproject/cma-api@sha256:".to_owned() + &"b".repeat(64),
                "60".into(),
            ]),
            Ok(Operation::Build {
                timeout_seconds: 60,
                ..
            })
        ));
        assert!(parse_timeout("59").is_err());
        assert!(parse_timeout("3601").is_err());
        assert_eq!(parse_timeout("2100").unwrap(), 2100);
    }

    #[test]
    fn size_output_helpers_reject_unbounded_index_cursors() {
        assert!(sanitize_indexes(&doc! {
            "cursor":{"id":1_i64,"firstBatch":[]}
        })
        .is_err());
        assert_eq!(cursor_id(&doc! {"id":0_i32}).unwrap(), 0);
    }
}
