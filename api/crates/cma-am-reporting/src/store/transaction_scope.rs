//! Complete each reporting transaction before returning or opening a fresh snapshot.
use futures_util::future::BoxFuture;
use layer2_core::sandbox_metrics::MetricsError;
use mongodb::{
    bson::Document,
    error::{Error, TRANSIENT_TRANSACTION_ERROR, UNKNOWN_TRANSACTION_COMMIT_RESULT},
    Client, ClientSession,
};
use tracing::instrument::WithSubscriber;

const ATTEMPTS: usize = 5;
const COMMIT_ATTEMPTS: usize = 5;
const ABORT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Debug)]
pub enum ErrorKind {
    Mongo(Error),
    Uncertain(Error),
    Command(Document),
    Domain(MetricsError),
}
pub type Result<T> = std::result::Result<T, ErrorKind>;
pub use ErrorKind::{Domain as domain, Mongo as mongo};

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
pub fn unavailable(error: impl std::fmt::Display) -> ErrorKind {
    domain(super::usage_reports::unavailable(error))
}
pub fn into_domain(error: ErrorKind) -> MetricsError {
    match error {
        ErrorKind::Domain(error) => error,
        ErrorKind::Mongo(error) | ErrorKind::Uncertain(error) => {
            super::usage_reports::unavailable(error)
        }
        ErrorKind::Command(_) => MetricsError::Unavailable,
    }
}

type SessionOperation =
    for<'a> fn(&'a mut ClientSession) -> BoxFuture<'a, mongodb::error::Result<()>>;

#[derive(Clone, Copy)]
pub struct Policy {
    pub open: SessionOperation,
    pub commit: SessionOperation,
    pub abort: SessionOperation,
    pub backoff: fn(usize) -> BoxFuture<'static, ()>,
}

// Context borrows share the session borrow's lifetime; retries cannot retain
// cursors or mutable observations from an earlier attempt.
pub async fn run<T: Send + 'static, C: Sync, F>(
    client: &Client,
    context: C,
    mut body: F,
    policy: Policy,
) -> std::result::Result<T, MetricsError>
where
    F: for<'a> FnMut(&'a C, &'a mut ClientSession) -> BoxFuture<'a, Result<T>>,
{
    for attempt in 0..ATTEMPTS {
        (policy.backoff)(attempt).await;
        let mut session = client
            .start_session()
            .await
            .map_err(super::usage_reports::unavailable)?;
        let result = match session.start_transaction().await {
            Ok(()) => match (policy.open)(&mut session).await {
                Ok(()) => body(&context, &mut session).await,
                Err(error) => Err(mongo(error)),
            },
            Err(error) => Err(mongo(error)),
        };
        // The driver changes transaction state before sending commit/abort.
        // Own that terminal operation so caller cancellation cannot strand a
        // server transaction that session drop would no longer abort.
        let finished = tokio::spawn(
            async move { finish_session(&mut session, result, policy).await }
                .with_current_subscriber(),
        )
        .await
        .map_err(super::usage_reports::unavailable)?;
        match finished {
            Ok(value) => return Ok(value),
            Err(error) if error.retryable() && attempt + 1 < ATTEMPTS => {}
            Err(error) => return Err(into_domain(error)),
        }
    }
    unreachable!("the final attempt returns its original error")
}

/// Finish an existing reporting transaction with the same terminal retry policy.
pub async fn finish_session<T>(
    session: &mut ClientSession,
    result: Result<T>,
    policy: Policy,
) -> Result<T> {
    let mut terminal = SessionTerminal {
        session,
        commit: policy.commit,
        abort: policy.abort,
    };
    finish(&mut terminal, result, policy.backoff).await
}

#[async_trait::async_trait]
trait Transaction: Send {
    async fn commit(&mut self) -> mongodb::error::Result<()>;
    async fn abort(&mut self) -> mongodb::error::Result<()>;
}
struct SessionTerminal<'a> {
    session: &'a mut ClientSession,
    commit: SessionOperation,
    abort: SessionOperation,
}

#[async_trait::async_trait]
impl Transaction for SessionTerminal<'_> {
    async fn commit(&mut self) -> mongodb::error::Result<()> {
        (self.commit)(self.session).await
    }

    async fn abort(&mut self) -> mongodb::error::Result<()> {
        (self.abort)(self.session).await
    }
}

async fn finish<T>(
    session: &mut impl Transaction,
    result: Result<T>,
    backoff: fn(usize) -> BoxFuture<'static, ()>,
) -> Result<T> {
    if result.is_err() {
        // Best effort: neither an abort failure nor its timeout replaces the
        // body error. Cancellation during the body relies on the driver's drop.
        match tokio::time::timeout(ABORT_DEADLINE, session.abort()).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::warn!(target: "cma::store::transaction_scope", %error, "reporting transaction abort unconfirmed")
            }
            Err(_) => {
                tracing::warn!(target: "cma::store::transaction_scope", "reporting transaction abort timed out")
            }
        }
    }
    // After a commit attempt, the server has aborted or the driver refuses
    // abort. Return the commit outcome without a spurious abort warning.
    match result {
        Ok(value) => commit(session, backoff).await.map(|()| value),
        Err(error) => Err(error),
    }
}

async fn commit(
    session: &mut impl Transaction,
    backoff: fn(usize) -> BoxFuture<'static, ()>,
) -> Result<()> {
    let mut uncertain = false;
    for attempt in 0..COMMIT_ATTEMPTS {
        match session.commit().await {
            Ok(()) => return Ok(()),
            Err(error) if error.contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT) => {
                uncertain = true;
                if attempt + 1 == COMMIT_ATTEMPTS {
                    return Err(ErrorKind::Uncertain(error));
                }
                backoff(attempt + 1).await;
            }
            // Even a later transient label cannot disprove an earlier commit.
            Err(error) if uncertain => return Err(ErrorKind::Uncertain(error)),
            Err(error) => return Err(mongo(error)),
        }
    }
    unreachable!("the final commit attempt returns")
}

#[cfg(test)]
#[path = "transaction_scope_tests.rs"]
mod tests;
