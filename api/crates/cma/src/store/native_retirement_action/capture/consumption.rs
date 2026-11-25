use super::*;
use crate::store::native_retirement_action::{
    read_sandbox, signed, transact, unavailable, writer_fences, LifecycleLoadError,
    MongoSandboxStore, RetirementAction, RetirementFences, RetirementObservation,
};
use crate::store::work_claim_authority::sandbox_claim;
use mongodb::bson::{doc, Bson};
use mongodb::{options::ReadConcern, ClientSession};

use LifecycleLoadError::{Domain, Driver};

#[async_trait::async_trait]
pub(crate) trait RetirementCaptureConsumer: Send + Sync {
    async fn offer(&self, parent: &Sandbox, child: &Agent) -> Result<(), SandboxStoreError>;

    async fn observed(
        &self,
        parent: &Sandbox,
        agents: &[Agent],
    ) -> Result<Option<CapturedRetirementObservation>, SandboxStoreError>;

    async fn admit(
        &self,
        parent: &Sandbox,
        action: &RetirementAction,
        captured: &CapturedRetirementObservation,
    ) -> Result<(), SandboxStoreError>;
}

pub(crate) struct CapturedRetirementObservation {
    parent: ParentBinding,
    slot: Slot,
    observation: RetirementObservation,
    fences: RetirementFences,
}

impl CapturedRetirementObservation {
    pub(crate) fn observation(&self) -> (RetirementObservation, RetirementFences) {
        (self.observation.clone(), self.fences)
    }

    pub(in crate::store::native_retirement_action) async fn fence_admission(
        &self,
        store: &MongoSandboxStore,
        row: &Document,
        parent: &Sandbox,
        agents: &[Agent],
        action: &RetirementAction,
        session: &mut ClientSession,
    ) -> Result<(), LifecycleLoadError> {
        let record = read_record(row, parent)
            .map_err(Domain)?
            .ok_or(Domain(CONFLICT))?;
        if record.parent != self.parent
            || !record.slots.contains(&self.slot)
            || self.observation != action.observation
            || self.fences != action.writer_fences
        {
            return Err(Domain(CONFLICT));
        }
        let CaptureState::Observed { attempt, .. } = &self.slot.state else {
            return Err(Domain(CONFLICT));
        };
        if !agents
            .iter()
            .any(|agent| current_child(parent, agent, &self.slot, attempt))
        {
            return Err(Domain(CONFLICT));
        }
        let agent_id = self.slot.child.agent.as_str();
        // Compete with child publication and owner takeover in this same transaction.
        let child = store.agents.update_one(
            doc! {"_id":agent_id, "revision": signed(attempt.child_revision, "capture child revision").map_err(|_| Domain(CONFLICT))?},
            doc! {"$inc":{"native_retirement_capture_fence":1_i64}},
        ).session(&mut *session).await.map_err(Driver)?;
        let owner = store.client.database(&store.agents.namespace().db)
            .collection::<Document>(crate::store::agent_ownership::OWNERS)
            .update_one(
                doc! {"_id":agent_id, "epoch":signed(attempt.owner.epoch, "capture owner epoch").map_err(|_| Domain(CONFLICT))?,
                    "protocol":i64::from(attempt.owner.protocol),
                    "instance_id":{"$in":[Bson::String(attempt.owner.instance_id.clone()), Bson::Null]}},
                doc! {"$inc":{"effect_revision":1_i64}},
            ).session(session).await.map_err(Driver)?;
        if child.matched_count != 1 || owner.matched_count != 1 {
            return Err(Domain(CONFLICT));
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl RetirementCaptureConsumer for MongoSandboxStore {
    async fn offer(&self, parent: &Sandbox, child: &Agent) -> Result<(), SandboxStoreError> {
        self.offer_retirement_capture(parent, &child.id)
            .await
            .map(|_| ())
    }

    async fn observed(
        &self,
        parent: &Sandbox,
        agents: &[Agent],
    ) -> Result<Option<CapturedRetirementObservation>, SandboxStoreError> {
        let mut filter = sandbox_claim(parent)?;
        filter.insert(
            "revision",
            signed(parent.revision, "retirement parent revision").map_err(|_| CONFLICT)?,
        );
        let row = self
            .sandboxes
            .find_one(filter)
            .read_concern(ReadConcern::majority())
            .await
            .map_err(unavailable)?
            .ok_or(CONFLICT)?;
        let current = read_sandbox(&row)?;
        if ParentBinding::from_parent(&current)? != ParentBinding::from_parent(parent)? {
            return Err(CONFLICT);
        }
        let Some(record) = read_record(&row, &current)? else {
            return Ok(None);
        };
        let Some((slot, receiver)) = record.slots.into_iter().find_map(|slot| {
            let CaptureState::Observed {
                attempt, receiver, ..
            } = &slot.state
            else {
                return None;
            };
            agents
                .iter()
                .any(|agent| current_child(&current, agent, &slot, attempt))
                .then(|| (slot.clone(), receiver.clone()))
        }) else {
            return Ok(None);
        };
        Ok(Some(CapturedRetirementObservation {
            observation: RetirementObservation {
                owner: current.owner,
                binding: record.parent.runtime.clone(),
                receiver,
            },
            parent: record.parent,
            fences: writer_fences(&row)?,
            slot,
        }))
    }

    async fn admit(
        &self,
        parent: &Sandbox,
        action: &RetirementAction,
        captured: &CapturedRetirementObservation,
    ) -> Result<(), SandboxStoreError> {
        transact(self, parent, None, Some(action), Some(captured)).await
    }
}

fn read_record(
    row: &Document,
    parent: &Sandbox,
) -> Result<Option<CaptureRecord>, SandboxStoreError> {
    if row.contains_key(FIELD) || row.contains_key(LEDGER_MAINTENANCE_FIELD) {
        return Err(CONFLICT);
    }
    match row.get(CAPTURE) {
        None => Ok(None),
        Some(Bson::Document(document)) => {
            let record = CaptureRecord::decode(document)?;
            if ParentBinding::from_parent(parent)? != record.parent {
                return Err(CONFLICT);
            }
            Ok(Some(record))
        }
        _ => Err(invalid_capture()),
    }
}

fn current_child(parent: &Sandbox, agent: &Agent, slot: &Slot, attempt: &BoundAttempt) -> bool {
    agent.id == slot.child.agent
        && agent.revision == attempt.child_revision
        && agent.requested_stop_reason.as_ref() == Some(&slot.child.stop_reason)
        && agent.work.step == layer2_core::sandbox::AgentWorkStep::CloseProcess
        && agent.work.owns(&attempt.claim.holder, attempt.claim.fence)
        && ChildLineage::from_child(parent, agent).is_ok_and(|lineage| lineage == slot.child)
}
