//! Resumable stale-turn discovery and sealed terminal settlement.

use std::sync::Arc;

use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::{bson::Document, Collection};

use super::{agent_ownership::OWNERS, durability::majority_collection, MongoSandboxStore};

mod evidence;
mod proof_budget;
mod scan;
mod settlement;

pub(crate) use evidence::{Candidate, Observation, QuietTurn};
pub(in crate::store) use proof_budget::find_with_proof_budget;
pub(crate) use proof_budget::ProofBudget;

pub(crate) struct StaleTurnStore {
    store: Arc<MongoSandboxStore>,
    state: Collection<Document>,
}

impl StaleTurnStore {
    pub(crate) fn new(store: Arc<MongoSandboxStore>) -> Self {
        let database = store.client.database(&store.agents.namespace().db);
        Self {
            store,
            // Replica mode already grants this coordination collection. The
            // scan uses a BSON object key, disjoint from all string Agent IDs.
            state: majority_collection(&database, OWNERS),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ScanError {
    #[error("stale-turn scan state is invalid")]
    Invalid,
    #[error("stale-turn evidence exceeds the proof byte budget")]
    ProofBudget,
    #[error("stale-turn discovery database operation failed")]
    Mongo(#[from] mongodb::error::Error),
    #[error("stale-turn evidence is unavailable")]
    Evidence(#[from] SandboxStoreError),
}

impl ScanError {
    pub(crate) fn outcome(&self) -> &'static str {
        match self {
            Self::Invalid => "invalid_scan_state",
            Self::ProofBudget => "proof_byte_budget",
            Self::Mongo(_) => "database_unavailable",
            Self::Evidence(_) => "evidence_unavailable",
        }
    }
}
