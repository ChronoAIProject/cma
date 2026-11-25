//! Bounded opt-in capture of a retained Sandbox metric window.
use super::*;
use cma_service_auth::mongodb_authority::{verify_shared_schema, MongoAuthorityMode};
use layer2_core::sandbox_metrics::{
    measured_evidence::capture_observations, Binding, BindingKind, ProofStatus, Range,
};
use layer3_adapter::sandbox_metrics::{MetricsClient, RawPhysicalSeries, Selection, SourceMetric};
use serde_json::{json, Value};

use crate::store::{
    isolation, migrations::measured_custody_ready, sandbox_metric_bindings, usage_reports,
};

const INVALID: &str =
    "measured capture requires one Sandbox and a past window of at most 900 seconds";
const PREVIEW_INVALID: &str =
    "measured preview requires one physical sample ID and a past window of at most 900 seconds";
const PREVIEW_UNAVAILABLE: &str =
    "measured preview could not establish a complete custody snapshot";
const UNAVAILABLE: &str = "measured capture is unavailable or its write outcome is unresolved";
const AUTHORITY: &str = "measured capture requires isolated append-only custody authority";
const MAX_BINDINGS: usize = 16;
const MAX_OBSERVATIONS: usize = 2048;
type Result<T> = std::result::Result<T, &'static str>;

#[cfg(test)]
mod tests;

pub(crate) async fn run(arguments: &[String]) -> Result<Value> {
    if let [command] = arguments {
        if matches!(
            command.as_str(),
            "--measured-index-backfill" | "--measured-index-certify"
        ) {
            return index_command(command).await;
        }
    }
    if arguments.first().is_some_and(|value| {
        matches!(
            value.as_str(),
            "--measured-freeze"
                | "--measured-evidence-read"
                | "--measured-period-read"
                | "--measured-period-replay"
        )
    }) {
        return batch_command(arguments).await;
    }

    if arguments
        .first()
        .is_some_and(|value| value == "--measured-preview")
    {
        let (anchor, window) = preview_invocation(arguments, usage_reports::now())?;
        let database = open_database().await?;
        return tokio::time::timeout(Duration::from_secs(30), async {
            verify_authority(&database).await?;
            check_schema(&database).await?;
            capture_store(&database)?
                .preview(anchor, window)
                .await
                .map_err(|_| PREVIEW_UNAVAILABLE)
        })
        .await
        .map_err(|_| PREVIEW_UNAVAILABLE)?;
    }
    let (sandbox, range) = invocation(arguments, usage_reports::now())?;
    let database = open_database().await?;
    let source = std::env::var("CHRONO_USAGE_METRICS_URL").map_err(|_| UNAVAILABLE)?;
    let source = MetricsClient::internal(&source).map_err(|_| UNAVAILABLE)?;
    tokio::time::timeout(Duration::from_secs(90), async {
        verify_authority(&database).await?;
        let receipt = execute(&database, &source, sandbox, range).await?;
        if receipt["status"] == "no_attributable_observations" {
            eprintln!("{receipt}");
            return Err("measured capture has no attributable observations in the window");
        }
        Ok(receipt)
    })
    .await
    .map_err(|_| UNAVAILABLE)?
}

async fn batch_command(arguments: &[String]) -> Result<Value> {
    use super::ledger::{
        ExactPeriodReadRequest, FreezeRequest, MeasuredLedger, PeriodReadRequest, ReadRequest,
        SelectedWindow,
    };
    use crate::store::{ledger_census::files, migrations::measured_batches};
    use std::path::Path;
    let [command, input, output] = arguments else {
        return Err("measured evidence requires a private request and new output file");
    };
    if std::env::var("CHRONO_MEASURED_BATCH_ENABLED").as_deref() != Ok("true") {
        return Err("measured finalization is disabled");
    }
    let bytes = files::read(Path::new(input))?;
    let output = Path::new(output);
    files::require_parent(output)?;
    if output.exists() {
        return Err("measured evidence requires a new output file");
    }
    let database = open_database().await?;
    let (payload, receipt) = tokio::time::timeout(Duration::from_secs(45), async {
        verify_authority(&database).await?;
        let status = database.run_command(doc!{"connectionStatus":1,"showPrivileges":true}).await.map_err(|_| AUTHORITY)?;
        let batch = doc!{"privileges":[{"resource":{"db":database.name(),"collection": measured_batches::COLLECTION},"actions":isolation::MEASURED_CUSTODY_ACTIONS}]};
        verify_shared_schema(&status, database.name(), &batch).map_err(|_|AUTHORITY)?;
        check_schema(&database).await?;
        if !crate::store::migrations::measured_batches_ready(&database).await? {
            return Err("measured batch custody requires its explicit schema migration");
        }
        let mut ledger = MeasuredLedger::new(&database);
        if let Some(indexed) = indexed_store(&database)? {
            ledger = ledger.with_indexed(indexed);
        }
        if matches!(command.as_str(), "--measured-period-read" | "--measured-period-replay") {
            let windows = if command == "--measured-period-read" {
                let request: PeriodReadRequest = serde_json::from_slice(&bytes).map_err(|_| INVALID)?;
                ledger.read_period(request).await
            } else {
                let request: ExactPeriodReadRequest = serde_json::from_slice(&bytes).map_err(|_| INVALID)?;
                ledger.read_exact_period(request).await
            }.map_err(|_| "measured period evidence is unavailable or incomplete")?;
            let receipt = json!({"selected": windows.iter().map(|batch| SelectedWindow { revision: batch.revision(), reference: batch.reference() }).collect::<Vec<_>>()});
            return Ok((serde_json::to_vec(&windows).map_err(|_| UNAVAILABLE)?, receipt));
        }
        let evidence = match command.as_str() {
            "--measured-freeze" => {
                let request: FreezeRequest = serde_json::from_slice(&bytes).map_err(|_| INVALID)?;
                if request.source_cutoff_ms > usage_reports::now().checked_mul(1000).ok_or(INVALID)? { return Err(INVALID); }
                ledger.freeze(request).await
            }
            "--measured-evidence-read" => {
                let request: ReadRequest = serde_json::from_slice(&bytes).map_err(|_| INVALID)?;
                ledger.read(request).await
            }
            _ => return Err(INVALID),
        }.map_err(|_| "measured batch evidence is unavailable or conflicts with retained custody")?;
        Ok((serde_json::to_vec(&evidence).map_err(|_| UNAVAILABLE)?, json!({"evidence_id": evidence.evidence_id(), "evidence_sha256": evidence.evidence_sha256(), "revision": evidence.revision()})))
    }).await.map_err(|_|UNAVAILABLE)??;
    files::publish(output, &payload)?;
    Ok(receipt)
}

async fn open_database() -> Result<Database> {
    if std::env::var("CHRONO_MEASURED_CAPTURE_ENABLED").as_deref() != Ok("true") {
        return Err("measured capture is disabled");
    }
    if MongoAuthorityMode::from_env("CHRONO_MONGODB_AUTHORITY_MODE").map_err(|_| AUTHORITY)?
        != MongoAuthorityMode::Isolated
    {
        return Err(AUTHORITY);
    }
    let db_name = std::env::var("CHRONO_MONGODB_DATABASE").map_err(|_| UNAVAILABLE)?;
    let uri = std::env::var("CHRONO_MONGODB_URI").map_err(|_| UNAVAILABLE)?;
    let client = Client::with_uri_str(uri).await.map_err(|_| UNAVAILABLE)?;
    Ok(client.database(&db_name))
}

async fn verify_authority(database: &Database) -> Result<()> {
    let status = database
        .run_command(doc! {"connectionStatus": 1, "showPrivileges": true})
        .await
        .map_err(|_| AUTHORITY)?;
    isolation::validate(&status, database.name()).map_err(|_| AUTHORITY)?;
    let custody = doc! {"privileges": [{
        "resource": {"db": database.name(), "collection": COLLECTION},
        "actions": isolation::MEASURED_CUSTODY_ACTIONS,
    }]};
    verify_shared_schema(&status, database.name(), &custody).map_err(|_| AUTHORITY)?;
    use crate::store::migrations::measured_index::{BATCHES, MANIFESTS, PROJECTIONS};
    let indexed = doc! {"privileges": [
        {"resource": {"db":database.name(),"collection":PROJECTIONS},
         "actions":isolation::MEASURED_CUSTODY_ACTIONS},
        {"resource": {"db":database.name(),"collection":BATCHES},
         "actions":isolation::MEASURED_CUSTODY_ACTIONS},
        {"resource": {"db":database.name(),"collection":MANIFESTS},
         "actions":isolation::MEASURED_MANIFEST_ACTIONS},
    ]};
    let indexed_grant = verify_shared_schema(&status, database.name(), &indexed).is_ok();
    if indexed_store(database)?.is_some() {
        if !indexed_grant {
            return Err("measured index writer floor requires its matching isolated credential");
        }
        if !crate::store::migrations::measured_index::inspect(database)
            .await?
            .is_empty()
        {
            return Err("measured index requires its explicit schema migration");
        }
    } else {
        let manifest_grant = doc! {"privileges":[{"resource":{"db":database.name(),"collection":MANIFESTS},
        "actions":["find"]}]};
        if verify_shared_schema(&status, database.name(), &manifest_grant).is_ok()
            && indexed::started(database).await.map_err(|_| AUTHORITY)?
        {
            return Err("measured index generation forbids legacy capture and finalization");
        }
    }
    Ok(())
}

fn indexed_store(database: &Database) -> Result<Option<indexed::IndexedCustody>> {
    match std::env::var("CHRONO_MEASURED_INDEX_WRITER_FLOOR_SHA256") {
        Ok(floor) => indexed::IndexedCustody::new(database, floor)
            .map(Some)
            .map_err(|_| AUTHORITY),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(_) => Err(AUTHORITY),
    }
}

fn capture_store(database: &Database) -> Result<MeasuredCapture> {
    let store = MeasuredCapture::new(database);
    Ok(match indexed_store(database)? {
        Some(indexed) => store.with_indexed(indexed),
        None => store,
    })
}

// The floor identifies the independently documented exclusion and drain of
// v1-only writers. The operator must establish it before starting backfill.
async fn index_command(command: &str) -> Result<Value> {
    let database = open_database().await?;
    tokio::time::timeout(Duration::from_secs(300), async {
        verify_authority(&database).await?;
        check_schema(&database).await?;
        let indexed = indexed_store(&database)?.ok_or(AUTHORITY)?;
        if command == "--measured-index-backfill" {
            indexed.begin().await.map_err(|_| UNAVAILABLE)?;
            let (rows, exhausted) = indexed.advance().await.map_err(|_| UNAVAILABLE)?;
            Ok(json!({"projected_rows":rows, "exhausted":exhausted}))
        } else {
            let proof = indexed.certify().await.map_err(|_| UNAVAILABLE)?;
            serde_json::to_value(proof).map_err(|_| UNAVAILABLE)
        }
    })
    .await
    .map_err(|_| UNAVAILABLE)?
}

fn invocation(arguments: &[String], now: u64) -> Result<(&str, Range)> {
    let [command, sandbox, from, to] = arguments else {
        return Err(INVALID);
    };
    let valid = sandbox.starts_with("sbx_")
        && sandbox.len() > "sbx_".len()
        && sandbox.len() <= 128
        && sandbox
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    if command != "--measured-capture" || !valid {
        return Err(INVALID);
    }
    Ok((sandbox, bounded_range(from, to, now).ok_or(INVALID)?))
}

fn preview_invocation(arguments: &[String], now: u64) -> Result<(&str, Range)> {
    let [command, anchor, from, to] = arguments else {
        return Err(PREVIEW_INVALID);
    };
    if command != "--measured-preview"
        || anchor.len() != 64
        || !anchor
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(PREVIEW_INVALID);
    }
    Ok((anchor, bounded_range(from, to, now).ok_or(PREVIEW_INVALID)?))
}

fn bounded_range(from: &str, to: &str, now: u64) -> Option<Range> {
    let from = from.parse::<u64>().ok()?;
    let to = to.parse::<u64>().ok()?;
    (from < to && to - from <= 900 && to <= now).then_some(Range { from, to })
}

async fn check_schema(database: &Database) -> Result<()> {
    if !measured_custody_ready(database).await? {
        return Err("measured custody requires its explicit schema migration");
    }
    Ok(())
}

async fn execute(
    database: &Database,
    source: &MetricsClient,
    sandbox: &str,
    range: Range,
) -> Result<Value> {
    check_schema(database).await?;
    let (revision, bindings) = binding_snapshot(database, sandbox, range).await?;
    let mut observations = Vec::new();
    let mut incomplete = 0;
    let mut unattributed_series = 0_u64;
    let mut unattributed_samples = 0_u64;
    for binding in &bindings {
        for metric in SourceMetric::ALL
            .into_iter()
            .filter(|metric| metric.is_pvc() == (binding.kind == BindingKind::Home))
        {
            let active = binding.range(range.to).intersect(range);
            if active.from == active.to {
                incomplete += 1;
                continue;
            }
            let name = if metric.is_pvc() {
                Some(binding.pvc_name.as_str())
            } else {
                binding.pod_name.as_deref()
            }
            .ok_or(UNAVAILABLE)?;
            let selected = Selection {
                cluster: binding.cluster.clone(),
                namespace: binding.namespace.clone(),
                metric,
                names: vec![name.to_owned()],
                from: active.from,
                to: active.to,
            };
            let mut series = source
                .export_raw(&selected)
                .await
                .map_err(|_| UNAVAILABLE)?;
            series.retain(|row| {
                let verified = has_physical_identity(binding, row, metric);
                if !verified {
                    unattributed_series += 1;
                    unattributed_samples += row.samples().len() as u64;
                }
                verified
            });
            let mut captured = capture_observations(binding, revision, metric, &series, active)
                .map_err(|_| "measured capture could not attribute a source observation")?;
            if captured.is_empty() {
                incomplete += 1;
            }
            if observations.len() + captured.len() > MAX_OBSERVATIONS {
                return Err("measured capture exceeds its observation limit");
            }
            observations.append(&mut captured);
        }
    }
    if observations.is_empty() {
        return Ok(json!({
            "status": "no_attributable_observations", "inserted": 0,
            "replayed": 0, "disputed": 0, "incomplete_metric_intervals": incomplete,
            "bindings": bindings.len(), "unattributed_source_series": unattributed_series,
            "unattributed_source_samples": unattributed_samples,
        }));
    }
    let store = capture_store(database)?;
    let (mut inserted, mut replayed, mut disputed) = (0, 0, 0);
    for observation in &observations {
        match store.capture(observation).await.map_err(|_| UNAVAILABLE)? {
            Capture::Inserted => inserted += 1,
            Capture::Replay => replayed += 1,
            Capture::Disputed => disputed += 1,
        }
    }
    Ok(json!({
        "inserted": inserted, "replayed": replayed, "disputed": disputed,
        "incomplete_metric_intervals": incomplete, "bindings": bindings.len(),
        "unattributed_source_series": unattributed_series,
        "unattributed_source_samples": unattributed_samples,
    }))
}

fn has_physical_identity(binding: &Binding, row: &RawPhysicalSeries, metric: SourceMetric) -> bool {
    if metric.is_pvc() {
        row.label("pvc_uid") == Some(binding.pvc_uid.as_str())
    } else {
        row.label("pod_uid").is_some()
            && row.label("pod_uid") == binding.pod_uid.as_deref()
            && row.label("container_id").is_some()
            && row.label("container_id") == binding.runtime_container_id.as_deref()
    }
}

async fn binding_snapshot(
    database: &Database,
    sandbox: &str,
    range: Range,
) -> Result<(u64, Vec<Binding>)> {
    let mut session = database
        .client()
        .start_session()
        .snapshot(true)
        .await
        .map_err(|_| UNAVAILABLE)?;
    let control = database
        .collection::<Document>(usage_reports::CONTROL)
        .find_one(doc! {"_id": "observer"})
        .session(&mut session)
        .await
        .map_err(|_| UNAVAILABLE)?
        .ok_or(UNAVAILABLE)?;
    let revision = control
        .get_i64("revision")
        .ok()
        .and_then(|value| u64::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or(UNAVAILABLE)?;
    let from = i64::try_from(range.from).map_err(|_| INVALID)?;
    let to = i64::try_from(range.to).map_err(|_| INVALID)?;
    let mut rows = database
        .collection::<Document>(sandbox_metric_bindings::COLLECTION)
        .find(doc! {"sandbox_id": sandbox, "valid_from": {"$lt": to},
        "$or": [{"valid_until": null}, {"valid_until": {"$gt": from}}]})
        .sort(doc! {"valid_from": 1, "_id": 1})
        .limit((MAX_BINDINGS + 1) as i64)
        .session(&mut session)
        .await
        .map_err(|_| UNAVAILABLE)?;
    let mut bindings = Vec::<Binding>::new();
    while let Some(row) = rows
        .next(&mut session)
        .await
        .transpose()
        .map_err(|_| UNAVAILABLE)?
    {
        let binding: Binding = usage_reports::decode(&row).map_err(|_| UNAVAILABLE)?;
        if bindings.len() == MAX_BINDINGS
            || binding.sandbox_id != sandbox
            || binding.proof_status != ProofStatus::Verified
            || bindings
                .first()
                .is_some_and(|first| first.owner_subject != binding.owner_subject)
        {
            return Err(
                "measured capture could not establish one Sandbox owner and verified binding set",
            );
        }
        bindings.push(binding);
    }
    if bindings.is_empty() {
        return Err("measured capture has no retained Sandbox bindings in the window");
    }
    Ok((revision, bindings))
}
