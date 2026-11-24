//! Operator control after the shared CLI has verified migration authority and
//! schema inventory. No service-startup policy writes or automatic activation.
use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::{Client, Database};
use serde::Serialize;

use super::writer_gate::{self, GateName, GateRecord, Policy};
use crate::store::{
    agent_log_integrity::TransactionLogError,
    sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome},
};

#[derive(Debug, Serialize)]
pub(crate) struct Status {
    present: bool,
    enabled: bool,
    revision: i64,
    basis_points: u16,
    explicit_agents: usize,
    explicit_owners: usize,
    preparation_slots: usize,
}

impl Status {
    fn from_gate(gate: Option<&GateRecord>) -> Self {
        Self {
            present: gate.is_some(),
            enabled: gate.is_some_and(|g| g.policy.enabled),
            revision: gate.map_or(0, |g| g.revision),
            basis_points: gate.map_or(0, |g| g.policy.basis_points),
            explicit_agents: gate.map_or(0, |g| g.policy.agents.len()),
            explicit_owners: gate.map_or(0, |g| g.policy.owners.len()),
            preparation_slots: gate.map_or(0, |g| g.slots.len()),
        }
    }
}

pub(in crate::store) enum Action {
    Status,
    Replace {
        expected_revision: i64,
        policy: Policy,
    },
    Close {
        expected_revision: i64,
    },
}

/// The caller must have verified the existing operator authority before entry.
/// Schema/grants are installed through the shared migration runner, never here.
/// One attempt only: an unknown commit is not retried under a different revision.
pub(in crate::store) async fn execute(
    client: &Client,
    database: &Database,
    name: GateName,
    action: Action,
) -> Result<Status, SandboxStoreError> {
    let mut session = client.start_session().await.map_err(driver)?;
    start_transaction(&mut session).await.map_err(driver)?;
    let result = async {
        let current = writer_gate::read(database, name, &mut session).await?;
        let (expected_revision, policy) = match action {
            Action::Status => return Ok(Status::from_gate(current.as_ref())),
            Action::Replace {
                expected_revision,
                policy,
            } => (expected_revision, policy),
            Action::Close { expected_revision } => {
                // Preserve the fixed cohort seed and finite accepted slot leases.
                let mut policy = current
                    .as_ref()
                    .map_or_else(Policy::default, |g| g.policy.clone());
                policy.enabled = false;
                (expected_revision, policy)
            }
        };
        let next =
            writer_gate::replace(database, name, expected_revision, policy, &mut session).await?;
        Ok::<_, TransactionLogError>(Status::from_gate(Some(&next)))
    }
    .await;
    let status = match result {
        Ok(status) => status,
        Err(error) => {
            let _ = session.abort_guarded().await;
            return Err(match error {
                TransactionLogError::Domain(error) => error,
                TransactionLogError::Driver(error) => driver(error),
            });
        }
    };
    match commit_transaction(&mut session).await {
        CommitOutcome::Committed => Ok(status),
        CommitOutcome::Unknown(_) => Err(SandboxStoreError::Unavailable(
            "Agent log gate outcome is unknown; inspect durable status before another CAS".into(),
        )),
        CommitOutcome::Retry(_) => Err(SandboxStoreError::Conflict),
        CommitOutcome::Failed(error) => Err(driver(error)),
    }
}

fn driver(_: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable("Agent log gate control database operation failed".into())
}
