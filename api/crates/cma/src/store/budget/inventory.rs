//! Explicit observation of retained turn records, without accounting mutations.
mod authority;
mod cli;
#[cfg(test)]
mod mongo_tests;
mod projection;
mod snapshot;
#[cfg(test)]
mod tests;

pub(crate) use cli::{command, is_command};
use mongodb::{Client, Database};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, time::Duration};

type Result<T> = std::result::Result<T, &'static str>;
const INVALID: &str = "turn_inventory_invalid_record";
const LIMIT: &str = "turn_inventory_limit_exceeded";
const READ: &str = "turn_inventory_read_failed";
const COLLECTIONS: [&str; 2] = [super::COLLECTION, "agents"];

#[derive(Clone, Copy)]
pub(in crate::store) struct Limits {
    page_size: u32,
    rows: usize,
    pages: usize,
    source_bytes: usize,
    output_bytes: usize,
    timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            page_size: 128,
            rows: 100_000,
            pages: 4096,
            source_bytes: 64 * 1024 * 1024,
            output_bytes: 16 * 1024 * 1024,
            timeout: Duration::from_secs(60),
        }
    }
}

pub(in crate::store) struct CompleteInventory {
    bytes: Vec<u8>,
    selected: usize,
    settled: usize,
    outcomes: BTreeMap<&'static str, usize>,
}

impl CompleteInventory {
    fn receipt(&self) -> String {
        json!({
            "format": "turn-budget-inventory-receipt-v1",
            "artifact_sha256": digest(&self.bytes),
            "selected": self.selected, "settled": self.settled,
            "unresolved": self.selected - self.settled, "linkages": self.outcomes,
            "database_writes": false, "recovery_authorized": false,
        })
        .to_string()
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(in crate::store) async fn observe(
    client: &Client,
    database: &Database,
    limits: Limits,
    #[cfg(test)] hook: &dyn PageHook,
) -> Result<CompleteInventory> {
    let max = Limits::default();
    if limits.page_size == 0
        || limits.page_size > max.page_size
        || limits.rows > max.rows
        || limits.pages > max.pages
        || limits.source_bytes > max.source_bytes
        || limits.output_bytes > max.output_bytes
        || limits.timeout > max.timeout
    {
        return Err(LIMIT);
    }
    tokio::time::timeout(limits.timeout, async {
        authority::verify(database).await?;
        let scope = authority::scope(database).await?;
        let mut session = client
            .start_session()
            .snapshot(true)
            .await
            .map_err(|_| READ)?;
        let started = layer2_core::now_rfc3339();
        let scan = snapshot::read(
            database,
            &mut session,
            limits,
            #[cfg(test)]
            hook,
        )
        .await?;
        let cut = session.snapshot_time().ok_or(READ)?;
        if authority::scope(database).await? != scope {
            return Err("turn_inventory_scope_changed");
        }
        authority::verify(database).await?;
        let session_bytes = mongodb::bson::serialize_to_vec(session.id()).map_err(|_| READ)?;
        let payload = json!({
            "format": "turn-budget-inventory-v1", "source_kind": "mongo_snapshot",
            "observed_start": started, "observed_end": layer2_core::now_rfc3339(),
            "snapshot": {"time": cut.time, "increment": cut.increment,
                "session_sha256": digest(&session_bytes), "scope_sha256": scope},
            "scope": "string_turn_keys_in_model_budget_reservations",
            "excluded": ["command_copies_and_unstarted_commands", "auxiliary_requests",
                "owner_and_platform_aggregates", "sandbox_funding", "external_liabilities",
                "records_outside_the_turn_key_namespace"],
            "eof": true, "selected": scan.selected, "settled": scan.settled,
            "unresolved": scan.rows, "linkages": scan.outcomes,
            "source_sha256": scan.source_sha256,
            "saved_integer_encoding": "decimal_strings",
            "database_writes": false, "recovery_authorized": false,
            "financial_reconciliation": "not_performed",
        });
        let bytes = serde_json::to_vec(&payload).map_err(|_| INVALID)?;
        if bytes.len() > limits.output_bytes {
            return Err(LIMIT);
        }
        Ok(CompleteInventory {
            bytes,
            selected: scan.selected,
            settled: scan.settled,
            outcomes: scan.outcomes,
        })
    })
    .await
    .map_err(|_| "turn_inventory_timeout")?
}

#[cfg(test)]
#[async_trait::async_trait]
pub(in crate::store) trait PageHook: Send + Sync {
    async fn after_page(&self, _page: usize) -> Result<()> {
        Ok(())
    }
}
#[cfg(test)]
#[async_trait::async_trait]
impl PageHook for () {}
