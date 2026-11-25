//! Compose reporting transactions with the Agent Manager's MongoDB policy.
use crate::store::transaction_terminal::GuardedTerminal as _;
use futures_util::future::BoxFuture;
use layer2_core::sandbox_metrics::MetricsError;
use mongodb::{Client, ClientSession};

pub(crate) use cma_am_reporting::transaction_scope::{
    domain, into_domain, mongo, unavailable, Result,
};

#[cfg(test)]
pub(super) async fn finish<T>(session: &mut ClientSession, result: Result<T>) -> Result<T> {
    cma_am_reporting::transaction_scope::finish_session(
        session,
        result,
        |session| Box::pin(session.commit_guarded()),
        |session| Box::pin(session.abort_guarded()),
        |attempt| Box::pin(super::contention::backoff(attempt)),
    )
    .await
}

pub(super) async fn run<T: Send + 'static, C: Sync, F>(
    client: &Client,
    context: C,
    body: F,
) -> std::result::Result<T, MetricsError>
where
    F: for<'a> FnMut(&'a C, &'a mut ClientSession) -> BoxFuture<'a, Result<T>>,
{
    cma_am_reporting::transaction_scope::run(
        client,
        context,
        body,
        |session| Box::pin(super::transaction_barrier::open(session)),
        |session| Box::pin(session.commit_guarded()),
        |session| Box::pin(session.abort_guarded()),
        |attempt| Box::pin(super::contention::backoff(attempt)),
    )
    .await
}
