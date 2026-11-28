//! Exact, bounded observation slots on the parent retirement document.
use super::{Agent, RuntimeBinding, Sandbox, SandboxStoreError, FIELD};
use crate::store::work_claim_authority::LEDGER_MAINTENANCE_FIELD;
use layer2_core::sandbox::store::MAX_SANDBOX_LIFECYCLE_AGENTS;
use layer2_core::sandbox::{AgentStopReason, AgentWorkLease, SandboxStatus, SandboxWorkStep};
use layer2_core::{AgentId, SandboxId, SubjectId, WorkspaceId, WorktreeId};
use layer3_adapter::owner_protocol::TransportOwner;
use layer3_adapter::receiver_failure::{ChallengeRefusal, RetirementReceiver};
use mongodb::bson::{self, Document};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use uuid::Uuid;

const CAPTURE: &str = "native_retirement_capture_v1";
const MAX_BYTES: usize = 256 * 1024;
const MAX_RECEIVER_ID_BYTES: usize = 512;
// Reserve the runtime, the two Pod identities, the owner tuple and BSON overhead.
const OBSERVATION_RESERVE: usize = 2 * 1024;

mod consumption;
mod transactions;
pub(crate) use consumption::{CapturedRetirementObservation, RetirementCaptureConsumer};
const CONFLICT: SandboxStoreError = SandboxStoreError::Conflict;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptureRecord {
    parent: ParentBinding,
    slots: Vec<Slot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParentBinding {
    sandbox: SandboxId,
    owner: SubjectId,
    runtime: RuntimeBinding,
    home_identity: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChildLineage {
    agent: AgentId,
    workspace: WorkspaceId,
    worktree: WorktreeId,
    runtime_generation: u64,
    stop_reason: AgentStopReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Slot {
    offer_id: Uuid,
    child: ChildLineage,
    state: CaptureState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
enum CaptureState {
    Offered,
    Bound {
        attempt: BoundAttempt,
    },
    Observed {
        attempt: BoundAttempt,
        receiver: RetirementReceiver,
        refusal: ChallengeRefusal,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoundAttempt {
    id: Uuid,
    child_revision: u64,
    claim: AgentWorkLease,
    owner: TransportOwner,
}

/// A historical capability for one admitted slot and attempt.
pub(crate) struct BoundCapture {
    parent: ParentBinding,
    offer_id: Uuid,
    child: ChildLineage,
    attempt: BoundAttempt,
}

impl BoundCapture {
    pub(crate) fn capture<F: std::future::Future>(
        &self,
        future: F,
    ) -> (
        layer3_adapter::receiver_failure::ReceiverFailureCapture,
        impl std::future::Future<Output = F::Output> + use<F>,
    ) {
        layer3_adapter::receiver_failure::capture_attempt(self.attempt.id, future)
    }
}

impl ParentBinding {
    fn from_parent(parent: &Sandbox) -> Result<Self, SandboxStoreError> {
        if parent.status != SandboxStatus::Retiring
            || parent.work.step != SandboxWorkStep::StopAgents
        {
            return Err(CONFLICT);
        }
        let runtime = parent
            .runtime
            .as_ref()
            .filter(|runtime| runtime.runtime_id.starts_with("agent-sandbox:"))
            .ok_or(CONFLICT)?;
        let home = parent
            .lifecycle
            .home
            .as_ref()
            .and_then(|home| home.attachment())
            .ok_or(CONFLICT)?;
        home.validate_for(parent.id.as_str())
            .map_err(|_| CONFLICT)?;
        Ok(Self {
            sandbox: parent.id.clone(),
            owner: parent.owner.clone(),
            runtime: runtime.clone(),
            home_identity: home.identity().map_err(|_| CONFLICT)?,
        })
    }
}

impl ChildLineage {
    fn from_child(parent: &Sandbox, child: &Agent) -> Result<Self, SandboxStoreError> {
        if child.sandbox_id != parent.id || child.owner != parent.owner || !child.status.is_live() {
            return Err(CONFLICT);
        }
        Ok(Self {
            agent: child.id.clone(),
            workspace: child.workspace_id.clone(),
            worktree: child.worktree_id.clone(),
            runtime_generation: child
                .conversation
                .runtime_generation
                .filter(|generation| *generation > 0)
                .ok_or(CONFLICT)?,
            stop_reason: child
                .requested_stop_reason
                .clone()
                .unwrap_or(AgentStopReason::SandboxRetired),
        })
    }
}

// The caller will compare-and-set this value under the current parent claim.
pub(super) fn prepare_offer(
    row: &Document,
    parent: &Sandbox,
    child: &Agent,
) -> Result<Document, SandboxStoreError> {
    if row.contains_key(FIELD) || row.contains_key(LEDGER_MAINTENANCE_FIELD) {
        return Err(CONFLICT);
    }
    let parent_identity = ParentBinding::from_parent(parent)?;
    let revision = child.revision;
    let work = &child.work;
    let child = ChildLineage::from_child(parent, child)?;
    let mut record = match row.get(CAPTURE) {
        None => CaptureRecord {
            parent: parent_identity,
            slots: Vec::new(),
        },
        Some(bson::Bson::Document(document)) => {
            let record = CaptureRecord::decode(document)?;
            if record.parent != parent_identity {
                return Err(CONFLICT);
            }
            record
        }
        Some(_) => return Err(invalid_capture()),
    };
    if let Some(existing) = record
        .slots
        .iter_mut()
        .find(|slot| slot.child.agent == child.agent)
    {
        let same_claim = match &existing.state {
            CaptureState::Offered => true,
            CaptureState::Bound { attempt } | CaptureState::Observed { attempt, .. } => {
                attempt.child_revision == revision
                    && work.owns(&attempt.claim.holder, attempt.claim.fence)
            }
        };
        if existing.child != child {
            return Err(CONFLICT);
        }
        if !same_claim {
            *existing = new_offer(child);
        }
        return record.encode();
    }
    if record.slots.len() == MAX_SANDBOX_LIFECYCLE_AGENTS {
        return Err(CONFLICT);
    }
    record.slots.push(new_offer(child));
    record.encode()
}

fn new_offer(child: ChildLineage) -> Slot {
    Slot {
        offer_id: Uuid::new_v4(),
        child,
        state: CaptureState::Offered,
    }
}

impl CaptureRecord {
    fn encode(&self) -> Result<Document, SandboxStoreError> {
        self.validate()?;
        let document = bson::serialize_to_document(self).map_err(|_| invalid_capture())?;
        if !size_allowed(&document)? || !self.has_observation_capacity(&document)? {
            return Err(CONFLICT);
        }
        Ok(document)
    }

    fn decode(document: &Document) -> Result<Self, SandboxStoreError> {
        if !size_allowed(document)? {
            return Err(invalid_capture());
        }
        let record: Self =
            bson::deserialize_from_document(document.clone()).map_err(|_| invalid_capture())?;
        record.validate()?;
        if !record.has_observation_capacity(document)? {
            return Err(invalid_capture());
        }
        Ok(record)
    }

    fn has_observation_capacity(&self, document: &Document) -> Result<bool, SandboxStoreError> {
        let bound = self
            .slots
            .iter()
            .filter(|slot| matches!(slot.state, CaptureState::Bound { .. }))
            .count();
        Ok(encoded_size(document)? + bound * OBSERVATION_RESERVE <= MAX_BYTES)
    }

    fn validate(&self) -> Result<(), SandboxStoreError> {
        let mut children = HashSet::new();
        let mut offers = HashSet::new();
        if self.slots.is_empty()
            || self.slots.len() > MAX_SANDBOX_LIFECYCLE_AGENTS
            || !self.slots.iter().all(|slot| {
                children.insert(&slot.child.agent)
                    && offers.insert(slot.offer_id)
                    && slot.valid_for(&self.parent)
            })
        {
            return Err(invalid_capture());
        }
        Ok(())
    }
}

impl Slot {
    fn valid_for(&self, parent: &ParentBinding) -> bool {
        if self.offer_id.is_nil()
            || self.child.runtime_generation == 0
            || [
                self.child.agent.as_str(),
                self.child.workspace.as_str(),
                self.child.worktree.as_str(),
            ]
            .iter()
            .any(|id| id.trim().is_empty())
        {
            return false;
        }
        let attempt = match &self.state {
            CaptureState::Offered => return true,
            CaptureState::Bound { attempt } => attempt,
            CaptureState::Observed {
                attempt, receiver, ..
            } => {
                if receiver.runtime.as_str() != parent.runtime.runtime_id
                    || receiver.owner != attempt.owner
                    || receiver.runtime.as_str().len() > MAX_RECEIVER_ID_BYTES
                    || receiver.pod.uid.is_empty()
                    || receiver.pod.uid.len() > MAX_RECEIVER_ID_BYTES
                    || receiver
                        .pod
                        .container_id
                        .as_deref()
                        .is_none_or(|id| id.is_empty() || id.len() > MAX_RECEIVER_ID_BYTES)
                    || receiver.pod.restart_count.is_none_or(|count| count < 0)
                {
                    return false;
                }
                attempt
            }
        };
        !attempt.id.is_nil()
            && attempt.child_revision > 0
            && attempt.claim.fence > 0
            && !attempt.claim.holder.trim().is_empty()
            && layer2_core::seconds_from_rfc3339(&attempt.claim.expires_at).is_some()
            && attempt.owner.agent_id == self.child.agent.as_str()
            && attempt.owner.validate().is_ok()
    }
}

fn size_allowed(document: &Document) -> Result<bool, SandboxStoreError> {
    Ok(encoded_size(document)? <= MAX_BYTES)
}

fn encoded_size(document: &Document) -> Result<usize, SandboxStoreError> {
    bson::serialize_to_vec(document)
        .map(|bytes| bytes.len())
        .map_err(|_| invalid_capture())
}

fn invalid_capture() -> SandboxStoreError {
    SandboxStoreError::Corrupt("native retirement capture is invalid".into())
}

#[cfg(test)]
mod tests;
