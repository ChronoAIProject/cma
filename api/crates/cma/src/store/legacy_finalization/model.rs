//! Wire shape retained only to validate historical retirement receipts.
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HistorySeal {
    pub schema_version: u16,
    pub policy: HistoryPolicy,
    pub sandbox_id: String,
    pub sandbox_created_at: String,
    pub tree_fingerprint: String,
    pub storage_retention_sha256: String,
    pub reference_count_at_seal: u64,
    pub references_sha256_at_seal: String,
    pub reference_ids_at_seal: Vec<String>,
    pub material: MaterialInventory,
    pub sealed_at: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HistoryPolicy {
    pub schema_version: u16,
    pub allocation_id: String,
    pub allocation_sha256: String,
    pub disposition: Retain,
    pub pinned_at_ms: i64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Retain {
    Retain,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MaterialInventory {
    pub agent_conversation_sha256: String,
    pub log_count: u64,
    pub log_sha256: String,
    pub commands: Vec<InventoryRow>,
    pub outbox: Vec<InventoryRow>,
    pub ancillary: Vec<InventoryRow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controls: Vec<InventoryRow>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct InventoryRow {
    pub collection: String,
    pub id: String,
    pub sha256: String,
}
