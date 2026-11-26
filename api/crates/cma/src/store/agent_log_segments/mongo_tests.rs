use super::collections::collection;
use super::*;
use crate::store::agent_log_segments as segments;
use crate::store::agents::agent_replacement_fields;
use crate::store::{agent_log::log_document, sandbox_mongo::MongoSandboxStore};
use futures_util::TryStreamExt;
use layer2_core::sandbox::{
    store::{AgentLogStore, AgentStore, SandboxStoreError},
    Agent, AgentLogEntry, AgentLogKind,
};
use mongodb::{
    bson::{self, doc, Document, RawDocumentBuf},
    Client,
};
const NOW: &str = "2026-11-01T00:00:00Z";
pub(in crate::store) mod fixtures;
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
mod malformed_sequences;
mod range_bounds;
mod ranges;
mod recovery_shape_fixture;
mod response_readers;
mod retention;
mod schema_controls;
mod schema_inventory_drift;
mod schema_output;
mod schema_recovery_discovery;
mod schema_recovery_migration;
mod schema_recovery_shapes;
mod schema_tests;
mod snapshot;
