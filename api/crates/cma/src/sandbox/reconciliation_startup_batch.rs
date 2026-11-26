//! Admit a bounded number of binding operations and drain every admitted mutation.
use std::future::Future;

use futures_util::{stream::FuturesUnordered, StreamExt};

pub(super) struct Report<E> {
    pub(super) started: usize,
    pub(super) settled: usize,
    pub(super) deferred: usize,
    pub(super) peak: usize,
    pub(super) error: Option<E>,
}

pub(super) async fn run<T, E, F, Fut>(
    items: impl IntoIterator<Item = T>,
    limit: usize,
    stopping: impl Fn() -> bool,
    mut operation: F,
) -> Report<E>
where
    F: FnMut(T) -> Fut,
    Fut: Future<Output = Result<bool, E>>,
{
    assert!(limit > 0);
    let mut items = items.into_iter();
    let mut active = FuturesUnordered::new();
    let mut report = Report {
        started: 0,
        settled: 0,
        deferred: 0,
        peak: 0,
        error: None,
    };
    loop {
        // An observed error permanently closes admission, including while draining.
        while report.error.is_none() && active.len() < limit && !stopping() {
            let Some(item) = items.next() else { break };
            active.push(operation(item));
            report.started += 1;
            report.peak = report.peak.max(active.len());
        }
        let Some(result) = active.next().await else {
            break;
        };
        report.settled += 1;
        match result {
            Ok(false) => report.deferred += 1,
            Ok(true) => {}
            Err(error) if report.error.is_none() => report.error = Some(error),
            Err(_) => {}
        }
    }
    report
}

#[cfg(test)]
#[path = "reconciliation_startup_batch_tests.rs"]
mod tests;
