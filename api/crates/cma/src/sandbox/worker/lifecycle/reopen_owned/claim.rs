use std::{future::Future, time::Duration};

use layer2_core::{
    sandbox::{
        store::{SandboxMutationGuard, SandboxStoreError, WORK_RENEW_SECONDS},
        Sandbox,
    },
    seconds_from_rfc3339,
};
use layer3_adapter::{AdapterError, Result};
use tokio::time::{sleep_until, timeout_at, Instant};

use super::{refused, SandboxWorker};

const READ_INTERVAL: Duration = Duration::from_secs(WORK_RENEW_SECONDS);

/// The caller renews its Sandbox claim. The owner may follow a renewal only
/// while its last observed authority is live and the same claim still matches.
pub(super) struct Claim<'a> {
    worker: &'a SandboxWorker,
    guard: &'a SandboxMutationGuard,
    expires: Instant,
}

impl<'a> Claim<'a> {
    pub(super) fn new(
        worker: &'a SandboxWorker,
        guard: &'a SandboxMutationGuard,
        sandbox: &Sandbox,
    ) -> Result<Self> {
        let expires = expiry(sandbox).ok_or_else(refused)?;
        Ok(Self::from_observation(worker, guard, expires))
    }

    pub(super) fn from_observation(
        worker: &'a SandboxWorker,
        guard: &'a SandboxMutationGuard,
        expires: Instant,
    ) -> Self {
        Self {
            worker,
            guard,
            expires,
        }
    }

    pub(super) async fn follow<T>(
        &mut self,
        operation: impl Future<Output = Result<T>>,
    ) -> Result<T> {
        tokio::pin!(operation);
        loop {
            let expires = self.expires;
            tokio::select! {
                biased;
                _ = sleep_until(expires) => return Err(refused()),
                _ = sleep_until(self.next_read_at(READ_INTERVAL)) => {
                    let parent = self.read().await?;
                    self.observe(&parent)?;
                }
                result = &mut operation => {
                    let result = result?;
                    let parent = self.read().await?;
                    self.observe(&parent)?;
                    return Ok(result);
                }
            }
        }
    }

    pub(super) async fn read(&self) -> Result<Sandbox> {
        self.read_while(
            self.worker
                .dependencies
                .store
                .get_sandbox(&self.guard.sandbox_id),
        )
        .await?
        .ok_or_else(refused)
    }

    pub(super) async fn read_while<T, E>(
        &self,
        request: impl Future<Output = std::result::Result<T, E>>,
    ) -> Result<T> {
        if Instant::now() >= self.expires {
            return Err(refused());
        }
        let result = timeout_at(self.expires.min(Instant::now() + READ_INTERVAL), request)
            .await
            .map_err(|_| refused())?
            .map_err(|_| {
                AdapterError::Unavailable("lifecycle reopen claim could not be read".into())
            })?;
        // Tokio may return a ready store result before observing its timer.
        if Instant::now() >= self.expires {
            return Err(refused());
        }
        Ok(result)
    }

    /// Once the owner returns a receipt, a transient store read cannot revoke
    /// its stage. Retry within the last proven expiry, with no extension inferred
    /// from an unavailable store.
    pub(super) async fn read_staged<T, F>(&self, mut request: impl FnMut() -> F) -> Result<T>
    where
        F: Future<Output = std::result::Result<T, SandboxStoreError>>,
    {
        while Instant::now() < self.expires {
            let read =
                timeout_at(self.expires.min(Instant::now() + READ_INTERVAL), request()).await;
            if Instant::now() >= self.expires {
                break;
            }
            match read {
                Ok(Ok(value)) => return Ok(value),
                Ok(Err(SandboxStoreError::Unavailable(_))) | Err(_) => {
                    tracing::debug!(sandbox_id = %self.guard.sandbox_id, "retrying staged reopen store read");
                    sleep_until(
                        self.expires
                            .min(Instant::now() + Duration::from_millis(100)),
                    )
                    .await;
                }
                Ok(Err(_)) => return Err(refused()),
            }
        }
        Err(refused())
    }

    pub(super) async fn read_staged_parent(&self) -> Result<Sandbox> {
        self.read_staged(|| {
            self.worker
                .dependencies
                .store
                .get_sandbox(&self.guard.sandbox_id)
        })
        .await?
        .ok_or_else(refused)
    }

    pub(super) fn observe(&mut self, parent: &Sandbox) -> Result<()> {
        if Instant::now() >= self.expires || !self.guard.matches(parent) {
            return Err(refused());
        }
        self.expires = expiry(parent).ok_or_else(refused)?;
        Ok(())
    }

    pub(super) fn expires(&self) -> Instant {
        self.expires
    }

    pub(super) fn next_read_at(&self, interval: Duration) -> Instant {
        let now = Instant::now();
        let remaining = self.expires.saturating_duration_since(now);
        // Leave time to observe an in-place renewal before the last proven
        // lease expires. Bound the polling rate as that deadline approaches.
        now + interval.min((remaining / 2).max(Duration::from_millis(100)))
    }
}

fn expiry(parent: &Sandbox) -> Option<Instant> {
    let expires = seconds_from_rfc3339(&parent.work.lease.as_ref()?.expires_at)?;
    let remaining = (std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(expires))
        .duration_since(std::time::SystemTime::now())
        .ok()
        .filter(|remaining| !remaining.is_zero())?;
    Instant::now().checked_add(remaining)
}
