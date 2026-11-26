use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::sandbox::agent_io::{
    CommandLedgerError, QueuedInputOutcome, QueuedInputReservation,
};

use super::ApplyError;
use crate::store::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
    MongoSandboxStore, TRANSACTION_ATTEMPTS,
};

mod apply;

pub(super) async fn run(
    store: &MongoSandboxStore,
    request: QueuedInputReservation,
) -> Result<QueuedInputOutcome, CommandLedgerError> {
    request.validate()?;
    for _ in 0..TRANSACTION_ATTEMPTS {
        request.cancellation.command.require_current_authority()?;
        let mut session = store
            .client
            .start_session()
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?;
        start_transaction(&mut session)
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?;
        let result = match apply::apply(store, &request, &mut session).await {
            Ok(QueuedInputOutcome::StaleObservation) => {
                session
                    .abort_guarded()
                    .await
                    .map_err(|_| CommandLedgerError::Unavailable)?;
                return Ok(QueuedInputOutcome::StaleObservation);
            }
            Ok(result) => result,
            Err(ApplyError::Domain(error)) => {
                let _ = session.abort_guarded().await;
                return Err(error);
            }
            Err(ApplyError::Driver(error)) => {
                let _ = session.abort_guarded().await;
                if is_transient(&error) || is_duplicate_key(&error) {
                    tokio::task::yield_now().await;
                    continue;
                }
                return Err(CommandLedgerError::Unavailable);
            }
        };
        if let Err(error) = request.cancellation.command.require_current_authority() {
            let _ = session.abort_guarded().await;
            return Err(error);
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(result),
            CommitOutcome::Retry(_) => continue,
            CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                return Err(CommandLedgerError::Unavailable)
            }
        }
    }
    Err(CommandLedgerError::Conflict)
}
