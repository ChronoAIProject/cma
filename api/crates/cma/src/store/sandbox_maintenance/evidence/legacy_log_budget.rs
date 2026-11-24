//! Limit the cost of proving inactivity from retained Agent logs.

use layer2_core::sandbox::{
    store::{SandboxStoreError, MAX_SANDBOX_LIFECYCLE_AGENTS},
    Agent,
};
use mongodb::{bson::doc, options::Hint, ClientSession};

use super::{capture_segmented, scoped::CapturedActivities, ApplyError, MongoSandboxStore};

pub(in crate::store::sandbox_maintenance) const MAX_LEGACY_ROWS: i64 = 256;
pub(in crate::store::sandbox_maintenance) const MAX_LEGACY_BYTES: i64 = 4 * 1024 * 1024;

pub(in crate::store::sandbox_maintenance) async fn within(
    store: &MongoSandboxStore,
    agents: &[Agent],
    sources: Option<&CapturedActivities>,
    session: &mut ClientSession,
) -> Result<bool, ApplyError> {
    let mut ids = Vec::new();
    let mut expected_rows = 0_u64;
    for agent in agents.iter().filter(|agent| agent.status.is_live()) {
        if sources.is_some_and(|sources| sources.contains(agent)) {
            continue;
        }
        // The dispatch hint predates the snapshot. A concurrent v2 adoption
        // must reach the existing format refusal before any legacy head cap.
        capture_segmented(store, agent, session).await?;
        if ids.len() == MAX_SANDBOX_LIFECYCLE_AGENTS {
            return Ok(exceeded("legacy_agents"));
        }
        expected_rows = expected_rows.saturating_add(agent.conversation.last_seq);
        ids.push(agent.id.as_str());
    }
    if expected_rows > MAX_LEGACY_ROWS as u64 {
        return Ok(exceeded("expected_rows"));
    }
    if ids.is_empty() {
        return Ok(true);
    }
    // Count retained rows too. A stale head cannot bound an integrity scan.
    // The shared transaction keeps this input bound through the actual proof.
    let mut totals = store
        .agent_log
        .aggregate([
            doc! { "$match": { "agent_id": { "$in": ids } } },
            doc! { "$limit": MAX_LEGACY_ROWS + 1 },
            doc! { "$group": {
                "_id": null,
                "rows": { "$sum": 1_i64 },
                "bytes": { "$sum": { "$toLong": { "$bsonSize": "$$ROOT" } } },
            } },
        ])
        .hint(Hint::Name("agent_log_sequence".into()))
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    if !totals.advance(session).await.map_err(ApplyError::Driver)? {
        return Ok(true);
    }
    let row = totals.deserialize_current().map_err(ApplyError::Driver)?;
    let total = |field| {
        row.get_i64(field)
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| {
                ApplyError::Domain(SandboxStoreError::Unavailable(
                    "legacy inactivity log budget returned invalid totals".into(),
                ))
            })
    };
    let rows = total("rows")?;
    let bytes = total("bytes")?;
    if rows > MAX_LEGACY_ROWS {
        return Ok(exceeded("retained_rows"));
    }
    if bytes > MAX_LEGACY_BYTES {
        return Ok(exceeded("bytes"));
    }
    Ok(true)
}

fn exceeded(reason: &'static str) -> bool {
    metrics::counter!("cma_auto_pause_legacy_log_budget_total", "reason" => reason).increment(1);
    tracing::debug!(parent: None, reason, "legacy Agent log budget prevents inactivity pause");
    false
}
