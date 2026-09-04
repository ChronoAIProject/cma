use serde::{Deserialize, Serialize};

/// Capture the complete sandbox home under one writer fence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointRequest {
    pub epoch: u64,
    #[serde(default)]
    pub exclusions: Vec<String>,
}

/// State of every agent writer when checkpoint capture returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriterState {
    Resumed,
    Held,
}

/// Sealed object-store checkpoint receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointReceipt {
    pub epoch: u64,
    pub object_reference: String,
    pub sha256: String,
    pub writers: WriterState,
}

/// Restore one previously sealed whole-home checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreRequest {
    pub object_reference: String,
    pub sha256: String,
}

/// One direct resource reading from the sandbox supervisor.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceReading {
    pub uptime_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_usage_usec: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_millis_limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_bytes_used: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_bytes_limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_bytes_peak: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stall_permille: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ceiling_hits: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allocation_failures: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kills: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub received_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent_bytes: Option<u64>,
}
