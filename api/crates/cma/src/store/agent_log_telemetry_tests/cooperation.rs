//! Complete-scan contracts and a metadata-only local scheduling reproduction.
use crate::store::{
    agent_log::log_document,
    agent_log_integrity::{scan_collection, ScanOptions, TransactionLogError},
    agent_log_telemetry::{isolated_test, recorded_execution, snapshot, ScanCaller},
    cooperative_budget::Policy,
    execution_sections::{Clock, SectionStats},
};
use layer2_core::{
    ids::AgentId,
    sandbox::store::SandboxStoreError,
    sandbox::{AgentLogEntry, AgentLogKind},
};
use mongodb::{
    bson::{doc, Document},
    Client, Collection, Database,
};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

#[path = "cooperation/benchmark.rs"]
mod benchmark;
#[path = "cooperation/cancellation.rs"]
mod cancellation;
#[path = "cooperation/fixture.rs"]
mod fixture;
#[path = "cooperation/invariants.rs"]
mod invariants;
#[path = "cooperation/observation.rs"]
mod observation;
#[path = "cooperation/quarantine.rs"]
mod quarantine;
