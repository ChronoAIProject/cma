//! Bounded lifecycle transactions with snapshot proof after an uncertain commit.
use super::quota::{unavailable, IO};
use crate::store::{
    commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
};
use layer2_core::document_artifacts::DocumentError;
use mongodb::{Client, ClientSession};

pub(super) enum Error {
    Domain(DocumentError),
    Driver(mongodb::error::Error),
}
impl From<DocumentError> for Error {
    fn from(value: DocumentError) -> Self {
        Self::Domain(value)
    }
}
impl From<mongodb::error::Error> for Error {
    fn from(value: mongodb::error::Error) -> Self {
        Self::Driver(value)
    }
}
#[async_trait::async_trait]
pub(super) trait Operation: Sync {
    type Output: Send;
    async fn apply(&self, session: &mut ClientSession) -> Result<Self::Output, Error>;
    async fn reconcile(&self, snapshot: &mut ClientSession) -> Result<Self::Output, DocumentError>;
    async fn duplicate(&self) -> Result<Option<Self::Output>, DocumentError> {
        Ok(None)
    }
}
pub(super) async fn run<O: Operation>(
    client: &Client,
    operation: O,
) -> Result<O::Output, DocumentError> {
    tokio::time::timeout(IO, async {
        for attempt in 0..8 {
            let mut session = client.start_session().await.map_err(unavailable)?;
            if let Err(error) = start_transaction(&mut session).await {
                if is_transient(&error) {
                    continue;
                }
                return Err(DocumentError::Unavailable);
            }
            let applied = match operation.apply(&mut session).await {
                Ok(value) => value,
                Err(Error::Domain(error)) => {
                    let _ = session.abort_transaction().await;
                    return Err(error);
                }
                Err(Error::Driver(error)) if is_duplicate_key(&error) => {
                    let _ = session.abort_transaction().await;
                    if let Some(outcome) = operation.duplicate().await? {
                        return Ok(outcome);
                    }
                    continue;
                }
                Err(Error::Driver(error)) if is_transient(&error) => {
                    let _ = session.abort_transaction().await;
                    tokio::time::sleep(std::time::Duration::from_millis(attempt + 1)).await;
                    continue;
                }
                Err(_) => {
                    let _ = session.abort_transaction().await;
                    return Err(DocumentError::Unavailable);
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(applied),
                CommitOutcome::Retry(_) => {
                    let _ = session.abort_transaction().await;
                }
                CommitOutcome::Unknown(_) => {
                    let time = session.operation_time().ok_or(DocumentError::Unavailable)?;
                    let mut snapshot = client
                        .start_session()
                        .snapshot(true)
                        .snapshot_time(time)
                        .await
                        .map_err(unavailable)?;
                    return operation.reconcile(&mut snapshot).await;
                }
                CommitOutcome::Failed(_) => return Err(DocumentError::Unavailable),
            }
        }
        Err(DocumentError::Unavailable)
    })
    .await
    .map_err(|_| DocumentError::Unavailable)?
}
