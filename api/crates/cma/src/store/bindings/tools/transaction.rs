//! Retry only transactions MongoDB definitively aborted; reread all authority.
use super::*;
use crate::store::resource_access::ReadError;
use crate::store::sandbox_mongo::{
    commit_transaction, is_transient, start_transaction, CommitOutcome, TRANSACTION_ATTEMPTS,
};
use crate::store::transaction_terminal::GuardedTerminal as _;

pub(super) enum Operation<'a> {
    Resolve(&'a RuntimeHandle, &'a AgentOpenRequest, u64),
    Confirm(&'a HostedProviderToolBinding, &'a str),
    Transport(&'a HostedProviderToolBinding, Option<&'a str>, &'a str, u64),
    Authenticate(&'a str, bool),
}

impl Operation<'_> {
    fn agent_id(&self) -> &str {
        match self {
            Self::Resolve(_, request, _) => &request.options.slot.0,
            Self::Confirm(binding, _) => &binding.scope().1.options.slot.0,
            Self::Transport(binding, ..) => &binding.scope().1.options.slot.0,
            Self::Authenticate(..) => "relay-capability",
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Resolve(..) => "resolve",
            Self::Confirm(..) => "confirm_open",
            Self::Transport(_, Some(_), ..) => "prepare_transport",
            Self::Transport(_, None, ..) => "confirm_transport",
            Self::Authenticate(..) => "authenticate_relay",
        }
    }
}

pub(super) enum Outcome {
    Resolved(Option<Box<HostedProviderToolBinding>>),
    Confirmed,
    Authenticated(Box<relay_auth::AuthenticatedProviderTool>),
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
                    if retry_driver(&operation, "start_session", attempt, &error) {
                        continue;
                    }
                    return Err(unavailable(()));
                }
            };
            if let Err(error) = start_transaction(&mut session).await {
                let _ = session.abort_guarded().await;
                if retry_driver(&operation, "start_transaction", attempt, &error) {
                    continue;
                }
                return Err(unavailable(()));
            }
            let result = match operation {
                Operation::Resolve(runtime, request, fence) => self
                    .resolve_in(runtime, request, fence, &mut session)
                    .await
                    .map(|value| Outcome::Resolved(value.map(|(binding, _)| Box::new(binding)))),
                Operation::Confirm(binding, conversation) => self
                    .confirm_in(binding, conversation, &mut session)
                    .await
                    .map(|()| Outcome::Confirmed),
                Operation::Transport(binding, origin, conversation, sequence) => self
                    .relay_transport_in(binding, origin, conversation, sequence, &mut session)
                    .await
                    .map(|binding| Outcome::Resolved(Some(Box::new(binding)))),
                Operation::Authenticate(capability, setup) => self
                    .relay_auth_in(capability, setup, &mut session)
                    .await
                    .map(|value| Outcome::Authenticated(Box::new(value))),
            };
            let outcome = match result {
                Ok(outcome) => outcome,
                Err(error) => {
                    let _ = session.abort_guarded().await;
                    match error {
                        Error::Driver(error) => {
                            if retry_driver(&operation, "read_write", attempt, &error) {
                                continue;
                            }
                            return Err(unavailable(()));
                        }
                        Error::Domain(error) => {
                            let category = match &error {
                                AdapterError::Rejected(_) => "authority_rejected",
                                _ => "invalid_storage_record",
                            };
                            diagnostic(&operation, "read_write", category, attempt);
                            return Err(error);
                        }
                    }
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(outcome),
                CommitOutcome::Retry(_) => {
                    let _ = session.abort_guarded().await;
                    diagnostic(&operation, "commit", "transient_transaction", attempt);
                }
                // An unknown commit may have installed the receipt. Do not
                // start a new transaction or report success without proof.
                CommitOutcome::Unknown(_) => {
                    diagnostic(&operation, "commit", "unknown_commit", attempt);
                    return Err(unavailable(()));
                }
                CommitOutcome::Failed(_) => {
                    let _ = session.abort_guarded().await;
                    diagnostic(&operation, "commit", "driver_failure", attempt);
                    return Err(unavailable(()));
                }
            }
        }
        diagnostic(
            &operation,
            "retry",
            "retry_exhausted",
            TRANSACTION_ATTEMPTS - 1,
        );
        Err(unavailable(()))
    }
}

fn retry_driver(
    operation: &Operation<'_>,
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
    operation: &Operation<'_>,
    stage: &'static str,
    category: &'static str,
    attempt: usize,
) {
    // Driver text and BSON may contain capabilities or owner data. Log only
    // fixed categories and the bounded attempt number, never error bodies.
    tracing::warn!(
        operation = operation.name(),
        agent_id = operation.agent_id(),
        stage,
        category,
        attempt = attempt + 1,
        "Provider tool authority transaction did not complete"
    );
}
