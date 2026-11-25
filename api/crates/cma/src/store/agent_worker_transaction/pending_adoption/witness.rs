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
    source_digest: Binary,
    effect_digest: Binary,
    key: String,
    fingerprint: Binary,
    appended_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) sandbox_event_seq: Option<u64>,
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
            source_digest: publication::result_digest(current).map_err(ApplyError::Domain)?,
            effect_digest: effect_digest(current).map_err(ApplyError::Domain)?,
            key: operation.key,
            fingerprint: operation.fingerprint,
            appended_at: operation.appended_at,
            sandbox_event_seq: None,
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
            || !valid_digest(&marker.source_digest)
            || !valid_digest(&marker.effect_digest)
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

    pub(super) fn lease_live(&self) -> bool {
        layer2_core::seconds_from_rfc3339(&self.expires_at)
            .zip(layer2_core::seconds_from_rfc3339(
                &layer2_core::now_rfc3339(),
            ))
            .is_some_and(|(expires, now)| now < expires)
    }

    pub(super) fn permits_replacement(&self, current: &Agent) -> bool {
        self.state == State::Retired
            && current.revision > self.expected_revision
            && current.conversation.last_seq == self.expected_seq
            && incarnation(current) == self.incarnation
            && effect_digest(current).is_ok_and(|digest| digest == self.effect_digest)
            && current
                .work
                .lease
                .as_ref()
                .is_some_and(|lease| lease.fence > self.fence)
    }

    pub(super) fn lease_server_expression(&self) -> Document {
        doc! {"$gt": [{"$dateFromString": {"dateString": "$work_lease_expires_at", "onError": Bson::Null}}, "$$NOW"]}
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

pub(in crate::store) fn unavailable() -> SandboxStoreError {
    SandboxStoreError::Unavailable(
        "Original Agent publication requires exact adoption settlement".into(),
    )
}

pub(in crate::store) fn blocks_claim(
    store: &MongoSandboxStore,
    row: &Document,
    agent: &Agent,
) -> Result<bool, SandboxStoreError> {
    if !pending_step(agent) {
        return Ok(false);
    }
    Ok(Witness::read(row)
        .map_err(|_| unavailable())?
        .is_some_and(|witness| {
            witness.state == State::Admitted
                || witness.state == State::Retired && !store.segment_adoption_enabled
        }))
}

pub(super) fn matches_source(witness: &Witness, agent: &Agent) -> Result<bool, ApplyError> {
    Ok(witness.incarnation == incarnation(agent)
        && witness.expected_revision == agent.revision
        && witness.expected_seq == agent.conversation.last_seq
        && witness.source_digest
            == publication::result_digest(agent).map_err(ApplyError::Domain)?
        && agent.work.lease.as_ref().is_some_and(|lease| {
            lease.fence == witness.fence && lease.expires_at == witness.expires_at
        }))
}

pub(super) fn expired(witness: &Witness, now: &str) -> bool {
    layer2_core::seconds_from_rfc3339(&witness.expires_at)
        .zip(layer2_core::seconds_from_rfc3339(now))
        .is_some_and(|(expires, at)| at >= expires)
}

fn effect_digest(agent: &Agent) -> Result<Binary, SandboxStoreError> {
    let mut agent = agent.clone();
    agent.work.lease = None;
    agent.work.next_fence = 0;
    agent.revision = 0;
    agent.updated_at.clear();
    publication::request_digest(&agent)
}

pub(in crate::store) async fn fence_replacement(
    store: &MongoSandboxStore,
    row: &Document,
    current: &Agent,
    sandbox: &Sandbox,
    session: &mut mongodb::ClientSession,
) -> Result<bool, TransactionLogError> {
    if !pending_step(current) {
        return Ok(true);
    }
    let Some(witness) = Witness::read(row).map_err(|_| unavailable())? else {
        return Ok(true);
    };
    match witness.state {
        State::Admitted => Ok(false),
        State::Published => Ok(true),
        State::Retired => {
            if !store.segment_adoption_enabled
                || incarnation(current) != witness.incarnation
                || current.conversation.last_seq != witness.expected_seq
                || effect_digest(current)? != witness.effect_digest
                || !lineage::matches_retained(store, current, sandbox, &witness.lineage, session)
                    .await?
            {
                return Ok(false);
            }
            let database = store.client.database(&store.agents.namespace().db);
            for name in [
                writer_gate::GateName::CapacityRecovery,
                writer_gate::GateName::Segments,
            ] {
                if writer_gate::select_and_fence(&database, name, current, session)
                    .await?
                    .is_none()
                {
                    return Ok(false);
                }
            }
            Ok(true)
        }
    }
}
