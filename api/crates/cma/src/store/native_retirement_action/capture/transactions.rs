use super::*;
use crate::store::{
    agent_ownership::FenceError,
    agents::read_agent,
    lifecycle_execution::LifecycleLoadError,
    native_retirement_action::{read_sandbox, retryable_body_error, unavailable},
    sandbox_mongo::{
        commit_transaction, signed, start_transaction, CommitOutcome, MongoSandboxStore,
        TRANSACTION_ATTEMPTS,
    },
    work_claim_authority::sandbox_claim,
};
use futures_util::future::BoxFuture;
use layer2_core::agent_ownership::execution_authority;
use layer2_core::sandbox::AgentWorkStep;
use layer3_adapter::owner_protocol::TransportAuthority;
use layer3_adapter::receiver_failure::EligibleReceiverFailure;
use mongodb::bson::{doc, Bson};
use mongodb::options::ReadConcern;
use mongodb::ClientSession;
use std::time::Duration;

use LifecycleLoadError::{Domain, Driver};

impl MongoSandboxStore {
    pub(crate) async fn offer_retirement_capture(
        &self,
        parent: &Sandbox,
        child: &AgentId,
    ) -> Result<Uuid, SandboxStoreError> {
        let parent_binding = ParentBinding::from_parent(parent)?;
        let mut filter = sandbox_claim(parent)?;
        filter.insert(
            "revision",
            signed(parent.revision, "retirement parent revision").map_err(|_| CONFLICT)?,
        );
        self.write_claimed_capture(
            (self, child, parent_binding, filter),
            |(store, child, parent_binding, filter), session| {
                Box::pin(async move {
                    let row = store.capture_row(filter.clone(), session).await?;
                    let current = read_sandbox(&row).map_err(Domain)?;
                    if ParentBinding::from_parent(&current).map_err(Domain)? != *parent_binding {
                        return Err(Domain(CONFLICT));
                    }
                    let agent_row = store
                        .agents
                        .find_one(doc! {"_id":child.as_str()})
                        .session(&mut *session)
                        .await
                        .map_err(Driver)?
                        .ok_or(Domain(CONFLICT))?;
                    let agent = read_agent(&agent_row).map_err(Domain)?;
                    let next = prepare_offer(&row, &current, &agent).map_err(Domain)?;
                    let receipt = CaptureRecord::decode(&next)
                        .map_err(Domain)?
                        .slots
                        .into_iter()
                        .find(|slot| &slot.child.agent == *child)
                        .ok_or(Domain(CONFLICT))?;
                    store
                        .write_capture_row(&row, next, Some(session), Some(filter.clone()))
                        .await?;
                    Ok((parent_binding.clone(), receipt))
                })
            },
        )
        .await
        .map(|(_, slot)| slot.offer_id)
    }

    pub(crate) async fn retirement_capture_offer(
        &self,
        expected: &Sandbox,
        agent: &Agent,
    ) -> Result<Uuid, SandboxStoreError> {
        let row = self
            .sandboxes
            .find_one(doc! {"_id": agent.sandbox_id.as_str()})
            .read_concern(ReadConcern::majority())
            .await
            .map_err(unavailable)?
            .ok_or(CONFLICT)?;
        let (parent, record) = current_record(&row)?;
        if ParentBinding::from_parent(expected)? != record.parent {
            return Err(CONFLICT);
        }
        let lineage = ChildLineage::from_child(&parent, agent)?;
        record
            .slots
            .iter()
            .find(|slot| slot.child == lineage)
            .map(|slot| slot.offer_id)
            .ok_or(CONFLICT)
    }

    pub(crate) async fn bind_retirement_capture(
        &self,
        agent: &Agent,
        offer_id: Uuid,
        attempt_id: Uuid,
    ) -> Result<BoundCapture, SandboxStoreError> {
        let authority = execution_authority()
            .filter(|authority| {
                authority.token.agent_id == agent.id
                    && self.agent_manager_instance() == Some(authority.token.instance_id.as_str())
                    && authority.is_current()
            })
            .ok_or(CONFLICT)?;
        if agent.work.step != AgentWorkStep::CloseProcess || agent.requested_stop_reason.is_none() {
            return Err(CONFLICT);
        }
        let attempt = BoundAttempt {
            id: attempt_id,
            child_revision: agent.revision,
            claim: agent.work.lease.clone().ok_or(CONFLICT)?,
            owner: authority.owner(),
        };
        let (parent, slot) = self
            .write_claimed_capture(
                (self, agent, offer_id, &attempt),
                |(store, agent, offer_id, attempt), session| {
                    Box::pin(async move {
                        let row = store
                            .capture_row(doc! {"_id":agent.sandbox_id.as_str()}, session)
                            .await?;
                        let (parent, mut record) = current_record(&row).map_err(Domain)?;
                        let lineage = ChildLineage::from_child(&parent, agent).map_err(Domain)?;
                        let slot = record
                            .slots
                            .iter_mut()
                            .find(|slot| slot.offer_id == *offer_id && slot.child == lineage)
                            .ok_or(Domain(CONFLICT))?;
                        let bound = CaptureState::Bound {
                            attempt: (*attempt).clone(),
                        };
                        if slot.state != CaptureState::Offered && slot.state != bound {
                            return Err(Domain(CONFLICT));
                        }
                        store.fence_capture_child(agent, attempt, session).await?;
                        slot.state = bound;
                        let receipt = slot.clone();
                        store
                            .write_capture_row(
                                &row,
                                record.encode().map_err(Domain)?,
                                Some(session),
                                None,
                            )
                            .await?;
                        Ok((record.parent, receipt))
                    })
                },
            )
            .await?;
        if !authority.is_current() {
            return Err(CONFLICT);
        }
        Ok(BoundCapture {
            parent,
            offer_id,
            child: slot.child,
            attempt,
        })
    }

    pub(crate) async fn complete_retirement_capture(
        &self,
        bound: &BoundCapture,
        evidence: &EligibleReceiverFailure,
    ) -> Result<(), SandboxStoreError> {
        if evidence.attempt_id() != bound.attempt.id
            || evidence.receiver().owner != bound.attempt.owner
            || evidence.receiver().runtime.as_str() != bound.parent.runtime.runtime_id
        {
            return Err(CONFLICT);
        }
        let observed = CaptureState::Observed {
            attempt: bound.attempt.clone(),
            receiver: evidence.receiver().clone(),
            refusal: evidence.refusal(),
        };
        for _ in 0..TRANSACTION_ATTEMPTS {
            let row = self
                .sandboxes
                .find_one(doc! {"_id":bound.parent.sandbox.as_str()})
                .read_concern(ReadConcern::majority())
                .await
                .map_err(unavailable)?
                .ok_or(CONFLICT)?;
            let (_, mut record) = current_record(&row)?;
            if record.parent != bound.parent {
                return Err(CONFLICT);
            }
            let slot = record
                .slots
                .iter_mut()
                .find(|slot| slot.offer_id == bound.offer_id && slot.child == bound.child)
                .ok_or(CONFLICT)?;
            if slot.state == observed {
                return Ok(());
            }
            if !matches!(&slot.state, CaptureState::Bound { attempt } if attempt == &bound.attempt)
            {
                return Err(CONFLICT);
            }
            slot.state = observed.clone();
            let receipt = slot.clone();
            match self
                .write_capture_row(&row, record.encode()?, None, None)
                .await
            {
                Ok(()) => return Ok(()),
                Err(Domain(SandboxStoreError::Conflict)) => continue,
                Err(Driver(error)) => {
                    self.prove_capture(&record.parent, &receipt, error).await?;
                    return Ok(());
                }
                Err(Domain(error)) => return Err(error),
            }
        }
        Err(CONFLICT)
    }

    async fn fence_capture_child(
        &self,
        agent: &Agent,
        attempt: &BoundAttempt,
        session: &mut ClientSession,
    ) -> Result<(), LifecycleLoadError> {
        let row = self
            .agents
            .find_one(doc! {"_id":agent.id.as_str()})
            .session(&mut *session)
            .await
            .map_err(Driver)?
            .ok_or(Domain(CONFLICT))?;
        if read_agent(&row).map_err(Domain)? != *agent {
            return Err(Domain(CONFLICT));
        }
        self.fence_agent_effect_transaction(&agent.id, session)
            .await
            .map_err(|error| match error {
                FenceError::Driver(error) => Driver(error),
                FenceError::Domain(error) => Domain(error),
            })?;
        let mut filter = doc! {
            "_id":agent.id.as_str(),
            "revision":signed(agent.revision, "retirement child revision").map_err(|_| Domain(CONFLICT))?,
            "work_lease_expires_at": &attempt.claim.expires_at,
            "$expr":{"$gt":[{"$dateFromString":{"dateString":"$work_lease_expires_at","onError":Bson::Null}}, "$$NOW"]},
        };
        self.agent_work_owner_fields(&agent.id, &mut filter)
            .map_err(Domain)?;
        // Join the child row's write history so a concurrent work replacement conflicts.
        let result = self
            .agents
            .update_one(
                filter,
                doc! {"$inc":{"native_retirement_capture_fence":1_i64}},
            )
            .session(session)
            .await
            .map_err(Driver)?;
        if result.matched_count != 1 {
            return Err(Domain(CONFLICT));
        }
        Ok(())
    }

    async fn capture_row(
        &self,
        filter: Document,
        session: &mut ClientSession,
    ) -> Result<Document, LifecycleLoadError> {
        self.sandboxes
            .find_one(filter)
            .session(session)
            .await
            .map_err(Driver)?
            .ok_or(Domain(CONFLICT))
    }

    async fn write_capture_row(
        &self,
        row: &Document,
        next: Document,
        session: Option<&mut ClientSession>,
        claim: Option<Document>,
    ) -> Result<(), LifecycleLoadError> {
        let mut filter = doc! {
            "_id":row.get("_id").ok_or(Domain(CONFLICT))?.clone(),
            FIELD: {"$exists":false},
            LEDGER_MAINTENANCE_FIELD: {"$exists":false},
            "$expr":{"$eq":["$record", {"$literal": row.get("record").ok_or(Domain(CONFLICT))?.clone()}]},
        };
        if let Some(previous) = row.get(CAPTURE) {
            filter.insert(CAPTURE, previous.clone());
        } else {
            filter.insert(CAPTURE, doc! {"$exists":false});
        }
        if let Some(claim) = claim {
            filter.insert("$and", [claim]);
        }
        let update = self
            .sandboxes
            .update_one(filter, doc! {"$set": {CAPTURE: next}});
        let result = match session {
            Some(session) => update.session(session).await,
            None => update.await,
        }
        .map_err(Driver)?;
        if result.matched_count != 1 {
            return Err(Domain(CONFLICT));
        }
        Ok(())
    }

    async fn write_claimed_capture<C: Sync>(
        &self,
        context: C,
        apply: impl for<'a> Fn(
            &'a C,
            &'a mut ClientSession,
        )
            -> BoxFuture<'a, Result<(ParentBinding, Slot), LifecycleLoadError>>,
    ) -> Result<(ParentBinding, Slot), SandboxStoreError> {
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = self.client.start_session().await.map_err(unavailable)?;
            start_transaction(&mut session).await.map_err(unavailable)?;
            let (parent, slot) = match apply(&context, &mut session).await {
                Ok(receipt) => receipt,
                Err(error) => {
                    if let Err(abort) = session.abort_transaction().await {
                        tracing::warn!(error = %abort, "retirement capture abort was not acknowledged");
                    }
                    match error {
                        Driver(error) if retryable_body_error(&error) => continue,
                        Driver(error) => return Err(unavailable(error)),
                        Domain(error) => return Err(error),
                    }
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok((parent, slot)),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(error) => self.prove_capture(&parent, &slot, error).await?,
                CommitOutcome::Failed(error) => return Err(unavailable(error)),
            }
            return Ok((parent, slot));
        }
        Err(CONFLICT)
    }

    async fn prove_capture(
        &self,
        parent: &ParentBinding,
        slot: &Slot,
        error: mongodb::error::Error,
    ) -> Result<(), SandboxStoreError> {
        let exact = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let row = self
                    .sandboxes
                    .find_one(doc! {"_id":parent.sandbox.as_str()})
                    .read_concern(ReadConcern::majority())
                    .await
                    .map_err(unavailable)?
                    .ok_or(CONFLICT)?;
                if row.contains_key(FIELD)
                    || row.contains_key(LEDGER_MAINTENANCE_FIELD)
                    || ParentBinding::from_parent(&read_sandbox(&row)?)? != *parent
                {
                    return Err(CONFLICT);
                }
                if row.contains_key(CAPTURE) {
                    let (_, record) = current_record(&row)?;
                    if record.slots.contains(slot) {
                        return Ok(());
                    }
                }
                // Majority visibility can lag a commit whose reply was lost.
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        // A missing receipt cannot authorize a retry of this write.
        match exact {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error @ SandboxStoreError::Corrupt(_))) => Err(error),
            _ => Err(unavailable(error)),
        }
    }
}

fn current_record(row: &Document) -> Result<(Sandbox, CaptureRecord), SandboxStoreError> {
    if row.contains_key(FIELD) || row.contains_key(LEDGER_MAINTENANCE_FIELD) {
        return Err(CONFLICT);
    }
    let parent = read_sandbox(row)?;
    let record = match row.get(CAPTURE) {
        Some(Bson::Document(document)) => CaptureRecord::decode(document)?,
        None => return Err(CONFLICT),
        Some(_) => return Err(invalid_capture()),
    };
    if ParentBinding::from_parent(&parent)? != record.parent {
        return Err(CONFLICT);
    }
    Ok((parent, record))
}
