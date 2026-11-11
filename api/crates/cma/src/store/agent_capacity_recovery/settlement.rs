//! Accepted-work mutations within the caller's exclusive transaction. These
//! helpers never commit, infer an unknown outcome, or activate a runtime.
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
use mongodb::bson::{doc, spec::BinarySubtype, Binary};

use super::{authority, records, source};
use crate::store::{
    agent_log_integrity::{AppendCapacity, TransactionLogError},
    agent_log_segments::{
        capture::{self, CapturedFormat},
        collections::collection,
        digest,
        publication::{self, Publication, PublicationIdentity},
        transaction_scope::TransactionScope,
        writer_commit::{self, AggregateUpdate, PublicationInput},
        PUBLICATIONS,
    },
    agents::agent_replacement_fields,
    sandbox_mongo::MongoSandboxStore,
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
    if work.phase == RecoveryPhase::Completed {
        return Err(SandboxStoreError::Unavailable(
            "completed capacity recovery requires exact receipt reconciliation".into(),
        )
        .into());
    }
    if work.phase == RecoveryPhase::Superseded {
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
    let key = request.key().map_err(|_| SandboxStoreError::Conflict)?;
    let work = records::read(store, &key, scope)
        .await?
        .ok_or(SandboxStoreError::Conflict)?;
    if &work.request != request {
        return Err(SandboxStoreError::Conflict.into());
    }
    // The original immutable publication proves gated admission. A manually
    // inserted or damaged carrier cannot grant recovery permission.
    let admission = work
        .admission_identity()
        .map_err(|_| SandboxStoreError::Conflict)?;
    let identity = PublicationIdentity {
        key: admission.key,
        fingerprint: Binary {
            subtype: BinarySubtype::Generic,
            bytes: admission.fingerprint.to_vec(),
        },
        appended_at: admission.appended_at,
    };
    let admitted = read_publication(store, current, &identity.key, scope).await?;
    admitted.matches_request(current, &identity)?;
    if admitted.expected_seq != admitted.last_seq {
        return Err(SandboxStoreError::Conflict.into());
    }
    let receipt = if work.revision == 1 {
        admitted
    } else {
        read_publication(
            store,
            current,
            &format!("{key}:step:{}", work.revision),
            scope,
        )
        .await?
    };
    // This active-mutation path requires exact current result evidence. It is
    // not the later-head/unknown-outcome reconciliation path: changed aggregate
    // state must be reconciled there, never accepted from a carrier alone.
    if u64::try_from(receipt.result_revision).ok() != Some(current.revision)
        || u64::try_from(receipt.last_seq).ok() != Some(current.conversation.last_seq)
        || receipt.result_digest != publication::result_digest(&(current, &work))?
    {
        return Err(SandboxStoreError::Conflict.into());
    }
    Ok(work)
}

async fn read_publication(
    store: &MongoSandboxStore,
    current: &Agent,
    key: &str,
    scope: &mut TransactionScope<'_>,
) -> Result<Publication, TransactionLogError> {
    let id = publication::identity(current.id.as_str(), &digest::incarnation(current), key);
    let row = scope
        .find_one(&collection(store, PUBLICATIONS), doc! {"_id": id})
        .await
        .map_err(TransactionLogError::Driver)?
        .ok_or(SandboxStoreError::Conflict)?;
    Ok(Publication::read(row)?)
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
            result_digest: publication::result_digest(&(next, next_work))?,
        },
        None,
        None,
    )
    .await?;
    let filter = source::fence_filter_scoped(store, context.agent, scope).await?;
    records::replace(store, work, next_work, scope).await?;
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
