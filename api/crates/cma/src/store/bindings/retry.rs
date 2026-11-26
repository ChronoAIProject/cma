//! Binding recovery retries only transactions MongoDB proves aborted.
use super::*;
use crate::store::sandbox_mongo::{
    commit_transaction, is_transient, start_transaction, CommitOutcome, TRANSACTION_ATTEMPTS,
};
use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::sandbox::binding::ProvidedCredential;
use layer2_core::CoreError;

mod error;
mod transaction;
pub(in crate::store) use error::BindingTransactionError;
use transaction::RecoveryParents;

impl MongoBindingCredentials {
    pub(super) async fn retry_recovered(
        &self,
        binding: &ProviderBinding,
        credential: Option<&ProvidedCredential>,
    ) -> Result<ProviderBinding, CoreError> {
        // Identity and intent do not change when an aborted transaction reads a
        // fresh hierarchy. The eventual write also retains this operation time.
        let at = layer2_core::now_rfc3339();
        let mut captured = RecoveryParents::default();
        for attempt in 0..TRANSACTION_ATTEMPTS {
            if attempt > 0 {
                tokio::task::yield_now().await;
            }
            let mut session = match self.store.client.start_session().await {
                Ok(session) => session,
                Err(error) if known_abort(&error) => continue,
                Err(error) => return Err(unavailable(error)),
            };
            if let Err(error) = start_transaction(&mut session).await {
                let _ = session.abort_guarded().await;
                if known_abort(&error) {
                    continue;
                }
                return Err(unavailable(error));
            }
            let next = match self
                .recover_in(binding, credential, &at, &mut captured, &mut session)
                .await
            {
                Ok(next) => next,
                Err(BindingTransactionError::Driver(error)) if known_abort(&error) => {
                    let _ = session.abort_guarded().await;
                    continue;
                }
                Err(error) => {
                    let _ = session.abort_guarded().await;
                    return Err(error.into_core(unavailable));
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(next),
                CommitOutcome::Retry(_) => {
                    let _ = session.abort_guarded().await;
                }
                // Unknown custody cannot authorize a new body, even if a later
                // acknowledgement looks like a transient transaction failure.
                CommitOutcome::Unknown(error) => return Err(unavailable(error)),
                CommitOutcome::Failed(error) => {
                    let _ = session.abort_guarded().await;
                    return Err(unavailable(error));
                }
            }
        }
        Err(unavailable("binding recovery retry budget exhausted"))
    }
}

fn known_abort(error: &mongodb::error::Error) -> bool {
    is_transient(error) && !error.contains_label(mongodb::error::UNKNOWN_TRANSACTION_COMMIT_RESULT)
}

fn conflict() -> CoreError {
    CoreError::InvalidState("binding recovery authority changed or is busy".into())
}
fn unavailable(_: impl std::fmt::Display) -> CoreError {
    CoreError::RuntimeUnavailable("binding recovery admission unavailable".into())
}

#[cfg(test)]
mod tests;
