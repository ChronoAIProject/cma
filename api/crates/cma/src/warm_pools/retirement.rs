//! Fence admission, recheck durable users, then drain only unused pool capacity.
use super::*;
use layer3_adapter::agent_sandbox::{NativePoolControl, NativePoolDrain};

pub(crate) enum RetirementScan {
    Ready(NativePoolDrain),
    Blocked(PoolFailure),
    QuarantinedSandbox,
}

impl RetirementScan {
    pub(crate) fn into_drain(self) -> Option<NativePoolDrain> {
        match self {
            Self::Ready(drain) => Some(drain),
            Self::Blocked(_) | Self::QuarantinedSandbox => None,
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
                self.store
                    .finish_with_failure(&target, PoolFailure::retirement("no_selected_release", "The shared desktop lane has no selected release. Restore a verified desktop release before retiring its older pools."))
                    .await?;
                return Ok(RetirementAttempt::Finished);
            };
            image.uri
        } else {
            released.agent.uri
        };
        retire_claimed(
            self.store.as_ref(),
            provider,
            &image,
            target,
            self.pool_create_cutover_epoch,
        )
        .await
    }
}

pub(super) async fn retire_claimed(
    store: &dyn PoolTargetStore,
    provider: &dyn NativePoolControl,
    image: &str,
    mut target: PoolTarget,
    create_cutover_epoch: Option<uuid::Uuid>,
) -> Result<RetirementAttempt, WarmPoolError> {
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        // A saved pool request can still need its first Claim. A permanent
        // marker would refuse that admitted operation before it can resume.
        match store
            .scan_retirement_drain(&target, image)
            .await
            .map_err(|_| RETIREMENT_SCAN_FAILED)?
        {
            RetirementScan::Ready(_) => (),
            RetirementScan::Blocked(message) => return Err(message),
            RetirementScan::QuarantinedSandbox => return Ok(RetirementOutcome::QuarantineDeferred),
        }
        target = store.begin_retirement(&target).await.map_err(|_| {
            PoolFailure::retirement("retirement_barrier", "The durable retirement marker could not be written. Check the pool record and database availability.")
        })?;
        if target.binding.is_none() {
            // A missing inventory entry is not proof of absence. Look up the
            // exact definitions and graph after fencing new durable admissions.
            let mut fenced_epoch = None;
            if let Some(epoch) = create_cutover_epoch {
                if store.pool_creates_fenced(epoch).await.map_err(|_| RETIREMENT_SCAN_FAILED)? {
                    fenced_epoch = Some(epoch);
                }
            }
            let binding = provider.recover_retirement_binding(&target.plan).await.map_err(|_| {
                PoolFailure::retirement("unverified_binding", "The exact pool binding or its absence could not be verified. Check the original definitions and controller inventory.")
            })?;
            let Some(binding) = binding else {
                let Some(epoch) = fenced_epoch else {
                    return Err(PoolFailure::retirement("unsettled_create", "An earlier provider create cannot be ruled out. Verify predecessor writers and their dispatched operations before enabling legacy retirement."));
                };
                match store.scan_retirement_drain(&target, image).await.map_err(|_| RETIREMENT_SCAN_FAILED)? {
                    RetirementScan::Ready(drain) if drain.homes.is_empty() && drain.claims.is_empty() => (),
                    _ => return Err(PoolFailure::retirement("retained_allocation", "A retained allocation prevents settling the unbound pool. Check its original allocation records.")),
                }
                return confirm_retirement(
                    store, &target, store.remove_unbound_retired(&target, epoch).await,
                ).await;
            };
            store.bind(&target, &binding).await.map_err(|_| {
                PoolFailure::retirement("binding_write", "The pool binding could not be saved. Check the pool record and database availability.")
            })?;
            target.binding = Some(binding);
        }
        let binding = target.binding.as_ref().ok_or(PoolFailure::retirement("missing_binding", "The pool binding is missing. Check the recorded target and its pool definitions."))?;
        provider.begin_retirement(binding).await.map_err(|_| {
            PoolFailure::retirement("provider_barrier", "The provider did not confirm the retirement barrier. Check the original pool and template identities and controller health.")
        })?;
        let retirement_homes = store
            .retirement_homes(&target, image)
            .await
            .map_err(|_| RETIREMENT_SCAN_FAILED)?;
        let drain = match store
            .scan_retirement_drain(&target, image)
            .await
            .map_err(|_| RETIREMENT_SCAN_FAILED)?
        {
            RetirementScan::Ready(drain) => drain,
            RetirementScan::Blocked(message) => return Err(message),
            RetirementScan::QuarantinedSandbox => return Err(
                PoolFailure::retirement("quarantined_sandbox", "A quarantined Sandbox prevents proving retirement ownership. Repair its retained record before retirement can resume."),
            ),
        };
        // retire_pool handles absent definitions idempotently, and only
        // succeeds after both definitions and all spare graphs are absent.
        if let Some(homes) = retirement_homes {
            if provider
                .retire_pool(binding, &homes)
                .await
                .map_err(|_| PoolFailure::retirement("definition_removal", "The provider could not verify or remove the retired definitions. Check the original pool, template, and retained Claims."))?
            {
                return confirm_retirement(store, &target, store.remove_retired(&target).await).await;
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
            .map_err(|_| PoolFailure::retirement("spare_drain", "The provider could not drain the spare members. Check the controller and the original member and Claim identities."))?;
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
        Ok(Err(message)) => store.finish_with_failure(&target, message).await?,
        Err(_) => store
            .finish_with_failure(
                &target,
                PoolFailure::retirement("timeout", "Native pool retirement timed out. Check database and controller latency while reconciliation retries."),
            )
            .await?,
    }
    Ok(RetirementAttempt::Finished)
}

async fn confirm_retirement(
    store: &dyn PoolTargetStore,
    target: &PoolTarget,
    result: Result<(), WarmPoolError>,
) -> Result<RetirementOutcome, PoolFailure> {
    if result.is_err() {
        let confirmed = store
            .find(&target.id)
            .await
            .ok()
            .flatten()
            .is_some_and(|row| {
                row.retired
                    && row.revision == target.revision
                    && row.binding == target.binding
                    && row.plan == target.plan
            });
        if !confirmed {
            return Err(PoolFailure::retirement("tombstone_write", "The durable retirement result could not be recorded. Check database availability and the pool record."));
        }
    }
    Ok(RetirementOutcome::Removed)
}

const RETIREMENT_SCAN_FAILED: PoolFailure =
    PoolFailure::retirement("unverified_ownership", "The pool's retained ownership could not be verified. Check allocation records and database availability before retrying.");
