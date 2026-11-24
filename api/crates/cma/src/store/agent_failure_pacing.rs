//! Advisory retry pacing outside the rollback-sensitive Agent record schema.
use std::time::{Duration, SystemTime};

use layer2_core::sandbox::store::{AgentRetryObservation, AgentWorkMutation, SandboxStoreError};
use layer2_core::sandbox::{Agent, AgentLogKind, AgentWorkStep};
use mongodb::bson::{doc, Bson, Document};
use serde::{Deserialize, Serialize};

use super::agent_log_integrity::TransactionLogError;
use super::sandbox_mongo::{
    commit_transaction, is_transient, signed, start_transaction, CommitOutcome, MongoSandboxStore,
    DOCUMENT_SCHEMA_VERSION,
};

pub(super) mod discovery;
pub(in crate::store) mod ownership;
const FIELD: &str = "worker_failure_pacing_v1";
const CLASS: &str = "agent_runtime_unavailable";
const PARENT_CLASS: &str = "agent_parent_not_ready";
const MAX_DELAY: u64 = 300;

pub(super) fn command_generation(row: &Document) -> Option<u64> {
    match row.get("agent_command_admission_fence") {
        None => Some(0),
        Some(Bson::Int64(value)) => u64::try_from(*value).ok(),
        _ => None,
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Pacing {
    class: String,
    agent_revision: u64,
    #[serde(default)]
    command_admission_generation: u64,
    attempts: u32,
    repeats: u32,
    sandbox_revision: u64,
    workspace_revision: u64,
    worktree_revision: u64,
    ready_at: String,
    until: String,
}

impl Pacing {
    fn read(row: &Document) -> Option<Self> {
        mongodb::bson::deserialize_from_bson(row.get(FIELD)?.clone()).ok()
    }

    fn unchanged_parents(&self, observed: &AgentRetryObservation) -> bool {
        self.sandbox_revision == observed.sandbox_revision
            && self.workspace_revision == observed.workspace_revision
            && self.worktree_revision == observed.worktree_revision
    }

    fn applies(&self, agent: &Agent, now: &str) -> bool {
        let Some(at) = layer2_core::seconds_from_rfc3339(now) else {
            return false;
        };
        matches!(self.class.as_str(), CLASS | PARENT_CLASS)
            && agent.work.step == AgentWorkStep::PublishConnection
            && self.ready_at == agent.work.ready_at
            && self.attempts == agent.work.attempts
            && self.repeats <= 4
            && layer2_core::seconds_from_rfc3339(&self.until)
                .is_some_and(|until| until > at && until <= at.saturating_add(MAX_DELAY))
    }
}

/// Publication has already committed. Missing this optional CAS (including a
/// crash here) can only cause an extra ordinary retry, never lose pending work.
pub(super) async fn record(
    store: &MongoSandboxStore,
    mutation: &AgentWorkMutation,
    observed: &AgentRetryObservation,
) -> Result<(), SandboxStoreError> {
    record_with_marker(store, mutation, observed, None).await
}

pub(super) async fn record_with_marker(
    store: &MongoSandboxStore,
    mutation: &AgentWorkMutation,
    observed: &AgentRetryObservation,
    repeated: Option<&AgentLogKind>,
) -> Result<(), SandboxStoreError> {
    let next = &mutation.agent;
    let evidence = match (mutation.agent_logs.as_slice(), repeated) {
        ([], Some(marker)) => std::slice::from_ref(marker),
        (logs, None) => logs,
        _ => return Ok(()),
    };
    let class = match evidence {
        [AgentLogKind::WorkStepFailed {
            step: AgentWorkStep::PublishConnection,
            failure,
        }] if matches!(failure.class.as_str(), CLASS | PARENT_CLASS) => failure.class.as_str(),
        _ => return Ok(()),
    };
    if next.work.step != AgentWorkStep::PublishConnection || next.work.lease.is_some() {
        return Ok(());
    }
    let Some(at) = layer2_core::seconds_from_rfc3339(&mutation.appended_at) else {
        return Ok(());
    };
    let Some(base) = layer2_core::seconds_from_rfc3339(&next.work.ready_at)
        .and_then(|ready| ready.checked_sub(at))
        .filter(|base| *base > 0 && *base < MAX_DELAY)
    else {
        return Ok(());
    };
    let Some(row) = store
        .agents
        .find_one(doc! {"_id": next.id.as_str()})
        .await
        .map_err(unavailable)?
    else {
        return Ok(());
    };
    // Publication may have settled capacity or another writer may have advanced
    // the Agent. A hint must never describe a different resulting record.
    if super::agents::read_agent(&row)? != *next {
        return Ok(());
    }
    let Some(observed_publication_revision) = observed.agent_revision.checked_add(1) else {
        return Ok(());
    };
    let repeats = Pacing::read(&row)
        .filter(|previous| {
            previous.class == class
                && previous.command_admission_generation == observed.command_admission_generation
                && previous.unchanged_parents(observed)
                && previous.agent_revision.checked_add(2) == Some(next.revision)
                && previous.attempts.checked_add(1) == Some(next.work.attempts)
                && previous.repeats <= 4
        })
        .map_or(0, |previous| (previous.repeats + 1).min(4));
    let delay = base.saturating_mul(1 << repeats).min(MAX_DELAY);
    let pacing = Pacing {
        class: class.into(),
        // Connection failure publication may adopt concurrent first-message or
        // log metadata. Preserve the inspected revision so that change wakes
        // work instead of becoming a fresh but unobserved hold.
        agent_revision: observed_publication_revision,
        // Never adopt the generation read after publication. Keeping the
        // pre-inspection generation fences this hint even if admission commits
        // before, during, or after this CAS: both readers must wake on mismatch.
        command_admission_generation: observed.command_admission_generation,
        attempts: next.work.attempts,
        repeats,
        sandbox_revision: observed.sandbox_revision,
        workspace_revision: observed.workspace_revision,
        worktree_revision: observed.worktree_revision,
        ready_at: next.work.ready_at.clone(),
        until: layer2_core::rfc3339_at(SystemTime::UNIX_EPOCH + Duration::from_secs(at + delay)),
    };
    let mut filter = super::agent_capacity_recovery::source::envelope_filter(next, &row)?;
    filter.insert(FIELD, row.get(FIELD).cloned().unwrap_or(Bson::Null));
    let update = doc! {"$set": {FIELD: mongodb::bson::serialize_to_bson(&pacing)
    .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?}};
    let mut session = store.client.start_session().await.map_err(unavailable)?;
    if let Err(error) = start_transaction(&mut session).await {
        return optional_write_error(error);
    }
    match super::work_publication::agent::fence_parent(store, next, &mut session).await {
        Ok(true) => {}
        Ok(false) | Err(TransactionLogError::Domain(SandboxStoreError::Conflict)) => {
            let _ = session.abort_transaction().await;
            return Ok(());
        }
        Err(error) => {
            let _ = session.abort_transaction().await;
            return match error {
                TransactionLogError::Domain(error) => Err(error),
                TransactionLogError::Driver(error) => optional_write_error(error),
            };
        }
    }
    let saved = store
        .agents
        .update_one(filter, update)
        .session(&mut session)
        .await;
    match saved {
        Ok(result) if result.modified_count == 1 => {}
        other => {
            let _ = session.abort_transaction().await;
            return other.map(|_| ()).or_else(optional_write_error);
        }
    }
    optional_commit_result(commit_transaction(&mut session).await)
}

fn optional_commit_result(outcome: CommitOutcome) -> Result<(), SandboxStoreError> {
    match outcome {
        CommitOutcome::Committed | CommitOutcome::Retry(_) => Ok(()),
        CommitOutcome::Failed(error) => optional_write_error(error),
        CommitOutcome::Unknown(error) => Err(unavailable(error)),
    }
}

fn optional_write_error(error: mongodb::error::Error) -> Result<(), SandboxStoreError> {
    if is_transient(&error)
        || matches!(
            error.get_custom::<SandboxStoreError>(),
            Some(SandboxStoreError::Conflict)
        )
    {
        Ok(())
    } else {
        Err(unavailable(error))
    }
}

pub(super) enum Decision {
    Ordinary,
    Hold,
    Wake,
}

pub(super) async fn decision(
    store: &MongoSandboxStore,
    row: &Document,
    agent: &Agent,
    now: &str,
) -> Result<Decision, SandboxStoreError> {
    let Some(pacing) = Pacing::read(row).filter(|pacing| pacing.applies(agent, now)) else {
        return Ok(Decision::Ordinary);
    };
    if pacing.agent_revision != agent.revision
        || command_generation(row) != Some(pacing.command_admission_generation)
    {
        return Ok(Decision::Wake);
    }
    for (collection, id, revision) in [
        (
            &store.sandboxes,
            agent.sandbox_id.as_str(),
            pacing.sandbox_revision,
        ),
        (
            &store.workspaces,
            agent.workspace_id.as_str(),
            pacing.workspace_revision,
        ),
        (
            &store.worktrees,
            agent.worktree_id.as_str(),
            pacing.worktree_revision,
        ),
    ] {
        let parent = collection
            .find_one(super::work_schema::for_collection(
                collection,
                doc! {"_id": id,
                "document_schema_version": DOCUMENT_SCHEMA_VERSION},
            ))
            .projection(doc! {"revision": 1, "record.revision": 1})
            .await
            .map_err(unavailable)?;
        let Some(parent) = parent else {
            return Ok(Decision::Wake);
        };
        let expected = signed(revision, "parent revision").map_err(SandboxStoreError::Corrupt)?;
        if parent.get_i64("revision").ok() != Some(expected)
            || parent
                .get_document("record")
                .ok()
                .and_then(|r| r.get_i64("revision").ok())
                != Some(expected)
        {
            return Ok(Decision::Wake);
        }
    }
    Ok(Decision::Hold)
}

fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}

#[cfg(test)]
#[test]
fn retry_hint_preserves_publication_on_known_refusal_but_reports_unknown_commit() {
    let conflict = || mongodb::error::Error::custom(SandboxStoreError::Conflict);
    assert!(optional_commit_result(CommitOutcome::Failed(conflict())).is_ok());
    assert!(matches!(
        optional_commit_result(CommitOutcome::Unknown(conflict())),
        Err(SandboxStoreError::Unavailable(_))
    ));
}
