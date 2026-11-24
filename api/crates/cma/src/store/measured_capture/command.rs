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
const UNAVAILABLE: &str = "measured capture is unavailable or its write outcome is unresolved";
const AUTHORITY: &str = "measured capture requires isolated append-only custody authority";
const MAX_BINDINGS: usize = 16;
const MAX_OBSERVATIONS: usize = 2048;
type Result<T> = std::result::Result<T, &'static str>;

#[cfg(test)]
mod tests;

pub(crate) async fn run(arguments: &[String]) -> Result<Value> {
    let (sandbox, range) = invocation(arguments, usage_reports::now())?;
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
    let source = std::env::var("CHRONO_USAGE_METRICS_URL").map_err(|_| UNAVAILABLE)?;
    let client = Client::with_uri_str(uri).await.map_err(|_| UNAVAILABLE)?;
    let source = MetricsClient::internal(&source).map_err(|_| UNAVAILABLE)?;
    let database = client.database(&db_name);
    tokio::time::timeout(Duration::from_secs(90), async {
        verify_authority(&database).await?;
        execute(&database, &source, sandbox, range).await
    })
    .await
    .map_err(|_| UNAVAILABLE)?
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
    verify_shared_schema(&status, database.name(), &custody).map_err(|_| AUTHORITY)
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
    let from = from.parse::<u64>().map_err(|_| INVALID)?;
    let to = to.parse::<u64>().map_err(|_| INVALID)?;
    if command != "--measured-capture" || !valid || from >= to || to - from > 900 || to > now {
        return Err(INVALID);
    }
    Ok((sandbox, Range { from, to }))
}

async fn execute(
    database: &Database,
    source: &MetricsClient,
    sandbox: &str,
    range: Range,
) -> Result<Value> {
    if !measured_custody_ready(database).await? {
        return Err("measured custody requires its explicit schema migration");
    }
    let (revision, bindings) = binding_snapshot(database, sandbox, range).await?;
    let mut observations = Vec::new();
    let mut incomplete = 0;
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
            series.retain(|row| has_physical_identity(binding, row, metric));
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
        return Err("measured capture has no attributable observations in the window");
    }
    let store = MeasuredCapture::new(database);
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
