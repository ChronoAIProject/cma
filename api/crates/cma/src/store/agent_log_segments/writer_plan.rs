//! Caller-owned schema and admission wiring for bounded publication planning.
use super::Head;
use crate::store::sandbox_mongo::DOCUMENT_SCHEMA_VERSION;
use layer2_core::sandbox::{store::SandboxStoreError, AgentLogEntry};

pub(super) use cma_am_agent_log::writer_plan::{
    plan, EncodedBatch, PublicationPlan, RolloverPermission, SourceFormat,
};

pub(super) fn encode_batch(
    head: &Head,
    entries: &[AgentLogEntry],
) -> Result<EncodedBatch, SandboxStoreError> {
    EncodedBatch::new(head, entries, DOCUMENT_SCHEMA_VERSION)
}

pub(in crate::store) fn needs_normal_rollover(
    head: &Head,
    entries: &[AgentLogEntry],
) -> Result<bool, SandboxStoreError> {
    cma_am_agent_log::writer_plan::needs_normal_rollover(head, entries, DOCUMENT_SCHEMA_VERSION)
}

#[cfg(test)]
use super::{digest, MAX_BYTES, MAX_SEQUENCE};
#[cfg(test)]
use crate::store::{agent_log::log_document, agent_log_integrity::AppendCapacity};
#[cfg(test)]
use cma_am_agent_log::writer_plan::{DOCUMENT_BYTES, NORMAL_BYTES, STOP_BYTES, STOP_COUNT};
#[cfg(test)]
use mongodb::bson;

#[cfg(test)]
#[path = "writer_plan_tests.rs"]
mod tests;
