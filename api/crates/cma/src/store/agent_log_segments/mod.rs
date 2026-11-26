//! Versioned segmented log readers and gated atomic publication.
pub(in crate::store) mod active_proof;
pub(in crate::store) mod admission;
pub(in crate::store) mod audit;
pub(in crate::store) mod audit_command;
pub(in crate::store) mod capture;
mod codec;
#[cfg(test)]
mod codec_tests;
mod collections;
pub(in crate::store) mod conversation;
pub(in crate::store) mod current_publication;
pub(in crate::store) use collections::SegmentCollections;
pub(in crate::store) mod command;
pub(in crate::store) mod command_evidence;
pub(in crate::store) mod command_worker;
mod digest;
mod telemetry;
pub(in crate::store) mod terminal;
pub(in crate::store) use digest::{incarnation, incarnation_fields, valid_digest};
mod envelope;
pub(in crate::store) mod ordinary;
pub(in crate::store) mod projection;
pub(in crate::store) mod publication;
mod reads;
pub(in crate::store) mod retention;
pub(in crate::store) mod schema;
mod source_keys;
pub(in crate::store) mod transaction_scope;
pub(in crate::store) mod writer_admission;
pub(in crate::store) mod writer_bootstrap;
pub(in crate::store) mod writer_commit;
pub(in crate::store) mod writer_control;
pub(in crate::store) mod writer_gate;
pub(in crate::store) mod writer_plan;
pub(in crate::store) mod writer_projection;
pub(in crate::store) mod writer_reconcile;

pub(in crate::store) use codec::{Head, Seal};
pub(in crate::store) use envelope::{read_format, require_agent_version, validate_identity};
use layer2_core::sandbox::store::SandboxStoreError;
pub(in crate::store) use reads::{
    read_head, retained_turn, verify_terminal_seal, visit_range, LogRange,
};

pub(in crate::store) const HEADS: &str = "agent_log_heads";
pub(in crate::store) const SEGMENTS: &str = "agent_log_segments";
pub(in crate::store) const PUBLICATIONS: &str = "agent_log_publications";
pub(in crate::store) const PROJECTIONS: &str = "agent_log_projections";
pub(in crate::store) const EVIDENCE: &str = "agent_log_evidence";
pub(in crate::store) const AUDITS: &str = "agent_log_audits";
pub(in crate::store) const RECOVERY_WORK: &str = "agent_capacity_recovery_work";
pub(in crate::store) const COLLECTIONS: &[&str] = &[
    HEADS,
    SEGMENTS,
    PUBLICATIONS,
    PROJECTIONS,
    EVIDENCE,
    AUDITS,
    writer_gate::COLLECTION,
    writer_admission::COLLECTION,
    RECOVERY_WORK,
];
pub(in crate::store) const FORMAT: &str = "segmented_v1";
pub(in crate::store) const READER: &str = "agent_log_segments_v1";
pub(in crate::store) const MIGRATION: &str = "2026-11-05-am-log-segment-readers-v1";
pub(in crate::store) const MAX_SEQUENCE: i64 = 9_007_199_254_740_991;
pub(in crate::store) const MAX_ENTRIES: i64 = 10_128;
pub(in crate::store) const MAX_BYTES: i64 = 65 * 1024 * 1024;
const MAX_SEAL_BYTES: usize = 1024 * 1024;

fn corrupt() -> SandboxStoreError {
    SandboxStoreError::Corrupt("Agent log segment identity, format or integrity is invalid".into())
}

fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    <Option<T> as serde::Deserialize>::deserialize(deserializer)
}

#[cfg(test)]
pub(in crate::store) mod mongo_tests;

#[cfg(test)]
mod durable_control_fixture;
