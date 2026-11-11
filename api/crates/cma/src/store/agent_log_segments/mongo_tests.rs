use super::collections::collection;
use super::*;
use crate::store::agent_log_segments as segments;
use crate::store::agents::agent_replacement_fields;
use crate::store::{agent_log::log_document, sandbox_mongo::MongoSandboxStore};
use futures_util::TryStreamExt;
use layer2_core::sandbox::{
    store::{AgentLogStore, AgentStore},
    Agent, AgentLogEntry, AgentLogKind,
};
use mongodb::{
    bson::{self, doc, Document, RawDocumentBuf},
    Client,
};
const NOW: &str = "2026-11-01T00:00:00Z";
mod fixtures;
use fixtures::*;
mod audit_bounds;
mod audit_movement;
mod audit_output;
mod audit_tests;
mod audit_unknown;
mod audit_validation;
mod authority;
mod bounds;
mod capture_bounds;
mod capture_segmented_tests;
mod capture_tests;
mod capture_transaction_tests;
mod ranges;
mod retention;
mod schema_controls;
mod schema_output;
mod schema_recovery_migration;
mod schema_tests;
mod snapshot;
