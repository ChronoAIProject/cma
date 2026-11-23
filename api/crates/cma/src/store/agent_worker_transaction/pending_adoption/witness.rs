//! Durable proof visible on the ordinary Agent reads through flag changes.
use super::*;
use serde::{Deserialize, Serialize};

pub(in crate::store) const FIELD: &str = "pending_worker_log_adoption_v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum State {
    Admitted,
    Published,
    Retired,
}

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Witness {
    version: i64,
    pub(super) state: State,
    pub(super) expected_revision: u64,
    expected_seq: u64,
    fence: u64,
    expires_at: String,
    incarnation: String,
    pub(super) lineage: Binary,
    key: String,
    fingerprint: Binary,
    appended_at: String,
}

impl Witness {
    pub(super) fn new(
        current: &Agent,
        mutation: &AgentWorkMutation,
        lineage: Binary,
        operation: PublicationIdentity,
    ) -> Result<Self, ApplyError> {
        let lease = current.work.lease.as_ref().ok_or_else(unresolved)?;
        Ok(Self {
            version: 1,
            state: State::Admitted,
            expected_revision: mutation.expected_agent_revision,
            expected_seq: current.conversation.last_seq,
            fence: mutation.fence,
            expires_at: lease.expires_at.clone(),
            incarnation: incarnation(current),
            lineage,
            key: operation.key,
            fingerprint: operation.fingerprint,
            appended_at: operation.appended_at,
        })
    }

    pub(super) fn read(row: &Document) -> Result<Option<Self>, ApplyError> {
        let Some(value) = row.get(FIELD) else {
            return Ok(None);
        };
        let marker: Self =
            mongodb::bson::deserialize_from_bson(value.clone()).map_err(|_| unresolved())?;
        if marker.version != 1
            || marker.key
                != format!(
                    "{PENDING_WORKER_PREFIX}{}:{}",
                    marker.expected_revision, marker.fence
                )
            || !valid_digest(&marker.lineage)
            || layer2_core::seconds_from_rfc3339(&marker.expires_at).is_none()
            || marker.operation().validate().is_err()
        {
            return Err(unresolved());
        }
        Ok(Some(marker))
    }

    pub(super) fn operation(&self) -> PublicationIdentity {
        PublicationIdentity {
            key: self.key.clone(),
            fingerprint: self.fingerprint.clone(),
            appended_at: self.appended_at.clone(),
        }
    }

    pub(super) fn document(&self) -> Result<Document, ApplyError> {
        mongodb::bson::serialize_to_document(self).map_err(|_| unresolved())
    }

    pub(super) fn require_operation(
        &self,
        operation: &PublicationIdentity,
    ) -> Result<(), ApplyError> {
        if self.key != operation.key
            || self.fingerprint != operation.fingerprint
            || self.appended_at != operation.appended_at
        {
            return Err(unresolved());
        }
        Ok(())
    }

    pub(super) fn require_source(
        &self,
        current: &Agent,
        mutation: &AgentWorkMutation,
    ) -> Result<(), ApplyError> {
        self.require_operation(&identity(mutation, &self.lineage)?)?;
        if self.state != State::Admitted
            || current.revision != self.expected_revision
            || current.conversation.last_seq != self.expected_seq
            || incarnation(current) != self.incarnation
            || current.work.lease.as_ref().is_none_or(|lease| {
                lease.fence != self.fence
                    || lease.holder != mutation.holder
                    || lease.expires_at != self.expires_at
            })
            || self.expires_at <= layer2_core::now_rfc3339()
        {
            return Err(unresolved());
        }
        Ok(())
    }

    pub(super) fn logs(
        &self,
        mutation: &AgentWorkMutation,
    ) -> Result<Vec<AgentLogEntry>, ApplyError> {
        let count = mutation.agent_logs.len() as u64;
        if self.expected_seq.checked_add(count) != Some(mutation.agent.conversation.last_seq) {
            return Err(unresolved());
        }
        Ok(mutation
            .agent_logs
            .iter()
            .enumerate()
            .map(|(offset, kind)| AgentLogEntry {
                agent_id: mutation.agent.id.clone(),
                seq: self.expected_seq + offset as u64 + 1,
                appended_at: self.appended_at.clone(),
                kind: kind.clone(),
            })
            .collect())
    }
}

pub(in crate::store) fn pending_step(agent: &Agent) -> bool {
    agent.requested_stop_reason.is_none()
        && agent.deleted_at.is_none()
        && matches!(
            (agent.status, agent.work.step),
            (AgentStatus::Starting, AgentWorkStep::PublishReady)
                | (
                    AgentStatus::Ready | AgentStatus::Working | AgentStatus::WaitingApproval,
                    AgentWorkStep::PublishConnection
                )
        )
}

pub(super) fn unresolved() -> ApplyError {
    ApplyError::Domain(unavailable())
}

pub(super) fn unavailable() -> SandboxStoreError {
    SandboxStoreError::Unavailable(
        "Original Agent publication requires exact adoption settlement".into(),
    )
}

pub(in crate::store) fn blocks_claim(
    row: &Document,
    agent: &Agent,
) -> Result<bool, SandboxStoreError> {
    if !pending_step(agent) {
        return Ok(false);
    }
    let witness = Witness::read(row).map_err(|_| unavailable())?;
    Ok(witness.is_some_and(|witness| witness.state != State::Published))
}

/// Closing the expired claim writes the same witness as the publication. A
/// racing commit must either win first or lose its write conflict. Inspecting
/// a stale snapshot or failing to commit retirement grants no new authority.
pub(in crate::store) async fn retire_expired(
    store: &MongoSandboxStore,
    row: &Document,
    agent: &Agent,
    now: &str,
) -> Result<(), SandboxStoreError> {
    let value = Witness::read(row).map_err(|_| unavailable())?;
    let Some(mut witness) = value
        .filter(|witness| witness.state == State::Admitted && witness.expires_at.as_str() <= now)
    else {
        return Ok(());
    };
    let original = witness.document().map_err(|_| unavailable())?;
    witness.state = State::Retired;
    let retired = witness.document().map_err(|_| unavailable())?;
    let result = store.agents.update_one(
        doc! {"_id": agent.id.as_str(), FIELD: original, "document_schema_version": DOCUMENT_SCHEMA_VERSION},
        doc! {"$set": {FIELD: retired}},
    )
        .write_concern(mongodb::options::WriteConcern::majority())
        .await.map_err(|_| unavailable())?;
    if result.matched_count == 1 {
        cohort(Stage::Blocked, Outcome::KnownAbort);
    }
    Ok(())
}
