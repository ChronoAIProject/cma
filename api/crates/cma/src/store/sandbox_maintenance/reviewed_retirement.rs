//! Read and fence the exact inactivity evidence for a reviewed retirement.

use layer2_core::sandbox::reviewed_retirement::{
    ReviewedRetirementMember, ReviewedRetirementOutcome, ReviewedRetirementReceipt,
    ReviewedRetirementStore,
};
use layer2_core::sandbox::store::{SandboxEventStore, SandboxStoreError};
use layer2_core::sandbox::{Agent, Sandbox, SandboxEventKind, SandboxStatus};
use layer2_core::{seconds_from_rfc3339, SandboxId};
use mongodb::bson::{doc, Document};
use mongodb::ClientSession;

use super::evidence::{self, scoped::CapturedActivities, LogValidation};
use super::transaction::ApplyError;
use crate::store::agent_log_integrity::IntegrityFailurePolicy;
use crate::store::agent_log_segments::transaction_scope::TransactionScope;
use crate::store::lifecycle_execution::load_sandbox_agents;
use crate::store::sandbox_mongo::{commit_transaction, signed, CommitOutcome, MongoSandboxStore};
use crate::store::sandbox_transactions::save_reviewed_retirement;
use crate::store::sandboxes::read_sandbox;
use crate::store::transaction_terminal::GuardedTerminal as _;

const RECEIPT_FIELD: &str = "reviewed_retirement_v1";

enum Candidate {
    Eligible,
    Existing,
    Accepted(ReviewedRetirementReceipt),
    Stale,
}

fn classify(
    member: &ReviewedRetirementMember,
    sandbox: &Sandbox,
    document: &Document,
) -> Result<Candidate, SandboxStoreError> {
    if !member.same_sandbox(sandbox) {
        return Ok(Candidate::Stale);
    }
    if matches!(
        sandbox.status,
        SandboxStatus::Retiring | SandboxStatus::Retired
    ) {
        let Some(receipt) = read_receipt(document)? else {
            return Ok(Candidate::Existing);
        };
        if receipt.belongs_to(member)? {
            return Ok(Candidate::Accepted(receipt));
        }
        if receipt.batch_sha256 == member.batch_sha256
            && receipt.member_sha256 == member.member_sha256
        {
            return Ok(Candidate::Stale);
        }
        return Ok(Candidate::Existing);
    }
    if document.contains_key(RECEIPT_FIELD) {
        return Err(SandboxStoreError::Corrupt(
            "reviewed retirement receipt precedes retirement".into(),
        ));
    }
    Ok(if member.matches(sandbox) {
        Candidate::Eligible
    } else {
        Candidate::Stale
    })
}

fn read_receipt(
    document: &Document,
) -> Result<Option<ReviewedRetirementReceipt>, SandboxStoreError> {
    document
        .get(RECEIPT_FIELD)
        .map(|stored| {
            mongodb::bson::deserialize_from_bson(stored.clone()).map_err(|_| {
                SandboxStoreError::Corrupt("invalid reviewed retirement receipt".into())
            })
        })
        .transpose()
}

async fn fence_eligibility(
    store: &MongoSandboxStore,
    member: &ReviewedRetirementMember,
    sandbox: &Sandbox,
    agents: &[Agent],
    sources: &CapturedActivities,
    now: &str,
    session: &mut ClientSession,
) -> Result<bool, ApplyError> {
    if !member.validate(now)
        || evidence::queued_control(store, &sandbox.id, session).await?
        || !evidence::agent_commands_clear(store, agents, session).await?
        || !evidence::sessions::clear(store, &sandbox.id, now, session).await?
        || !evidence::legacy_log_budget::within(store, agents, Some(sources), session).await?
    {
        return Ok(false);
    }
    let Some(workspaces) = evidence::idle_workspaces(store, sandbox, session).await? else {
        return Ok(false);
    };
    let Some(cutoff) = seconds_from_rfc3339(&member.idle_before) else {
        return Ok(false);
    };
    let older = |at: &str| seconds_from_rfc3339(at).is_some_and(|at| at < cutoff);
    if agents.iter().any(|agent| {
        agent.owner != sandbox.owner
            || agent.sandbox_id != sandbox.id
            || !older(&agent.updated_at)
            || (!agent.status.is_live()
                && (agent.work.step.is_pending()
                    || agent.work.lease.is_some()
                    || agent.conversation.active_turn.is_some()
                    || !agent.conversation.queued_messages.is_empty()
                    || agent.requested_stop_reason.is_some()))
    }) || workspaces.iter().any(|workspace| {
        !older(&workspace.created_at)
            || workspace.ready_at.as_deref().is_some_and(|at| !older(at))
            || workspace.removed_at.as_deref().is_some_and(|at| !older(at))
    }) {
        return Ok(false);
    }
    let activities = evidence::live_activities_with_sources(
        store,
        agents,
        session,
        now,
        Some(sources),
        LogValidation {
            failure_policy: IntegrityFailurePolicy::ReportOnly,
            legacy_formats_checked: true,
            use_recovery_activity: false,
            certified_legacy: None,
        },
        true,
    )
    .await?;
    if !member.live_agents_are_idle(sandbox, &activities, now) {
        return Ok(false);
    }
    if activities.is_empty()
        && !layer2_core::sandbox::agentless_idle_with_children(
            sandbox,
            agents,
            &workspaces,
            now,
            seconds_from_rfc3339(now)
                .unwrap_or(cutoff)
                .saturating_sub(cutoff),
        )
    {
        return Ok(false);
    }
    evidence::fence_agents(store, agents, session).await?;
    Ok(true)
}

fn unavailable(error: impl std::fmt::Display) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}

async fn apply(
    store: &MongoSandboxStore,
    member: &ReviewedRetirementMember,
    now: &str,
    session: &mut ClientSession,
) -> Result<(ReviewedRetirementOutcome, bool), ApplyError> {
    let mut scope = TransactionScope::start(session)
        .await
        .map_err(ApplyError::Driver)?;
    let sources = CapturedActivities::capture(store, &member.sandbox_id, &mut scope).await?;
    apply_after_snapshot(store, member, now, &sources, scope.release()).await
}

#[cfg(test)]
#[path = "reviewed_retirement_tests.rs"]
mod tests;

async fn apply_after_snapshot(
    store: &MongoSandboxStore,
    member: &ReviewedRetirementMember,
    now: &str,
    sources: &CapturedActivities,
    session: &mut ClientSession,
) -> Result<(ReviewedRetirementOutcome, bool), ApplyError> {
    use ReviewedRetirementOutcome::{Accepted, NotEligible, ObserveExisting};
    let document = store
        .sandboxes
        .find_one(doc! { "_id": member.sandbox_id.as_str() })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    let mut sandbox = read_sandbox(&document).map_err(ApplyError::Domain)?;
    match classify(member, &sandbox, &document).map_err(ApplyError::Domain)? {
        Candidate::Accepted(receipt) => return Ok((Accepted(receipt), false)),
        Candidate::Existing => return Ok((ObserveExisting, false)),
        Candidate::Stale => return Ok((NotEligible, false)),
        Candidate::Eligible => {}
    }
    let agents = load_sandbox_agents(store, &sandbox.id, session).await?;
    if !fence_eligibility(store, member, &sandbox, &agents, sources, now, session).await? {
        return Ok((NotEligible, false));
    }
    let from = sandbox.status;
    sandbox
        .begin_retirement(now.to_owned())
        .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?;
    let event = save_reviewed_retirement(
        store,
        session,
        member.revision,
        &sandbox,
        now,
        &SandboxEventKind::StatusChanged {
            from,
            to: SandboxStatus::Retiring,
        },
    )
    .await
    .map_err(|error| match error {
        crate::store::sandbox_transactions::ApplyError::Domain(error) => ApplyError::Domain(error),
        crate::store::sandbox_transactions::ApplyError::Driver(error) => ApplyError::Driver(error),
    })?;
    let receipt = ReviewedRetirementReceipt {
        batch_sha256: member.batch_sha256.clone(),
        member_sha256: member.member_sha256.clone(),
        guard_sha256: member.fingerprint().map_err(ApplyError::Domain)?,
        sandbox_id: sandbox.id,
        accepted_revision: sandbox.revision,
        event_sequence: event.seq,
    };
    let revision = signed(receipt.accepted_revision, "retirement revision")
        .map_err(SandboxStoreError::Corrupt)
        .map_err(ApplyError::Domain)?;
    let value = mongodb::bson::serialize_to_bson(&receipt).map_err(|_| {
        ApplyError::Domain(SandboxStoreError::Corrupt(
            "invalid retirement receipt".into(),
        ))
    })?;
    let written = store
        .sandboxes
        .update_one(
            doc! { "_id": receipt.sandbox_id.as_str(), "revision": revision,
            RECEIPT_FIELD: { "$exists": false } },
            doc! { "$set": { RECEIPT_FIELD: value } },
        )
        .session(session)
        .await
        .map_err(ApplyError::Driver)?;
    if written.matched_count != 1 {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    Ok((Accepted(receipt), true))
}

#[async_trait::async_trait]
impl ReviewedRetirementStore for MongoSandboxStore {
    async fn admit_reviewed_retirement(
        &self,
        member: &ReviewedRetirementMember,
        now: &str,
    ) -> Result<ReviewedRetirementOutcome, SandboxStoreError> {
        let mut session = self.client.start_session().await.map_err(unavailable)?;
        let result = apply(self, member, now, &mut session).await;
        match result {
            Ok((outcome, true)) => match commit_transaction(&mut session).await {
                CommitOutcome::Committed => {
                    self.sandbox_admitted.notify_one();
                    Ok(outcome)
                }
                CommitOutcome::Retry(error)
                | CommitOutcome::Unknown(error)
                | CommitOutcome::Failed(error) => Err(unavailable(error)),
            },
            Ok((outcome, false)) => {
                session.abort_guarded().await.map_err(unavailable)?;
                Ok(outcome)
            }
            Err(error) => {
                session.abort_guarded().await.map_err(unavailable)?;
                Err(match error {
                    ApplyError::Domain(error) => error,
                    ApplyError::Driver(error) => unavailable(error),
                })
            }
        }
    }

    async fn reviewed_retirement_receipt(
        &self,
        id: &SandboxId,
        batch: &str,
        member: &str,
    ) -> Result<Option<ReviewedRetirementReceipt>, SandboxStoreError> {
        let Some(document) = self
            .sandboxes
            .find_one(doc! { "_id": id.as_str() })
            .read_concern(mongodb::options::ReadConcern::majority())
            .await
            .map_err(unavailable)?
        else {
            return Ok(None);
        };
        let Some(receipt) = read_receipt(&document)? else {
            return Ok(None);
        };
        if receipt.sandbox_id != *id
            || receipt.batch_sha256 != batch
            || receipt.member_sha256 != member
        {
            return Ok(None);
        }
        let after = receipt.event_sequence.checked_sub(1).ok_or_else(|| {
            SandboxStoreError::Corrupt("invalid retirement event sequence".into())
        })?;
        let events = self.sandbox_events(id, after, 1).await?;
        let exact = events.first().is_some_and(|event| {
            event.seq == receipt.event_sequence
                && matches!(
                    event.kind,
                    SandboxEventKind::StatusChanged {
                        to: SandboxStatus::Retiring,
                        ..
                    }
                )
        });
        if !exact {
            return Err(SandboxStoreError::Corrupt(
                "reviewed retirement event is unavailable".into(),
            ));
        }
        Ok(Some(receipt))
    }
}
