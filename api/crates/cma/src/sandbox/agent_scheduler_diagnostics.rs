//! Scheduler observations retain stage and correlation without private error text.

use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::AgentId;
use sha2::{Digest, Sha256};
use tokio::task::JoinError;

pub(super) fn agent_hash(id: &AgentId) -> String {
    format!("{:x}", Sha256::digest(id.as_str().as_bytes()))
}

fn store_failure(error: &SandboxStoreError) -> &'static str {
    match error {
        // These variants erase the originating boundary. In particular, a
        // Conflict alone proves neither a known abort nor lost ownership.
        SandboxStoreError::Conflict => "conflict_unclassified",
        SandboxStoreError::Unavailable(_) => "store_unavailable",
        SandboxStoreError::Corrupt(_) => "stored_data_invalid",
        _ => "domain_refusal",
    }
}

pub(super) fn failed(phase: &'static str, agent_hash: Option<&str>, error: &SandboxStoreError) {
    tracing::error!(target: "agent_scheduler_failure", parent: None,
        phase,
        failure = store_failure(error),
        agent_hash,
        "agent lifecycle task failed"
    );
}

pub(super) fn terminated(phase: &'static str, agent_hash: Option<&str>, error: &JoinError) {
    let failure = if error.is_panic() {
        "task_panicked"
    } else {
        "task_cancelled"
    };
    // A panic payload and inherited request spans may contain private data.
    tracing::error!(target: "agent_scheduler_failure", parent: None,
        phase,
        failure,
        agent_hash,
        "agent lifecycle task terminated unexpectedly"
    );
}
