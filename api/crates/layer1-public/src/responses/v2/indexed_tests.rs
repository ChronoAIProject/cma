use super::*;
use crate::responses::v2::{projection, stream_pages::indexed as stream_page};
use layer2_core::sandbox::{
    agent_output_index::*,
    store::{
        indexed::*, AgentResponseLogStore, AgentResponseLogWindow, SandboxStoreError, WorktreeStore,
    },
    Agent, AgentEventStreamSnapshot, AgentLogEntry, AgentLogEvidenceChange, AgentLogKind,
    AgentLogProjectionState, AgentOutputObservation, AgentReadError, AgentReadService,
    AgentVisibleLogEntry,
};
use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[path = "indexed_tests/fixtures.rs"]
mod fixtures;
use fixtures::*;
#[path = "indexed_tests/authority.rs"]
mod authority;
#[path = "indexed_tests/large_history.rs"]
mod large_history;
#[path = "indexed_tests/pagination.rs"]
mod pagination;
#[path = "indexed_tests/parity.rs"]
mod parity;
#[path = "indexed_tests/provider_failure.rs"]
mod provider_failure;

#[path = "indexed_tests/history.rs"]
mod history;
#[path = "indexed_tests/integrity.rs"]
mod integrity;

#[path = "indexed_tests/http.rs"]
mod http;

#[path = "indexed_tests/legacy_bounds.rs"]
mod legacy_bounds;

#[path = "indexed_tests/shared.rs"]
mod shared;

#[path = "indexed_tests/combined_observation.rs"]
mod combined_observation;
