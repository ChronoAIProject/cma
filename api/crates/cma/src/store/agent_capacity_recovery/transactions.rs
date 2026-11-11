//! One accepted-work attempt per exclusive transaction. An uncertain commit
//! never repeats the mutation or authorizes a runtime attachment/acknowledgement.
use layer2_core::resource_access::AuthorizedResource;
use layer2_core::sandbox::{
    agent_capacity_recovery::{
        ports::{ClaimOutcome, CommitOutcome, RecoveryError, RecoveryLookup},
        service::CapacityRecoveryFeatures,
        state::{RecoveryClaim, RecoveryConfirmation, RecoveryRequest, RecoveryWork},
    },
    store::SandboxStoreError,
};
use mongodb::{
    error::{TRANSIENT_TRANSACTION_ERROR, UNKNOWN_TRANSACTION_COMMIT_RESULT},
    ClientSession,
};

use super::{admission, authority, receipts, settlement};
use crate::store::{
    agent_log_integrity::TransactionLogError,
    agent_log_segments::transaction_scope::TransactionScope, sandbox_mongo::MongoSandboxStore,
};

/// The caller stages the original S4 admission/bootstrap before this atomic
/// attempt. Process admission is independently closed by default; accepted-work
/// settlement below deliberately does not consult this switch.
pub(in crate::store) async fn admit(
    store: &MongoSandboxStore,
    input: admission::AdmissionRequest<'_>,
    features: CapacityRecoveryFeatures,
) -> Result<CommitOutcome<admission::AdmittedRecovery>, RecoveryError> {
    if !features.enabled {
        return Err(RecoveryError::Disabled);
    }
    let mut session = store
        .client
        .start_session()
        .await
        .map_err(|_| RecoveryError::Unavailable)?;
    let mut scope = TransactionScope::start(&mut session)
        .await
        .map_err(|_| RecoveryError::Unavailable)?;
    let result = admission::persist_admission(store, input, &mut scope).await;
    finish(scope.release(), result).await
}

/// The caller obtains fresh resource authority for each call. This read does
/// not authorize a new logical admission: absence always remains Unknown.
pub(in crate::store) async fn lookup_accepted(
    store: &MongoSandboxStore,
    authorized: &AuthorizedResource,
    request: &RecoveryRequest,
) -> Result<RecoveryLookup, RecoveryError> {
    request.validate()?;
    let mut session = store
        .client
        .start_session()
        .await
        .map_err(|_| RecoveryError::Unavailable)?;
    let mut scope = TransactionScope::start(&mut session)
        .await
        .map_err(|_| RecoveryError::Unavailable)?;
    let result = async {
        let current = authority::capture(store, authorized, &request.agent_id, &mut scope).await?;
        let stored = receipts::read(store, current.context()?.agent, request, &mut scope).await?;
        authority::require(authorized)?;
        Ok::<_, TransactionLogError>(stored)
    }
    .await;
    // There have been no writes or commit attempts. A read-abort error does not
    // turn absence into a known-aborted *publication*.
    let _ = scope.release().abort_transaction().await;
    Ok(match result.map_err(error)? {
        Some(stored) => RecoveryLookup::Recorded(Box::new(stored.work)),
        None => RecoveryLookup::Unknown,
    })
}

/// Reconcile the original attempted work, never a result reconstructed with a
/// replacement lease or timestamp. Call with newly acquired current authority.
pub(in crate::store) async fn reconcile(
    store: &MongoSandboxStore,
    authorized: &AuthorizedResource,
    expected: &RecoveryWork,
) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
    expected.validate()?;
    Ok(
        match lookup_accepted(store, authorized, &expected.request).await? {
            RecoveryLookup::Recorded(actual) if actual.as_ref() == expected => {
                CommitOutcome::Recorded(*actual)
            }
            _ => CommitOutcome::Unknown,
        },
    )
}

pub(in crate::store) async fn claim(
    store: &MongoSandboxStore,
    input: settlement::ClaimRequest<'_>,
) -> Result<ClaimOutcome, RecoveryError> {
    let mut session = store
        .client
        .start_session()
        .await
        .map_err(|_| RecoveryError::Unavailable)?;
    let mut scope = TransactionScope::start(&mut session)
        .await
        .map_err(|_| RecoveryError::Unavailable)?;
    let result = settlement::claim(store, input, &mut scope).await;
    match finish(scope.release(), result).await? {
        CommitOutcome::Recorded(value) => Ok(value),
        CommitOutcome::KnownAbort => Err(RecoveryError::Unavailable),
        CommitOutcome::Unknown => Ok(ClaimOutcome::Unknown),
    }
}

pub(in crate::store) async fn complete(
    store: &MongoSandboxStore,
    authorized: &AuthorizedResource,
    claim: &RecoveryClaim,
    confirmation: &RecoveryConfirmation,
    now: &str,
) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
    let mut session = store
        .client
        .start_session()
        .await
        .map_err(|_| RecoveryError::Unavailable)?;
    let mut scope = TransactionScope::start(&mut session)
        .await
        .map_err(|_| RecoveryError::Unavailable)?;
    let result =
        settlement::complete(store, authorized, claim, confirmation, now, &mut scope).await;
    finish(scope.release(), result).await
}

pub(in crate::store) async fn retry(
    store: &MongoSandboxStore,
    authorized: &AuthorizedResource,
    claim: &RecoveryClaim,
    now: &str,
    retry_at: &str,
) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
    let mut session = store
        .client
        .start_session()
        .await
        .map_err(|_| RecoveryError::Unavailable)?;
    let mut scope = TransactionScope::start(&mut session)
        .await
        .map_err(|_| RecoveryError::Unavailable)?;
    let result = settlement::retry(store, authorized, claim, now, retry_at, &mut scope).await;
    finish(scope.release(), result).await
}

pub(super) async fn finish<T>(
    session: &mut ClientSession,
    result: Result<T, TransactionLogError>,
) -> Result<CommitOutcome<T>, RecoveryError> {
    let value = match result {
        Ok(value) => value,
        Err(failure) => {
            let _ = session.abort_transaction().await;
            return Err(error(failure));
        }
    };
    // Issue commit once. A helper that first retries an Unknown result and later
    // returns only a transient error would erase the earlier uncertainty.
    Ok(match session.commit_transaction().await {
        Ok(()) => CommitOutcome::Recorded(value),
        Err(failure) if failure.contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT) => {
            CommitOutcome::Unknown
        }
        Err(failure) if failure.contains_label(TRANSIENT_TRANSACTION_ERROR) => {
            CommitOutcome::KnownAbort
        }
        // A bare failure is not promoted to KnownAbort. The original attempt
        // must be reconciled from immutable publication evidence.
        Err(_) => CommitOutcome::Unknown,
    })
}

pub(super) fn error(error: TransactionLogError) -> RecoveryError {
    match error {
        TransactionLogError::Domain(
            SandboxStoreError::Conflict | SandboxStoreError::SequenceConflict,
        ) => RecoveryError::Conflict,
        TransactionLogError::Domain(SandboxStoreError::Corrupt(_)) => {
            RecoveryError::InvalidEvidence
        }
        _ => RecoveryError::Unavailable,
    }
}
