//! Retry only transactions MongoDB definitively aborted; reread all authority.
use super::*;
use crate::store::resource_access::ReadError;
use crate::store::sandbox_mongo::{
    commit_transaction, is_transient, start_transaction, CommitOutcome, TRANSACTION_ATTEMPTS,
};

pub(super) enum Operation<'a> {
    Resolve(&'a RuntimeHandle, &'a AgentOpenRequest, u64),
    Confirm(&'a HostedProviderToolBinding, &'a str),
}

impl Operation<'_> {
    fn name(&self) -> &'static str {
        match self {
            Self::Resolve(..) => "resolve",
            Self::Confirm(..) => "confirm_open",
        }
    }
}

pub(super) enum Outcome {
    Resolved(Option<HostedProviderToolBinding>),
    Confirmed,
}

pub(super) enum Error {
    Domain(AdapterError),
    Driver(mongodb::error::Error),
}

impl From<AdapterError> for Error {
    fn from(error: AdapterError) -> Self {
        Self::Domain(error)
    }
}

impl From<mongodb::error::Error> for Error {
    fn from(error: mongodb::error::Error) -> Self {
        Self::Driver(error)
    }
}

impl From<ReadError> for Error {
    fn from(error: ReadError) -> Self {
        match error {
            ReadError::Driver(error) => Self::Driver(error),
            ReadError::Domain(_) => Self::Domain(unavailable(())),
        }
    }
}

pub(super) type Result<T> = std::result::Result<T, Error>;

impl MongoProviderToolSource {
    pub(super) async fn run(&self, operation: Operation<'_>) -> layer3_adapter::Result<Outcome> {
        let name = operation.name();
        for attempt in 0..TRANSACTION_ATTEMPTS {
            if attempt > 0 {
                // Startup output also fences the Sandbox/Agent. Give that writer
                // time to commit before reading a fresh authority snapshot.
                tokio::time::sleep(std::time::Duration::from_millis(10 << (attempt - 1).min(3)))
                    .await;
            }
            let mut session = match self.store.client.start_session().await {
                Ok(session) => session,
                Err(error) => {
                    if retry_driver(name, "start_session", attempt, &error) {
                        continue;
                    }
                    return Err(unavailable(()));
                }
            };
            if let Err(error) = start_transaction(&mut session).await {
                let _ = session.abort_transaction().await;
                if retry_driver(name, "start_transaction", attempt, &error) {
                    continue;
                }
                return Err(unavailable(()));
            }
            let result = match operation {
                Operation::Resolve(runtime, request, fence) => self
                    .resolve_in(runtime, request, fence, &mut session)
                    .await
                    .map(|value| Outcome::Resolved(value.map(|(binding, _)| binding))),
                Operation::Confirm(binding, conversation) => self
                    .confirm_in(binding, conversation, &mut session)
                    .await
                    .map(|()| Outcome::Confirmed),
            };
            let outcome = match result {
                Ok(outcome) => outcome,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    match error {
                        Error::Driver(error) => {
                            if retry_driver(name, "read_write", attempt, &error) {
                                continue;
                            }
                            return Err(unavailable(()));
                        }
                        Error::Domain(error) => {
                            let category = match &error {
                                AdapterError::Rejected(_) => "authority_rejected",
                                _ => "invalid_storage_record",
                            };
                            diagnostic(name, "read_write", category, attempt);
                            return Err(error);
                        }
                    }
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(outcome),
                CommitOutcome::Retry(_) => {
                    let _ = session.abort_transaction().await;
                    diagnostic(name, "commit", "transient_transaction", attempt);
                }
                // An unknown commit may have installed the receipt. Do not
                // start a new transaction or report success without proof.
                CommitOutcome::Unknown(_) => {
                    diagnostic(name, "commit", "unknown_commit", attempt);
                    return Err(unavailable(()));
                }
                CommitOutcome::Failed(_) => {
                    let _ = session.abort_transaction().await;
                    diagnostic(name, "commit", "driver_failure", attempt);
                    return Err(unavailable(()));
                }
            }
        }
        diagnostic(name, "retry", "retry_exhausted", TRANSACTION_ATTEMPTS - 1);
        Err(unavailable(()))
    }
}

fn retry_driver(
    operation: &'static str,
    stage: &'static str,
    attempt: usize,
    error: &mongodb::error::Error,
) -> bool {
    let retry = is_transient(error);
    diagnostic(
        operation,
        stage,
        if retry {
            "transient_transaction"
        } else {
            "driver_failure"
        },
        attempt,
    );
    retry
}

fn diagnostic(
    operation: &'static str,
    stage: &'static str,
    category: &'static str,
    attempt: usize,
) {
    // Driver text and BSON may contain capabilities or owner data. Log only
    // fixed categories and the bounded attempt number, never error bodies.
    tracing::warn!(
        operation,
        stage,
        category,
        attempt = attempt + 1,
        "Provider tool authority transaction did not complete"
    );
}
