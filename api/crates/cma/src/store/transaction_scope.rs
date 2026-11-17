//! Complete each reporting transaction before returning or opening a fresh snapshot.
use futures_util::future::BoxFuture;
use layer2_core::sandbox_metrics::MetricsError;
use mongodb::{
    bson::Document,
    error::{Error, TRANSIENT_TRANSACTION_ERROR, UNKNOWN_TRANSACTION_COMMIT_RESULT},
    Client, ClientSession,
};

const ATTEMPTS: usize = 5;
const COMMIT_ATTEMPTS: usize = 5;
const ABORT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Debug)]
pub(crate) enum ErrorKind {
    Mongo(Error),
    Uncertain(Error),
    Command(Document),
    Domain(MetricsError),
}
pub(crate) type Result<T> = std::result::Result<T, ErrorKind>;
pub(super) use ErrorKind::{Domain as domain, Mongo as mongo};

impl ErrorKind {
    fn retryable(&self) -> bool {
        match self {
            Self::Mongo(error) => {
                error.contains_label(TRANSIENT_TRANSACTION_ERROR)
                    && !error.contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT)
            }
            Self::Command(response) => {
                let labelled = |label: &str| {
                    response.get_array("errorLabels").is_ok_and(|labels| {
                        labels.iter().any(|value| value.as_str() == Some(label))
                    })
                };
                labelled(TRANSIENT_TRANSACTION_ERROR)
                    && !labelled(UNKNOWN_TRANSACTION_COMMIT_RESULT)
            }
            Self::Uncertain(_) | Self::Domain(_) => false,
        }
    }
}
impl From<MetricsError> for ErrorKind {
    fn from(error: MetricsError) -> Self {
        Self::Domain(error)
    }
}
pub(super) fn unavailable(error: impl std::fmt::Display) -> ErrorKind {
    domain(super::usage_reports::unavailable(error))
}
pub(super) fn into_domain(error: ErrorKind) -> MetricsError {
    match error {
        ErrorKind::Domain(error) => error,
        ErrorKind::Mongo(error) | ErrorKind::Uncertain(error) => {
            super::usage_reports::unavailable(error)
        }
        ErrorKind::Command(_) => MetricsError::Unavailable,
    }
}

// Context borrows share the session borrow's lifetime; retries cannot retain
// cursors or mutable observations from an earlier attempt.
pub(super) async fn run<T, C: Sync, F>(
    client: &Client,
    context: C,
    mut body: F,
) -> std::result::Result<T, MetricsError>
where
    F: for<'a> FnMut(&'a C, &'a mut ClientSession) -> BoxFuture<'a, Result<T>>,
{
    for attempt in 0..ATTEMPTS {
        super::contention::backoff(attempt).await;
        let mut session = client
            .start_session()
            .await
            .map_err(super::usage_reports::unavailable)?;
        let result = match session.start_transaction().await {
            Ok(()) => body(&context, &mut session).await,
            Err(error) => Err(mongo(error)),
        };
        match finish(&mut session, result).await {
            Ok(value) => return Ok(value),
            Err(error) if error.retryable() && attempt + 1 < ATTEMPTS => {}
            Err(error) => return Err(into_domain(error)),
        }
    }
    unreachable!("the final attempt returns its original error")
}

#[async_trait::async_trait]
pub(super) trait Transaction: Send {
    async fn commit(&mut self) -> mongodb::error::Result<()>;
    async fn abort(&mut self) -> mongodb::error::Result<()>;
}
#[async_trait::async_trait]
impl Transaction for ClientSession {
    async fn commit(&mut self) -> mongodb::error::Result<()> {
        self.commit_transaction().await
    }
    async fn abort(&mut self) -> mongodb::error::Result<()> {
        self.abort_transaction().await
    }
}

pub(super) async fn finish<T>(session: &mut impl Transaction, result: Result<T>) -> Result<T> {
    let result = match result {
        Ok(value) => commit(session).await.map(|()| value),
        Err(error) => Err(error),
    };
    if result.is_err() {
        // Best effort: neither an abort failure nor its timeout replaces the
        // body/commit error. Cancellation still relies on the driver's drop.
        match tokio::time::timeout(ABORT_DEADLINE, session.abort()).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::warn!(%error, "reporting transaction abort unconfirmed"),
            Err(_) => tracing::warn!("reporting transaction abort timed out"),
        }
    }
    result
}

async fn commit(session: &mut impl Transaction) -> Result<()> {
    let mut uncertain = false;
    for attempt in 0..COMMIT_ATTEMPTS {
        match session.commit().await {
            Ok(()) => return Ok(()),
            Err(error) if error.contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT) => {
                uncertain = true;
                if attempt + 1 == COMMIT_ATTEMPTS {
                    return Err(ErrorKind::Uncertain(error));
                }
                super::contention::backoff(attempt + 1).await;
            }
            // Even a later transient label cannot disprove an earlier commit.
            Err(error) if uncertain => return Err(ErrorKind::Uncertain(error)),
            Err(error) => return Err(mongo(error)),
        }
    }
    unreachable!("the final commit attempt returns")
}

#[cfg(test)]
#[path = "usage_reports/transaction_scope_tests.rs"]
mod tests;
