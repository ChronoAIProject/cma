use super::*;
use crate::store::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
    TRANSACTION_ATTEMPTS,
};
use crate::store::transaction_terminal::GuardedTerminal as _;
use futures_util::future::BoxFuture;

pub(super) async fn run<T: Send + 'static, C: Sync, F>(
    store: &CaptureStore,
    context: C,
    mut body: F,
) -> Result<T, SandboxStoreError>
where
    F: for<'a> FnMut(
        &'a C,
        &'a mut ClientSession,
    ) -> BoxFuture<'a, Result<T, TransactionEventError>>,
{
    for attempt in 0..TRANSACTION_ATTEMPTS {
        super::super::retry_delay(attempt).await;
        let mut session = store.client.start_session().await.map_err(unavailable)?;
        start_transaction(&mut session).await.map_err(unavailable)?;
        let value = match body(&context, &mut session).await {
            Ok(value) => value,
            Err(error) => {
                let _ = session.abort_guarded().await;
                match error {
                    TransactionEventError::Driver(e)
                        if is_transient(&e) || is_duplicate_key(&e) =>
                    {
                        continue
                    }
                    TransactionEventError::Driver(e) => return Err(unavailable(e)),
                    TransactionEventError::Domain(e) => return Err(e),
                }
            }
        };
        let authorities = layer2_core::sandbox::store::WorkAuthority::current()?;
        // Task locals do not cross spawn; retain and check the admitted chain.
        let outcome = tokio::spawn(async move {
            for authority in authorities {
                if let Err(error) = authority.deadline() {
                    let _ = session.abort_guarded().await;
                    return CommitOutcome::Failed(mongodb::error::Error::custom(error));
                }
            }
            commit_transaction(&mut session).await
        })
        .await
        .map_err(unavailable)?;
        match outcome {
            CommitOutcome::Committed => return Ok(value),
            CommitOutcome::Retry(_) => continue,
            CommitOutcome::Unknown(e) | CommitOutcome::Failed(e) => return Err(unavailable(e)),
        }
    }
    Err(SandboxStoreError::Conflict)
}
