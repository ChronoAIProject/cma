//! Fence admission, recheck durable users, then drain only unused pool capacity.
use super::*;
use layer3_adapter::agent_sandbox::{NativePoolControl, NativePoolDrain};

pub(crate) enum RetirementScan {
    Ready(NativePoolDrain),
    Blocked,
    QuarantinedSandbox,
}

impl RetirementScan {
    pub(crate) fn into_drain(self) -> Option<NativePoolDrain> {
        match self {
            Self::Ready(drain) => Some(drain),
            Self::Blocked | Self::QuarantinedSandbox => None,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum RetirementAttempt {
    Unacknowledged,
    Finished,
    QuarantineDeferred { revision: i64 },
}

enum RetirementOutcome {
    Removed,
    Drained,
    QuarantineDeferred,
}

impl NativeWarmPools {
    pub(super) async fn retire_claimed(
        &self,
        provider: &dyn NativePoolControl,
        target: PoolTarget,
    ) -> Result<RetirementAttempt, WarmPoolError> {
        let released = self.released_images();
        let image = if target.machine {
            // Removing the configured machine image does not grant an agent-only
            // release authority to retire desktop capacity.
            let Some((image, _)) = released.machine else {
                self.store.finish(&target, false).await?;
                return Ok(RetirementAttempt::Finished);
            };
            image.uri
        } else {
            released.agent.uri
        };
        retire_claimed(self.store.as_ref(), provider, &image, target).await
    }
}

pub(super) async fn retire_claimed(
    store: &dyn PoolTargetStore,
    provider: &dyn NativePoolControl,
    image: &str,
    mut target: PoolTarget,
) -> Result<RetirementAttempt, WarmPoolError> {
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        // A saved pool request can still need its first Claim. A permanent
        // marker would refuse that admitted operation before it can resume.
        match store.scan_retirement_drain(&target, image).await? {
            RetirementScan::Ready(_) => (),
            RetirementScan::Blocked => return Err(WarmPoolError::Unavailable),
            RetirementScan::QuarantinedSandbox => return Ok(RetirementOutcome::QuarantineDeferred),
        }
        target = store.begin_retirement(&target).await?;
        if target.binding.is_none() {
            // Recover an unacknowledged binding without changing provider replicas.
            let binding = provider
                .inventory()
                .await
                .map_err(|_| WarmPoolError::Unavailable)?
                .into_iter()
                .map(|status| status.binding)
                .find(|binding| {
                    binding.config_sha256 == target.plan.config_sha256
                        && binding.spec == target.plan.spec
                        && binding.pod_policy == target.plan.pod_policy
                })
                .ok_or(WarmPoolError::Unavailable)?;
            store.bind(&target, &binding).await?;
            target.binding = Some(binding);
        }
        let binding = target.binding.as_ref().ok_or(WarmPoolError::Conflict)?;
        provider
            .begin_retirement(binding)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?;
        let retirement_homes = store.retirement_homes(&target, image).await?;
        let Some(drain) = store
            .scan_retirement_drain(&target, image)
            .await?
            .into_drain()
        else {
            return Err(WarmPoolError::Unavailable);
        };
        // retire_pool handles absent definitions idempotently, and only
        // succeeds after both definitions and all spare graphs are absent.
        if let Some(homes) = retirement_homes {
            if provider
                .retire_pool(binding, &homes)
                .await
                .map_err(|_| WarmPoolError::Unavailable)?
            {
                store.remove_retired(&target).await?;
                return Ok(RetirementOutcome::Removed);
            }
        }
        // Settled retained homes can prevent definition deletion without
        // keeping spare capacity alive. Admission is fenced and all durable
        // allocation sources have been rechecked after the provider marker.
        // Both lanes use the UID-bound request. A provider that does not
        // implement guarded member deletion leaves the drain pending.
        provider
            .request_spare_drain(binding, &drain)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?;
        Ok(RetirementOutcome::Drained)
    })
    .await;
    match result {
        Ok(Ok(RetirementOutcome::Removed)) => (),
        Ok(Ok(RetirementOutcome::Drained)) => store.finish(&target, true).await?,
        Ok(Ok(RetirementOutcome::QuarantineDeferred)) => {
            store.defer_quarantined_retirement(&target).await?;
            return Ok(RetirementAttempt::QuarantineDeferred {
                revision: target.revision,
            });
        }
        _ => store.finish(&target, false).await?,
    }
    Ok(RetirementAttempt::Finished)
}
