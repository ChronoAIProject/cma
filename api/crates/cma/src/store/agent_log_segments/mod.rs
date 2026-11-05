//! Immutable logical ranges over the original Agent log BSON rows.
//!
//! The Agent envelope is the discriminator: missing segmented state is never a
//! reason to fall back to legacy history. No activation writer lives here.

pub(super) mod answer;
pub(super) mod approval_pages;
pub(super) mod approvals;
mod audit;
mod codec;
#[cfg(test)]
mod codec_tests;
mod collections;
mod command;
mod consumers;
pub(super) mod current_state;
pub(super) mod digest;
mod envelope;
pub(super) mod history_context;
pub(super) mod projection;
mod projection_reads;
pub(super) mod publication;
mod reads;
mod schema;
pub(super) mod task_status;

#[cfg(test)]
pub(in crate::store) use audit::install as install_projection;
pub(crate) use audit::run_unit as audit_unit;
#[cfg(test)]
pub(in crate::store) use audit::{claim_for_test, finish_for_test};
pub(super) use codec::{Head, Seal};
pub(super) use collections::SegmentCollections;
pub(crate) use command::command;
pub(super) use consumers::{command_evidence, title_transcript};
pub(super) use envelope::validate_identity;
pub(super) use envelope::{read_format, require_agent_version};
pub(super) use projection_reads::{final_answer, read_projection};
pub(super) use reads::{read_head, visit_range, LogRange};
pub(crate) use schema::migrate;
pub(super) use schema::verify;

use layer2_core::sandbox::store::SandboxStoreError;

pub(super) const HEADS: &str = "agent_log_heads";
pub(super) const SEGMENTS: &str = "agent_log_segments";
pub(super) const PUBLICATIONS: &str = "agent_log_publications";
pub(super) const PROJECTIONS: &str = "agent_log_projections";
pub(super) const EVIDENCE: &str = "agent_log_evidence";
pub(super) const AUDITS: &str = "agent_log_audits";
pub(super) const COLLECTIONS: &[&str] =
    &[HEADS, SEGMENTS, PUBLICATIONS, PROJECTIONS, EVIDENCE, AUDITS];
pub(super) const FORMAT: &str = "segmented_v1";
pub(super) const READER: &str = "agent_log_segments_v1";
pub(super) const MIGRATION: &str = "2026-11-05-am-log-segment-readers-v1";
pub(super) const MAX_SEQUENCE: i64 = 9_007_199_254_740_991;
pub(super) const MAX_ENTRIES: i64 = 10_128;
pub(super) const MAX_BYTES: i64 = 65 * 1024 * 1024;
pub(super) const MAX_SEAL_BYTES: usize = 1024 * 1024;

fn corrupt() -> SandboxStoreError {
    // Do not include raw history, IDs, or user-derived hashes in diagnostics.
    SandboxStoreError::Corrupt("Agent log segment identity, format or integrity is invalid".into())
}

fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    <Option<T> as serde::Deserialize>::deserialize(deserializer)
}
