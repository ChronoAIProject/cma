//! Advisory retry pacing outside the rollback-sensitive Agent record schema.
use std::time::{Duration, SystemTime};

use layer2_core::sandbox::store::{AgentRetryObservation, AgentWorkMutation, SandboxStoreError};
use layer2_core::sandbox::{Agent, AgentLogKind, AgentWorkStep};
use mongodb::bson::{doc, Bson, Document};
use serde::{Deserialize, Serialize};

use super::sandbox_mongo::{signed, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};

pub(super) mod discovery;
const FIELD: &str = "worker_failure_pacing_v1";
const CLASS: &str = "agent_runtime_unavailable";
const MAX_DELAY: u64 = 300;

#[derive(Debug, Serialize, Deserialize)]
struct Pacing {
    class: String,
    agent_revision: u64,
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
        self.class == CLASS
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
    let next = &mutation.agent;
    if next.work.step != AgentWorkStep::PublishConnection
        || next.work.lease.is_some()
        || !matches!(mutation.agent_logs.as_slice(),
            [AgentLogKind::WorkStepFailed { step: AgentWorkStep::PublishConnection, failure }]
                if failure.class == CLASS)
    {
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
            previous.class == CLASS
                && previous.unchanged_parents(observed)
                && previous.agent_revision.checked_add(2) == Some(next.revision)
                && previous.attempts.checked_add(1) == Some(next.work.attempts)
                && previous.repeats <= 4
        })
        .map_or(0, |previous| (previous.repeats + 1).min(4));
    let delay = base.saturating_mul(1 << repeats).min(MAX_DELAY);
    let pacing = Pacing {
        class: CLASS.into(),
        // Connection failure publication may adopt concurrent first-message or
        // log metadata. Preserve the inspected revision so that change wakes
        // work instead of becoming a fresh but unobserved hold.
        agent_revision: observed_publication_revision,
        attempts: next.work.attempts,
        repeats,
        sandbox_revision: observed.sandbox_revision,
        workspace_revision: observed.workspace_revision,
        worktree_revision: observed.worktree_revision,
        ready_at: next.work.ready_at.clone(),
        until: layer2_core::rfc3339_at(SystemTime::UNIX_EPOCH + Duration::from_secs(at + delay)),
    };
    store.agents.update_one(
        doc! {"_id": next.id.as_str(), "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "revision": signed(next.revision, "agent revision").map_err(SandboxStoreError::Corrupt)?,
            FIELD: row.get(FIELD).cloned().unwrap_or(Bson::Null)},
        doc! {"$set": {FIELD: mongodb::bson::serialize_to_bson(&pacing)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?}},
    ).await.map_err(unavailable)?;
    Ok(())
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
    if pacing.agent_revision != agent.revision {
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
            .find_one(doc! {"_id": id,
            "document_schema_version": DOCUMENT_SCHEMA_VERSION})
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
