//! Versioned segmented log readers and gated atomic publication.
pub(in crate::store) mod active_proof;
pub(in crate::store) mod admission;
pub(in crate::store) mod audit;
pub(in crate::store) mod audit_command;
pub(in crate::store) mod capture;
#[cfg(test)]
mod codec_tests;
mod collections;
pub(in crate::store) mod conversation;
pub(in crate::store) mod current_publication;
pub(in crate::store) use collections::SegmentCollections;
pub(in crate::store) mod command;
pub(in crate::store) mod command_evidence;
pub(in crate::store) mod command_worker;
use cma_am_agent_log::digest;
mod telemetry;
pub(in crate::store) mod terminal;
pub(in crate::store) use digest::{incarnation, incarnation_fields, valid_digest};
pub(in crate::store) mod ordinary;
pub(in crate::store) mod projection;
pub(in crate::store) mod publication;
mod reads;
pub(in crate::store) mod retention;
pub(in crate::store) mod schema;
pub(in crate::store) mod source_keys;
pub(in crate::store) mod transaction_scope;
pub(in crate::store) mod writer_admission;
pub(in crate::store) mod writer_bootstrap;
pub(in crate::store) mod writer_commit;
pub(in crate::store) mod writer_control;
pub(in crate::store) mod writer_gate;
pub(in crate::store) mod writer_plan;
pub(in crate::store) mod writer_projection;
pub(in crate::store) mod writer_reconcile;

pub(in crate::store) use cma_am_agent_log::{
    read_format, require_agent_version, validate_identity,
};
pub(in crate::store) use cma_am_agent_log::{Head, Seal};
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
#[cfg(test)]
use cma_am_agent_log::MAX_SEAL_BYTES;
use cma_am_agent_log::{corrupt, required_nullable};
pub(in crate::store) use cma_am_agent_log::{
    FORMAT, MAX_BYTES, MAX_ENTRIES, MAX_SEQUENCE, MIGRATION, READER,
};

#[cfg(test)]
pub(in crate::store) mod mongo_tests;

#[cfg(test)]
mod durable_control_fixture;
