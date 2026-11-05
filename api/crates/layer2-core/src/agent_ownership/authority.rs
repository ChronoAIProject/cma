use std::future::Future;
use std::sync::Arc;

use tokio::sync::watch;
use tokio::time::Instant;

use super::{OwnershipError, OwnershipToken};

tokio::task_local! {
    static EXECUTION_OWNER: OwnershipToken;
}

/// The token captured at the start of this execution, never a lookup of whatever
/// epoch happens to be current when a late provider result finally returns.
pub fn execution_owner() -> Option<OwnershipToken> {
    EXECUTION_OWNER.try_with(Clone::clone).ok()
}

/// Monotonic local cancellation complements the durable and receiving-runtime
/// fences. A revoked/expired authority can never be revived by a late renewal.
pub struct Authority {
    pub token: OwnershipToken,
    deadline: watch::Sender<Option<Instant>>,
}

impl Authority {
    pub fn new(token: OwnershipToken, deadline: Instant) -> Arc<Self> {
        let (deadline, _) = watch::channel(Some(deadline));
        Arc::new(Self { token, deadline })
    }

    pub fn is_current(&self) -> bool {
        self.deadline
            .borrow()
            .is_some_and(|deadline| Instant::now() < deadline)
    }

    pub fn renew_until(&self, next: Instant) -> Result<(), OwnershipError> {
        let mut renewed = false;
        self.deadline.send_modify(|deadline| {
            if deadline.is_some_and(|current| Instant::now() < current) && Instant::now() < next {
                *deadline = Some(next);
                renewed = true;
            } else {
                *deadline = None;
            }
        });
        if renewed {
            Ok(())
        } else {
            Err(OwnershipError::Stale)
        }
    }

    pub fn revoke(&self) {
        self.deadline.send_replace(None);
    }

    pub async fn revoked(&self) {
        let mut deadline = self.deadline.subscribe();
        loop {
            let Some(until) = *deadline.borrow_and_update() else {
                return;
            };
            if until <= Instant::now() {
                self.revoke();
                return;
            }
            tokio::select! {
                changed = deadline.changed() => if changed.is_err() { return },
                _ = tokio::time::sleep_until(until) => (),
            }
        }
    }

    /// Spawned tasks must explicitly capture this authority and enter their own
    /// scope; Tokio task locals intentionally do not transfer implicitly.
    pub async fn run<F: Future>(&self, operation: F) -> Result<F::Output, OwnershipError> {
        tokio::select! {
            biased;
            _ = self.revoked() => Err(OwnershipError::Stale),
            result = EXECUTION_OWNER.scope(self.token.clone(), operation) => {
                if self.is_current() { Ok(result) } else { Err(OwnershipError::Stale) }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentId;
    use std::time::Duration;

    fn authority() -> Arc<Authority> {
        Authority::new(
            OwnershipToken {
                agent_id: AgentId("agent".into()),
                instance_id: "owner".into(),
                epoch: 1,
                protocol: super::super::PROTOCOL_VERSION,
            },
            Instant::now() + Duration::from_secs(30),
        )
    }

    #[tokio::test(start_paused = true)]
    async fn expiry_cancels_pending_provider_result_and_late_renewal_cannot_revive_it() {
        let authority = authority();
        let (result, receive) = tokio::sync::oneshot::channel::<()>();
        let pending = tokio::spawn({
            let authority = authority.clone();
            async move { authority.run(receive).await }
        });
        tokio::time::advance(Duration::from_secs(30)).await;
        assert!(matches!(pending.await.unwrap(), Err(OwnershipError::Stale)));
        assert!(result.send(()).is_err());
        assert_eq!(
            authority.renew_until(Instant::now() + Duration::from_secs(30)),
            Err(OwnershipError::Stale)
        );
        assert!(!authority.is_current());
    }

    #[tokio::test(start_paused = true)]
    async fn scoped_token_is_exact_and_revocation_cancels_all_waiters() {
        let authority = authority();
        assert!(execution_owner().is_none());
        let token = authority
            .run(async { execution_owner().unwrap() })
            .await
            .unwrap();
        assert_eq!(token, authority.token);
        assert!(execution_owner().is_none());
        let mut waiters = tokio::task::JoinSet::new();
        for _ in 0..3 {
            let authority = authority.clone();
            waiters.spawn(async move { authority.run(std::future::pending::<()>()).await });
        }
        authority.revoke();
        while let Some(result) = waiters.join_next().await {
            assert_eq!(result.unwrap(), Err(OwnershipError::Stale));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn successful_renewal_keeps_waiters_until_new_deadline() {
        let authority = authority();
        let pending = tokio::spawn({
            let authority = authority.clone();
            async move {
                authority.revoked().await;
            }
        });
        tokio::time::advance(Duration::from_secs(20)).await;
        authority
            .renew_until(Instant::now() + Duration::from_secs(30))
            .unwrap();
        tokio::time::advance(Duration::from_secs(10)).await;
        assert!(authority.is_current());
        assert!(!pending.is_finished());
        tokio::time::advance(Duration::from_secs(20)).await;
        pending.await.unwrap();
        assert!(!authority.is_current());
    }
}
