//! One source's reconnect loop. A fresh lease is acquired for every attempt.
use super::{
    status::{FailureCode, StatusAttempt, StatusRegistry},
    EndReason, GatewayEnd, GatewayError,
};
use futures_util::future::BoxFuture;
use std::{sync::Arc, time::Duration};
use tokio::sync::watch;

pub(super) trait SourcePort: Send + Sync {
    /// Return the current eligible source revision; no credential is returned.
    fn revision<'a>(&'a self, record: &'a str) -> BoxFuture<'a, Result<String, GatewayError>>;
    fn connect<'a>(
        &'a self,
        record: &'a str,
        stop: watch::Receiver<bool>,
        status: StatusAttempt,
    ) -> BoxFuture<'a, Attempt>;
}
pub(super) struct Attempt {
    pub result: Result<GatewayEnd, GatewayError>,
    pub cooldown: Duration,
}
#[derive(Clone, Copy)]
pub(super) struct Timing {
    pub poll: Duration,
    pub retry: Duration,
    pub stop: Duration,
}
impl Default for Timing {
    fn default() -> Self {
        Self {
            poll: Duration::from_secs(10),
            retry: Duration::from_secs(5),
            stop: Duration::from_secs(5),
        }
    }
}
pub(super) async fn run(
    port: Arc<dyn SourcePort>,
    record: String,
    mut shutdown: watch::Receiver<bool>,
    timing: Timing,
    statuses: StatusRegistry,
) {
    let mut status = statuses.begin(&record, None);
    let mut terminal_revision: Option<(String, super::status::Stage, FailureCode)> = None;
    let mut status_revision = None;
    let mut failures = 0u32;
    loop {
        if *shutdown.borrow() {
            break;
        }
        let revision = tokio::select! {
            changed = shutdown.changed() => { if changed.is_err() || *shutdown.borrow() { break; } else { continue; } },
            revision = port.revision(&record) => revision,
        };
        let revision = match revision {
            Ok(revision) => revision,
            Err(error) => {
                status = statuses.begin(&record, None);
                status_revision = None;
                status.finish(error.into(), false, timing.poll);
                if stopped(&mut shutdown, timing.poll).await {
                    break;
                }
                continue;
            }
        };
        if let Some((_, stage, code)) = terminal_revision
            .as_ref()
            .filter(|(saved, _, _)| saved == &revision)
        {
            if status_revision.as_ref() != Some(&revision) {
                status = statuses.begin(&record, Some(&revision));
                status.step(*stage);
                status.finish(*code, true, Duration::ZERO);
                status_revision = Some(revision.clone());
            }
            status.poll();
            if stopped(&mut shutdown, timing.poll).await {
                break;
            }
            continue;
        }
        status = statuses.begin(&record, Some(&revision));
        status_revision = Some(revision.clone());
        let (stop, receiver) = watch::channel(false);
        let attempt = port.connect(&record, receiver, status.clone());
        tokio::pin!(attempt);
        // Polling authority is independent of the socket future; neither blocks
        // the other. Routing removal is also an eligibility revocation.
        let authority = async {
            loop {
                tokio::time::sleep(timing.poll).await;
                match port.revision(&record).await {
                    Ok(current) if current == revision => {
                        status.poll();
                    }
                    _ => return,
                }
            }
        };
        tokio::pin!(authority);
        let outcome = tokio::select! {
            result = &mut attempt => Some(result),
            _ = &mut authority => None,
            _ = shutdown.changed() => None,
        };
        let Some(outcome) = outcome else {
            let shutting_down = *shutdown.borrow() || shutdown.has_changed().is_err();
            if !shutting_down {
                status.failure(FailureCode::AuthorityRevoked);
            }
            let _ = stop.send(true);
            let _ = tokio::time::timeout(timing.stop, &mut attempt).await;
            if *shutdown.borrow() || shutdown.has_changed().is_err() {
                break;
            }
            status.finish(FailureCode::AuthorityRevoked, false, Duration::ZERO);
            failures = 0;
            continue;
        };
        let terminal = match &outcome.result {
            Ok(end) => {
                !end.retryable()
                    && !matches!(end.reason, EndReason::Stopped | EndReason::AuthorityRevoked)
            }
            Err(
                GatewayError::InvalidConfiguration
                | GatewayError::MissingMessageContentIntent
                | GatewayError::Protocol,
            ) => true,
            _ => false,
        };
        failures = failures.saturating_add(1);
        let delay = retry_delay(&outcome, failures, timing.retry);
        let code = match &outcome.result {
            Ok(end) => end.reason.into(),
            Err(error) => (*error).into(),
        };
        status.finish(code, terminal, delay);
        if terminal {
            let snapshot = statuses.view(&record, Some(&revision));
            terminal_revision = Some((
                revision.clone(),
                snapshot.stage,
                snapshot.last_failure_code.unwrap_or(code),
            ));
        }
        if stopped(&mut shutdown, delay).await {
            break;
        }
    }
    status.stopped();
}
fn retry_delay(attempt: &Attempt, failures: u32, base: Duration) -> Duration {
    let minimum = match &attempt.result {
        Ok(end) => end.retry_after,
        // Lease contention is not permanent revocation. Do not hammer discovery
        // while another process holds the thirty-second lease.
        Err(GatewayError::Authority) => Duration::from_secs(30),
        Err(GatewayError::RateLimited) => Duration::from_secs(60),
        _ => Duration::ZERO,
    };
    base.saturating_mul(1u32 << failures.saturating_sub(1).min(4))
        .min(Duration::from_secs(60))
        .max(minimum)
        .max(attempt.cooldown)
}
pub(super) async fn stopped(stop: &mut watch::Receiver<bool>, delay: Duration) -> bool {
    if *stop.borrow() {
        return true;
    }
    tokio::select! {
        _ = tokio::time::sleep(delay) => false,
        changed = stop.changed() => changed.is_err() || *stop.borrow(),
    }
}

#[cfg(test)]
#[path = "source_worker_tests.rs"]
mod tests;
