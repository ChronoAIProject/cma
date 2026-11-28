//! A candidate may dispatch only within its durable, revision-bound authorization.
use super::*;
use layer3_adapter::agent_sandbox::NativePoolControl;

impl NativeWarmPools {
    pub(super) async fn reconcile_candidate(
        &self,
        provider: &dyn NativePoolControl,
        target: PoolTarget,
    ) -> Result<bool, WarmPoolError> {
        if target.binding.is_some()
            && target.applied_revision == target.revision
            && target.error.is_none()
        {
            self.store.finish(&target, true).await?;
            return Ok(false);
        }
        let deadline = candidate_deadline(&target);
        if !self.store.begin_candidate_effect(&target).await? {
            if self.store.candidate_pending(&target).await? {
                return self.settle_candidate(provider, target).await;
            }
            self.store.finish(&target, false).await?;
            return Ok(false);
        }
        if let Some(result) = dispatch_before(
            deadline,
            reconcile_claimed(self.store.as_ref(), provider, target.clone(), true),
        )
        .await
        {
            return result.map(|_| true);
        }
        self.store.finish(&target, false).await?;
        Ok(false)
    }

    async fn settle_candidate(
        &self,
        provider: &dyn NativePoolControl,
        mut target: PoolTarget,
    ) -> Result<bool, WarmPoolError> {
        let deadline = candidate_deadline(&target);
        let settle = async {
            if target.binding.is_none() {
                // An uncertain initial configure has only zero-capacity
                // authority. Recover its exact binding before fencing resize.
                self.store.authorize_provider_create(&target).await?;
                let observed = provider
                    .configure_planned(&target.plan, 0)
                    .await
                    .map_err(|_| WarmPoolError::Unavailable)?;
                self.store.bind(&target, &observed.binding).await?;
                target.binding = Some(observed.binding);
            }
            let binding = target.binding.as_ref().ok_or(WarmPoolError::Conflict)?;
            provider
                .begin_retirement(binding)
                .await
                .map_err(|_| WarmPoolError::Unavailable)?;
            self.store.confirm_candidate_barrier(&target).await?;
            Ok(true)
        };
        dispatch_before(deadline, settle)
            .await
            .ok_or(WarmPoolError::Unavailable)?
    }
}

fn candidate_deadline(target: &PoolTarget) -> tokio::time::Instant {
    let lease_ms = target
        .lease_until
        .timestamp_millis()
        .saturating_sub(mongodb::bson::DateTime::now().timestamp_millis())
        .max(0) as u64;
    tokio::time::Instant::now() + Duration::from_millis(lease_ms.min(30_000))
}

async fn dispatch_before<T>(
    deadline: tokio::time::Instant,
    effect: impl std::future::Future<Output = T>,
) -> Option<T> {
    tokio::select! {
        biased;
        _ = tokio::time::sleep_until(deadline) => None,
        result = effect => Some(result),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn delayed_candidate_authorization_cannot_dispatch_after_the_original_deadline() {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        tokio::time::advance(Duration::from_secs(31)).await;
        let mut dispatched = false;
        let result = dispatch_before(deadline, async { dispatched = true }).await;
        assert!(result.is_none());
        assert!(!dispatched);
    }
}
