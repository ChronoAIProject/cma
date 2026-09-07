use std::future::Future;
use std::time::{Duration, SystemTime};

use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::Sandbox;
use tokio::time::{Instant, MissedTickBehavior};

use super::{model_error, SandboxCheckpointMaintenance, CLAIM_DURATION};

const RENEW_INTERVAL: Duration = Duration::from_secs(60);
pub(super) const CAPTURE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
pub(super) const FINALIZE_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) struct CaptureClock {
    started_at: SystemTime,
    started: Instant,
}

impl CaptureClock {
    pub(super) fn new(started_at: SystemTime) -> Self {
        Self {
            started_at,
            started: Instant::now(),
        }
    }

    pub(super) fn now(&self) -> SystemTime {
        self.started_at + self.started.elapsed()
    }
}

impl SandboxCheckpointMaintenance {
    pub(super) async fn keep_capture_claim<T>(
        &self,
        claimed: &mut Sandbox,
        clock: &CaptureClock,
        deadline: Instant,
        work: impl Future<Output = Result<T, SandboxStoreError>>,
    ) -> Result<T, SandboxStoreError> {
        tokio::pin!(work);
        let mut renewal = tokio::time::interval_at(Instant::now() + RENEW_INTERVAL, RENEW_INTERVAL);
        renewal.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                _ = tokio::time::sleep_until(deadline) => return Err(deadline_error()),
                _ = renewal.tick() => {
                    let now = clock.now();
                    let lease = claimed.lifecycle.checkpoint_capture.as_ref()
                        .and_then(|capture| capture.work.lease.as_ref())
                        .ok_or(SandboxStoreError::Conflict)?;
                    let mut next = claimed.clone();
                    next.renew_checkpoint_capture(&lease.holder, lease.fence, rfc3339_at(now), rfc3339_at(now + CLAIM_DURATION))
                        .map_err(model_error)?;
                    tokio::time::timeout_at(deadline.min(Instant::now() + FINALIZE_TIMEOUT), self.commit(claimed, next.clone(), rfc3339_at(now)))
                        .await.map_err(|_| deadline_error())??;
                    *claimed = next;
                }
                result = &mut work => return result,
            }
        }
    }
}

pub(super) fn same_capture_claim(original: &Sandbox, current: &Sandbox) -> bool {
    if current.revision < original.revision || current.updated_at < original.updated_at {
        return false;
    }
    let mut expected = original.clone();
    expected.revision = current.revision;
    expected.updated_at = current.updated_at.clone();
    let Some(old) = expected
        .lifecycle
        .checkpoint_capture
        .as_mut()
        .and_then(|capture| capture.work.lease.as_mut())
    else {
        return false;
    };
    let Some(new) = current
        .lifecycle
        .checkpoint_capture
        .as_ref()
        .and_then(|capture| capture.work.lease.as_ref())
    else {
        return false;
    };
    if new.expires_at < old.expires_at {
        return false;
    }
    old.expires_at = new.expires_at.clone();
    expected == *current
}

pub(super) fn deadline_error() -> SandboxStoreError {
    SandboxStoreError::Unavailable("periodic checkpoint operation deadline elapsed".into())
}
