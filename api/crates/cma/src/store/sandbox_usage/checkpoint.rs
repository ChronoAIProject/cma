//! Bound live billing intervals and initialize pre-metering Sandboxes safely.
use super::*;
use crate::store::sandbox_mongo::{
    commit_transaction, is_transient, start_transaction, unsigned, CommitOutcome,
    TRANSACTION_ATTEMPTS,
};
use crate::store::transaction_terminal::GuardedTerminal as _;
use futures_util::TryStreamExt;
use std::sync::Arc;

impl MongoSandboxStore {
    pub(crate) fn spawn_usage_meter(
        self: Arc<Self>,
        mut stop: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = stop.changed() => break,
                    _ = interval.tick() => {
                        // Cancellation drops an uncommitted transaction; no provider
                        // operation is performed by this worker.
                        tokio::select! {
                            _ = stop.changed() => break,
                            result = self.checkpoint_usage() => {
                                if let Err(error) = result {
                                    tracing::error!(error = %error, "sandbox usage checkpoint failed");
                                }
                            }
                        }
                    }
                }
            }
        })
    }

    async fn checkpoint_usage(&self) -> Result<(), SandboxStoreError> {
        let mut after = String::new();
        loop {
            let mut cursor = self
                .sandboxes
                .find(doc! { "_id": { "$gt": &after } })
                .projection(doc! { "_id": 1 })
                .sort(doc! { "_id": 1 })
                .limit(100)
                .await
                .map_err(unavailable)?;
            let mut count = 0;
            while let Some(document) = cursor.try_next().await.map_err(unavailable)? {
                let id = document
                    .get_str("_id")
                    .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
                self.checkpoint_usage_one(id, &layer2_core::now_rfc3339())
                    .await?;
                after = id.to_owned();
                count += 1;
            }
            if count < 100 {
                return Ok(());
            }
            tokio::task::yield_now().await;
        }
    }

    pub(crate) async fn checkpoint_usage_one(
        &self,
        id: &str,
        at: &str,
    ) -> Result<(), SandboxStoreError> {
        self.checkpoint_usage_observed(id, at, false).await
    }

    pub(crate) async fn checkpoint_usage_observed(
        &self,
        id: &str,
        at: &str,
        funded: bool,
    ) -> Result<(), SandboxStoreError> {
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = self.client.start_session().await.map_err(unavailable)?;
            start_transaction(&mut session).await.map_err(unavailable)?;
            let result = checkpoint(self, &mut session, id, at, funded).await;
            match result {
                Ok(()) => {}
                Err(TransactionEventError::Driver(error)) if is_transient(&error) => {
                    let _ = session.abort_guarded().await;
                    continue;
                }
                Err(TransactionEventError::Driver(error)) => return Err(unavailable(error)),
                Err(TransactionEventError::Domain(error)) => return Err(error),
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(()),
                CommitOutcome::Retry(_) => continue,
                // Retry on the next sweep with the same source records. A committed
                // interval will already have advanced last_observed_at.
                CommitOutcome::Unknown(error) | CommitOutcome::Failed(error) => {
                    return Err(unavailable(error))
                }
            }
        }
        Err(SandboxStoreError::Conflict)
    }
}

pub(in crate::store) async fn checkpoint(
    store: &MongoSandboxStore,
    session: &mut ClientSession,
    id: &str,
    at: &str,
    funded: bool,
) -> Result<(), TransactionEventError> {
    use TransactionEventError::{Domain, Driver};
    let Some(document) = store
        .sandboxes
        .find_one(doc! { "_id": id })
        .session(&mut *session)
        .await
        .map_err(Driver)?
    else {
        return Ok(());
    };
    let sandbox = crate::store::sandboxes::read_sandbox(&document).map_err(Domain)?;
    let seq = document
        .get_i64("last_seq")
        .map_err(|error| Domain(SandboxStoreError::Corrupt(error.to_string())))?;
    let seq = unsigned(seq, "sandbox sequence")
        .map_err(|error| Domain(SandboxStoreError::Corrupt(error)))?;
    record(
        store,
        session,
        &sandbox,
        seq,
        at,
        (seq, sandbox.revision),
        if store.usage_capture.is_some() {
            UsageObservation::CaptureCheckpoint
        } else if funded {
            UsageObservation::FundedCheckpoint
        } else {
            UsageObservation::Checkpoint
        },
    )
    .await
}
fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}
