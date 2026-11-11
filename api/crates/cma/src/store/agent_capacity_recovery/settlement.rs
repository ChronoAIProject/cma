//! Accepted-work mutations within the caller's exclusive transaction. These
//! helpers never commit, infer an unknown outcome, or activate a runtime.
use super::{authority, receipts, records, source};
use crate::store::{
    agent_log_integrity::{AppendCapacity, TransactionLogError},
    agent_log_segments::{
        capture::{self, CapturedFormat},
        publication::{self, PublicationIdentity},
        transaction_scope::TransactionScope,
        writer_commit::{self, AggregateUpdate, PublicationInput},
    },
    agents::agent_replacement_fields,
    sandbox_mongo::MongoSandboxStore,
};
use layer2_core::resource_access::AuthorizedResource;
use layer2_core::sandbox::{
    agent_capacity_recovery::{
        ports::ClaimOutcome,
        state::{
            RecoveryClaim, RecoveryConfirmation, RecoveryPhase, RecoveryRequest, RecoveryWork,
        },
        CapacityRecoveryDisposition,
    },
    store::SandboxStoreError,
    Agent, AgentLogEntry,
};

pub(in crate::store) struct ClaimRequest<'a> {
    pub authorized: &'a AuthorizedResource,
    pub request: &'a RecoveryRequest,
    pub holder: &'a str,
    pub now: &'a str,
    pub expires_at: &'a str,
}

/// Caller must first reconcile any uncertain prior attempt. Admission gates do
/// not cancel accepted work. Returned proposals grant no runtime authority until
/// the caller establishes the exact transaction outcome.
pub(in crate::store) async fn claim(
    store: &MongoSandboxStore,
    input: ClaimRequest<'_>,
    scope: &mut TransactionScope<'_>,
) -> Result<ClaimOutcome, TransactionLogError> {
    let current =
        authority::capture(store, input.authorized, &input.request.agent_id, scope).await?;
    let work = read_work(store, input.request, current.context()?.agent, scope).await?;
    if matches!(
        work.phase,
        RecoveryPhase::Completed | RecoveryPhase::Superseded
    ) {
        return Ok(ClaimOutcome::Terminal(Box::new(work)));
    }
    let context = current.context()?;
    if matches!(
        context.classify(input.now),
        CapacityRecoveryDisposition::CleanupOnly | CapacityRecoveryDisposition::TerminalPreserved
    ) {
        let mut next_work = work.clone();
        next_work.phase = RecoveryPhase::Superseded;
        next_work.lease = None;
        next_work.revision = work
            .revision
            .checked_add(1)
            .ok_or(SandboxStoreError::Conflict)?;
        let next = advance(context.agent, input.now)?;
        persist(
            store,
            scope,
            Step {
                authorized: input.authorized,
                current: &current,
                work: &work,
                next_work: &next_work,
                next: &next,
                logs: &[],
                now: input.now,
            },
        )
        .await?;
        return Ok(ClaimOutcome::Terminal(Box::new(next_work)));
    }
    let Some(claim) = work
        .claim(&context, input.holder, input.now, input.expires_at)
        .map_err(|_| SandboxStoreError::Conflict)?
    else {
        return Ok(ClaimOutcome::Waiting(Box::new(work)));
    };
    persist(
        store,
        scope,
        Step {
            authorized: input.authorized,
            current: &current,
            work: &work,
            next_work: &claim.work,
            next: &claim.agent,
            logs: &[],
            now: input.now,
        },
    )
    .await?;
    Ok(ClaimOutcome::Claimed(Box::new(claim)))
}

pub(in crate::store) async fn complete(
    store: &MongoSandboxStore,
    authorized: &AuthorizedResource,
    claim: &RecoveryClaim,
    confirmation: &RecoveryConfirmation,
    now: &str,
    scope: &mut TransactionScope<'_>,
) -> Result<RecoveryWork, TransactionLogError> {
    let current =
        authority::capture(store, authorized, &claim.work.request.agent_id, scope).await?;
    let work = read_work(store, &claim.work.request, current.context()?.agent, scope).await?;
    let (next_work, next, kind) = work
        .complete(&current.context()?, claim, confirmation, now)
        .map_err(|_| SandboxStoreError::Conflict)?;
    let logs = [AgentLogEntry {
        agent_id: next.id.clone(),
        seq: next.conversation.last_seq,
        appended_at: now.into(),
        kind,
    }];
    persist(
        store,
        scope,
        Step {
            authorized,
            current: &current,
            work: &work,
            next_work: &next_work,
            next: &next,
            logs: &logs,
            now,
        },
    )
    .await?;
    Ok(next_work)
}

pub(in crate::store) async fn retry(
    store: &MongoSandboxStore,
    authorized: &AuthorizedResource,
    claim: &RecoveryClaim,
    now: &str,
    retry_at: &str,
    scope: &mut TransactionScope<'_>,
) -> Result<RecoveryWork, TransactionLogError> {
    let current =
        authority::capture(store, authorized, &claim.work.request.agent_id, scope).await?;
    let work = read_work(store, &claim.work.request, current.context()?.agent, scope).await?;
    if work != claim.work {
        return Err(SandboxStoreError::Conflict.into());
    }
    claim
        .require_current(&current.context()?, now)
        .map_err(|_| SandboxStoreError::Conflict)?;
    let lease = claim
        .work
        .lease
        .as_ref()
        .ok_or(SandboxStoreError::Conflict)?;
    let next_work = work
        .retry(&lease.holder, lease.fence, now, retry_at)
        .map_err(|_| SandboxStoreError::Conflict)?;
    let next = advance(&claim.agent, now)?;
    persist(
        store,
        scope,
        Step {
            authorized,
            current: &current,
            work: &work,
            next_work: &next_work,
            next: &next,
            logs: &[],
            now,
        },
    )
    .await?;
    Ok(next_work)
}

async fn read_work(
    store: &MongoSandboxStore,
    request: &RecoveryRequest,
    current: &Agent,
    scope: &mut TransactionScope<'_>,
) -> Result<RecoveryWork, TransactionLogError> {
    let stored = receipts::read(store, current, request, scope)
        .await?
        .ok_or(SandboxStoreError::Conflict)?;
    Ok(stored.work)
}

fn advance(current: &Agent, now: &str) -> Result<Agent, SandboxStoreError> {
    let mut next = current.clone();
    next.revision = next
        .revision
        .checked_add(1)
        .ok_or(SandboxStoreError::Conflict)?;
    next.updated_at = now.into();
    Ok(next)
}

struct Step<'a> {
    authorized: &'a AuthorizedResource,
    current: &'a authority::Authority,
    work: &'a RecoveryWork,
    next_work: &'a RecoveryWork,
    next: &'a Agent,
    logs: &'a [AgentLogEntry],
    now: &'a str,
}

async fn persist(
    store: &MongoSandboxStore,
    scope: &mut TransactionScope<'_>,
    step: Step<'_>,
) -> Result<(), TransactionLogError> {
    let Step {
        authorized,
        current,
        work,
        next_work,
        next,
        logs,
        now,
    } = step;
    let context = current.context()?;
    let captured = capture::capture_transaction(store, context.agent, scope).await?;
    if captured.format() != CapturedFormat::Segmented {
        return Err(SandboxStoreError::Conflict.into());
    }
    let operation = PublicationIdentity {
        key: format!(
            "{}:step:{}",
            work.request
                .key()
                .map_err(|_| SandboxStoreError::Conflict)?,
            next_work.revision
        ),
        fingerprint: publication::request_digest(&(work, next_work, logs))?,
        appended_at: now.into(),
    };
    let key = work
        .request
        .key()
        .map_err(|_| SandboxStoreError::Conflict)?;
    let prior = records::read(store, &key, scope)
        .await?
        .ok_or(SandboxStoreError::Conflict)?;
    if prior.work != *work {
        return Err(SandboxStoreError::Conflict.into());
    }
    let stored = records::StoredRecovery::new(next, next_work.clone(), logs, now)?;
    let prepared = writer_commit::prepare(
        store,
        scope,
        &captured,
        PublicationInput {
            operation: &operation,
            current: context.agent,
            next,
            logs,
            capacity: AppendCapacity::Normal,
            result_digest: stored.result.digest(next_work)?,
        },
        None,
        None,
    )
    .await?;
    let filter = source::fence_filter_scoped(store, context.agent, scope).await?;
    records::replace(store, &prior, &stored, scope).await?;
    authority::fence_parents(store, current, scope).await?;
    authority::require(authorized)?;
    writer_commit::persist(
        store,
        scope,
        prepared,
        AggregateUpdate {
            filter,
            fields: agent_replacement_fields(next)?,
        },
    )
    .await?;
    Ok(())
}
