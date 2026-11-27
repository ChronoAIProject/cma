use super::*;
use crate::store::sandbox_mongo::is_transient;
use crate::store::transaction_terminal::GuardedTerminal as _;
use mongodb::error::{Error, ErrorKind, WriteFailure};

const ACCEPTANCE_START_ATTEMPTS: usize = 3;
const WRITE_CONFLICT: i32 = 112;

enum StartError {
    Driver(Error),
    InvalidAuthority,
}

impl From<StartError> for CredentialCustodyError {
    fn from(error: StartError) -> Self {
        match error {
            StartError::Driver(error) => unavailable(error),
            StartError::InvalidAuthority => corrupt(),
        }
    }
}

pub(super) async fn start(
    store: &MongoSandboxCredentialStore,
    sandbox: &Sandbox,
) -> Result<ClientSession, CredentialCustodyError> {
    start_once(store, sandbox).await.map_err(Into::into)
}

pub(super) async fn start_acceptance(
    store: &MongoSandboxCredentialStore,
    sandbox: &Sandbox,
) -> Result<ClientSession, CredentialCustodyError> {
    // Keep the one-time provider result in memory while retrying a definite
    // database abort. Each attempt must establish the original work fence.
    for _ in 1..ACCEPTANCE_START_ATTEMPTS {
        match start_once(store, sandbox).await {
            Err(StartError::Driver(error)) if is_aborted_write_conflict(&error) => continue,
            result => return result.map_err(Into::into),
        }
    }
    start(store, sandbox).await
}

fn is_aborted_write_conflict(error: &Error) -> bool {
    is_transient(error)
        && match error.kind.as_ref() {
            ErrorKind::Command(error) => error.code == WRITE_CONFLICT,
            ErrorKind::Write(WriteFailure::WriteError(error)) => error.code == WRITE_CONFLICT,
            _ => false,
        }
}

async fn start_once(
    store: &MongoSandboxCredentialStore,
    sandbox: &Sandbox,
) -> Result<ClientSession, StartError> {
    if sandbox.work.authority_protocol.is_none() {
        return Err(StartError::InvalidAuthority);
    }
    let identity = crate::store::work_claim_authority::sandbox_claim(sandbox)
        .map_err(|_| StartError::InvalidAuthority)?;
    let identity = crate::store::work_publication::sandbox(identity, sandbox.id.as_str())
        .map_err(|_| StartError::InvalidAuthority)?;
    let credential = mongodb::bson::serialize_to_bson(&sandbox.credential)
        .map_err(|_| StartError::InvalidAuthority)?;
    let mut session = store
        .client
        .start_session()
        .await
        .map_err(StartError::Driver)?;
    start_transaction(&mut session)
        .await
        .map_err(StartError::Driver)?;
    let outcome = async {
        let result = store
            .sandboxes
            .update_one(
                doc! {"$and":[identity,{
                    "document_schema_version":3_i64,
                    "record.credential":credential,
                    "$or":[{"credential_issuance_fence":{"$exists":false}},
                        {"credential_issuance_fence":{"$type":"long","$lt":i64::MAX}}],
                }]},
                doc! {"$inc":{"credential_issuance_fence":1_i64}},
            )
            .session(&mut session)
            .await
            .map_err(StartError::Driver)?;
        if result.matched_count != 1 {
            return Err(StartError::InvalidAuthority);
        }
        Ok(())
    }
    .await;
    if let Err(error) = outcome {
        if let Err(abort) = session.abort_guarded().await {
            tracing::warn!(error = %abort, "credential authority abort was not acknowledged");
        }
        return Err(error);
    }
    Ok(session)
}

pub(super) async fn finish<T>(
    session: &mut ClientSession,
    outcome: Result<T, CredentialCustodyError>,
) -> Result<T, CredentialCustodyError> {
    let result = match outcome {
        Ok(result) => result,
        Err(error) => {
            if let Err(abort) = session.abort_guarded().await {
                tracing::warn!(error = %abort, "credential custody abort was not acknowledged");
            }
            return Err(error);
        }
    };
    match commit_transaction(session).await {
        CommitOutcome::Committed => Ok(result),
        CommitOutcome::Retry(error)
        | CommitOutcome::Unknown(error)
        | CommitOutcome::Failed(error) => Err(unavailable(error)),
    }
}
