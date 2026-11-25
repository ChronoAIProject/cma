use super::{bounded, OwnershipError, OwnershipLease, OwnershipStore, OwnershipToken};
use crate::AgentId;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{task::JoinHandle, time::Instant};

#[derive(Clone)]
pub(super) enum WriteOutcome {
    Acquired {
        started: Instant,
        now_ms: i64,
        lease: Option<OwnershipLease>,
    },
    ReconciledAcquisition(Option<OwnershipToken>),
    Renewed(OwnershipLease),
    RenewalRefused(OwnershipError),
    NotIssued(OwnershipError),
    Uncertain(OwnershipError),
}

/// The coordinator slot or exact authority owns the actual store task and its
/// result. A timed-out or cancelled waiter cannot drop either one.
pub(super) struct LeaseWrite {
    task: tokio::sync::Mutex<Option<JoinHandle<WriteOutcome>>>,
    result: Mutex<Option<WriteOutcome>>,
}

pub(super) const OPERATION_LIMIT: Duration = Duration::from_secs(23);

impl LeaseWrite {
    pub(super) fn acquisition(
        store: Arc<dyn OwnershipStore>,
        agent: AgentId,
        instance: String,
    ) -> Arc<Self> {
        let started = Instant::now();
        Self::spawn(async move {
            if Instant::now() >= started + OPERATION_LIMIT {
                return WriteOutcome::NotIssued(unresolved());
            }
            let membership = match bounded(store.membership()).await {
                Ok(membership) => membership,
                Err(error) => return WriteOutcome::NotIssued(error),
            };
            if let Err(error) = membership.require_compatible() {
                return WriteOutcome::NotIssued(error);
            }
            if Instant::now() >= started + OPERATION_LIMIT {
                return WriteOutcome::NotIssued(unresolved());
            }
            match tokio::time::timeout_at(
                started + OPERATION_LIMIT,
                store.acquire(&agent, &instance),
            )
            .await
            {
                Ok(Ok(lease)) => WriteOutcome::Acquired {
                    started,
                    now_ms: membership.now_ms,
                    lease,
                },
                Ok(Err(error @ OwnershipError::Unavailable(_))) => WriteOutcome::Uncertain(error),
                Ok(Err(error)) => WriteOutcome::NotIssued(error),
                Err(_) => WriteOutcome::Uncertain(unresolved()),
            }
        })
    }

    pub(super) fn renewal(store: Arc<dyn OwnershipStore>, token: OwnershipToken) -> Arc<Self> {
        Self::renewal_with(async move { store.renew(&token).await })
    }

    pub(super) fn renewal_with(
        write: impl std::future::Future<Output = Result<OwnershipLease, OwnershipError>>
            + Send
            + 'static,
    ) -> Arc<Self> {
        let until = Instant::now() + OPERATION_LIMIT;
        Self::spawn(async move {
            if Instant::now() >= until {
                return WriteOutcome::NotIssued(unresolved());
            }
            match tokio::time::timeout_at(until, write).await {
                Ok(Ok(lease)) => WriteOutcome::Renewed(lease),
                Ok(Err(error @ OwnershipError::Unavailable(_))) => WriteOutcome::Uncertain(error),
                Ok(Err(error)) => WriteOutcome::RenewalRefused(error),
                Err(_) => WriteOutcome::Uncertain(unresolved()),
            }
        })
    }

    fn spawn(
        operation: impl std::future::Future<Output = WriteOutcome> + Send + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            task: tokio::sync::Mutex::new(Some(tokio::spawn(operation))),
            result: Mutex::new(None),
        })
    }

    pub(super) fn completed(&self) -> Option<WriteOutcome> {
        self.result.lock().unwrap().clone()
    }

    pub(super) async fn join_until(&self, until: Instant) -> Result<WriteOutcome, OwnershipError> {
        if let Some(result) = self.completed() {
            return Ok(result);
        }
        if Instant::now() >= until {
            return Err(unresolved());
        }
        let mut task = tokio::time::timeout_at(until, self.task.lock())
            .await
            .map_err(|_| unresolved())?;
        if let Some(result) = self.completed() {
            return Ok(result);
        }
        let handle = task.as_mut().ok_or_else(unresolved)?;
        let result = tokio::time::timeout_at(until, handle)
            .await
            .map_err(|_| unresolved())?;
        let result = result.unwrap_or_else(|_| {
            WriteOutcome::Uncertain(OwnershipError::Invalid(
                "owned lease operation terminated".into(),
            ))
        });
        *self.result.lock().unwrap() = Some(result.clone());
        task.take();
        Ok(result)
    }

    pub(super) async fn abort_until(&self, until: Instant) -> Result<(), OwnershipError> {
        let mut task = tokio::time::timeout_at(until, self.task.lock())
            .await
            .map_err(|_| unresolved())?;
        if let Some(handle) = task.as_mut() {
            handle.abort();
            let result = tokio::time::timeout_at(until, handle)
                .await
                .map_err(|_| unresolved())?;
            let result = result.unwrap_or_else(|_| WriteOutcome::Uncertain(unresolved()));
            *self.result.lock().unwrap() = Some(result);
            task.take();
        }
        Ok(())
    }
}

impl Drop for LeaseWrite {
    fn drop(&mut self) {
        if let Some(task) = self.task.get_mut().as_ref() {
            task.abort();
        }
    }
}

pub(super) fn unresolved() -> OwnershipError {
    OwnershipError::Unavailable("admitted lease write remains unresolved".into())
}
