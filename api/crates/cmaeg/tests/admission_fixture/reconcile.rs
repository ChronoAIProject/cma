//! Final independent reads after the caller supplies process finality separately.
use super::{config::private_json, ledger::hash, Shared};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

pub async fn observe(shared: Arc<Shared>) -> Result<Value, &'static str> {
    let _single = shared
        .run_lock
        .try_lock()
        .map_err(|_| "another fixture case is active")?;
    if !shared
        .ledger
        .lock()
        .map_err(|_| "fixture ledger lock")?
        .closed
    {
        return Err("close ledger before final reconciliation");
    }
    let events = shared
        .events
        .lock()
        .map_err(|_| "fixture event lock")?
        .values()
        .filter(|event| event.signed.is_some())
        .map(|event| event.identity.clone())
        .collect::<Vec<_>>();
    let mut rows = Vec::new();
    for identity in events {
        let observation = tokio::time::timeout(
            Duration::from_secs(3),
            shared
                .oracle
                .observe(&identity, shared.config.replicas_enabled),
        )
        .await;
        rows.push(match observation {
            Ok(Ok(value))=>value,
            Ok(Err(_))=>json!({"event_sha256":hash(identity.event_id.as_str().as_bytes()),"oracle_error":"read_failed"}),
            Err(_)=>json!({"event_sha256":hash(identity.event_id.as_str().as_bytes()),"oracle_error":"read_timeout"}),
        });
    }
    let ledger = shared
        .ledger
        .lock()
        .map_err(|_| "fixture ledger lock")?
        .snapshot();
    let value = json!({"schema":"eg_fixture_reconciliation_v1","events":rows,"ledger":ledger,
        "status":"OBSERVED_NOT_ACCEPTANCE","gateway_process_finality":"requires_outer_evidence"});
    // A re-read is a new immutable receipt, never a replacement for prior uncertainty.
    let id = uuid::Uuid::new_v4().to_string();
    private_json(
        &shared
            .config
            .private_output
            .join(format!("reconciliation-{id}.json")),
        &value,
    )?;
    Ok(value)
}
