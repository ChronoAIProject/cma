//! Verify recovery carriers against immutable publications and retained source.
//! A missing carrier/receipt never establishes a known-aborted transaction.
use layer2_core::sandbox::{
    agent_capacity_recovery::state::{RecoveryPhase, RecoveryRequest},
    store::SandboxStoreError,
    Agent,
};
use mongodb::bson::{doc, spec::BinarySubtype, Binary};

use super::records::{self, StoredRecovery};
use crate::store::{
    agent_log_integrity::TransactionLogError,
    agent_log_segments::{
        capture::{self, CapturedFormat, CapturedLog},
        incarnation,
        publication::{self, Publication, PublicationIdentity},
        transaction_scope::TransactionScope,
        Seal,
    },
    sandbox_mongo::MongoSandboxStore,
};

pub(super) async fn read(
    store: &MongoSandboxStore,
    current: &Agent,
    request: &RecoveryRequest,
    scope: &mut TransactionScope<'_>,
) -> Result<Option<StoredRecovery>, TransactionLogError> {
    request
        .validate()
        .map_err(|_| SandboxStoreError::Conflict)?;
    let key = request.key().map_err(|_| SandboxStoreError::Conflict)?;
    let Some(stored) = records::read(store, &key, scope).await? else {
        return Ok(None);
    };
    if stored.work.request != *request {
        return Err(SandboxStoreError::Conflict.into());
    }
    let admitted = publication(store, current, &key, scope).await?;
    let latest = if stored.work.revision == 1 {
        admitted.clone()
    } else {
        publication(
            store,
            current,
            &format!("{key}:step:{}", stored.work.revision),
            scope,
        )
        .await?
    };
    verify_binding(current, &stored, &admitted, &latest)?;
    let captured = capture::capture_transaction(store, current, scope).await?;
    if captured.format() != CapturedFormat::Segmented {
        return Err(SandboxStoreError::Conflict.into());
    }
    require_source(store, &captured, &admitted, scope).await?;
    if stored.work.revision != 1 {
        require_source(store, &captured, &latest, scope).await?;
    }
    let logs = if stored.work.phase == RecoveryPhase::Completed {
        capture::points_transaction(store, &captured, &[stored.result.through_seq], scope).await?
    } else {
        Vec::new()
    };
    stored.result.require_logs(&stored.work, &logs)?;
    Ok(Some(stored))
}

fn verify_binding(
    current: &Agent,
    stored: &StoredRecovery,
    admitted: &Publication,
    latest: &Publication,
) -> Result<(), SandboxStoreError> {
    let work = &stored.work;
    work.validate().map_err(|_| SandboxStoreError::Conflict)?;
    let identity = work
        .admission_identity()
        .map_err(|_| SandboxStoreError::Conflict)?;
    admitted.matches_request(
        current,
        &PublicationIdentity {
            key: identity.key.clone(),
            fingerprint: Binary {
                subtype: BinarySubtype::Generic,
                bytes: identity.fingerprint.to_vec(),
            },
            appended_at: identity.appended_at,
        },
    )?;
    latest.validate()?;
    stored.result.require_publication(work, latest)?;
    let latest_key = if work.revision == 1 {
        identity.key
    } else {
        format!("{}:step:{}", identity.key, work.revision)
    };
    if current.creation_key != work.request.creation_key
        || current.creation_fingerprint != work.request.creation_fingerprint
        || latest.agent_id != current.id.as_str()
        || latest.incarnation != incarnation(current)
        || latest.publication_key != latest_key
        || admitted.expected_seq != admitted.last_seq
        || u64::try_from(admitted.last_seq).ok() != Some(work.request.failure_seq)
        || admitted.expected_revision.checked_add(1) != Some(admitted.result_revision)
        || latest.result_revision < admitted.result_revision
        || latest.last_seq < admitted.last_seq
        || stored.result.agent_revision > current.revision
        || stored.result.through_seq > current.conversation.last_seq
        || (stored.result.agent_revision == current.revision
            && stored.result.agent_digest != publication::result_digest(current)?)
    {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(())
}

async fn publication(
    store: &MongoSandboxStore,
    current: &Agent,
    key: &str,
    scope: &mut TransactionScope<'_>,
) -> Result<Publication, TransactionLogError> {
    let id = publication::identity(current.id.as_str(), &incarnation(current), key);
    let row = scope
        .find_one(&store.segment_logs.publications, doc! {"_id": id})
        .await
        .map_err(TransactionLogError::Driver)?
        .ok_or_else(|| {
            SandboxStoreError::Unavailable(
                "capacity recovery publication outcome remains unresolved".into(),
            )
        })?;
    Ok(Publication::read(row)?)
}

async fn require_source(
    store: &MongoSandboxStore,
    captured: &CapturedLog,
    receipt: &Publication,
    scope: &mut TransactionScope<'_>,
) -> Result<(), TransactionLogError> {
    let head = captured.active_head();
    if head.incarnation != receipt.incarnation
        || head.last_seq < receipt.last_seq
        || head.revision < receipt.head_revision
        || head.active_ordinal < receipt.active_ordinal
        || (head.revision == receipt.head_revision && head.last_seq != receipt.last_seq)
        || (head.active_ordinal == receipt.active_ordinal
            && head.previous_seal != receipt.previous_seal)
    {
        return Err(SandboxStoreError::Conflict.into());
    }
    if receipt.active_ordinal > 0 {
        let ordinal = receipt.active_ordinal - 1;
        let id = Seal::identity(&head.agent_id, &head.incarnation, ordinal);
        let row = scope
            .find_one(&store.segment_logs.seals, doc! {"_id": id})
            .await
            .map_err(TransactionLogError::Driver)?
            .ok_or(SandboxStoreError::Conflict)?;
        let seal = Seal::read(row)?;
        if seal.agent_id != head.agent_id
            || seal.incarnation != head.incarnation
            || seal.ordinal != ordinal
            || receipt.previous_seal.as_ref() != Some(&seal.seal_digest)
            || receipt.expected_seq < seal.end_seq
        {
            return Err(SandboxStoreError::Conflict.into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
