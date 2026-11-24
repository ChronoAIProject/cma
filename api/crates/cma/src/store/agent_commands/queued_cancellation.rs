use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::sandbox::agent_io::{
    CommandLedgerError, QueuedCancellationOutcome, QueuedCancellationReservation,
};

use super::ApplyError;
use crate::store::agent_log_segments::{
    command_worker, terminal, transaction_scope::TransactionScope,
};
use crate::store::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, CommitOutcome, MongoSandboxStore,
    TRANSACTION_ATTEMPTS,
};

pub(in crate::store) mod apply;
pub(super) mod fences;

pub(super) async fn run(
    store: &MongoSandboxStore,
    request: QueuedCancellationReservation,
) -> Result<QueuedCancellationOutcome, CommandLedgerError> {
    request.validate()?;
    for _ in 0..TRANSACTION_ATTEMPTS {
        request.command.require_current_authority()?;
        let mut session = store
            .client
            .start_session()
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?;
        let mut scope = TransactionScope::start(&mut session)
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?;
        let applied = Box::pin(command_worker::cancel(store, &request, &mut scope)).await;
        let session = scope.release();
        let (result, receipt) = match applied {
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
        if let Err(error) = request.command.require_current_authority() {
            let _ = session.abort_guarded().await;
            return Err(error);
        }
        match commit_transaction(session).await {
            CommitOutcome::Committed => return Ok(result),
            CommitOutcome::Retry(_) => continue,
            CommitOutcome::Unknown(_) => {
                if terminal::reconcile(store, receipt.as_ref(), session.operation_time()).await? {
                    return Ok(result);
                }
                return Err(CommandLedgerError::Unavailable);
            }
            CommitOutcome::Failed(_) => return Err(CommandLedgerError::Unavailable),
        }
    }
    Err(CommandLedgerError::Conflict)
}
