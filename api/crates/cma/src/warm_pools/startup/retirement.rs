use super::*;

impl NativeWarmPools {
    async fn startup_retirement_candidate(
        &self,
        target: &PoolTarget,
    ) -> Result<bool, WarmPoolError> {
        if self.default_holds.lane(target.machine) || target.retired {
            return Ok(false);
        }
        Ok(target.retiring
            || (!self.current_plan(target)? && !self.store.candidate_protected(target).await?))
    }

    pub(super) async fn confirm_startup_retirement(
        &self,
        attempts: &BTreeMap<String, RetirementAttempt>,
    ) -> Result<(), WarmPoolError> {
        // The local attempt map tracks our own writes, including lost acknowledgements.
        confirm_local_attempts(self.store.as_ref(), attempts).await?;
        // Another Manager may own a lease or have scheduled a retry. Re-read
        // superseded targets even if this startup never acquired their leases.
        for target in self.store.list().await? {
            if self.startup_retirement_candidate(&target).await? && !durably_settled(&target) {
                return Err(WarmPoolError::Unavailable);
            }
        }
        Ok(())
    }

    pub(super) async fn retire_during_startup(
        &self,
        stopping: &impl Fn() -> bool,
        writer: Option<StartupWriter<'_>>,
        unsettled: &mut BTreeMap<String, RetirementAttempt>,
    ) {
        if self
            .retire_superseded(stopping, writer, unsettled)
            .await
            .is_err()
        {
            tracing::warn!("superseded native pool retirement during startup will retry");
        }
    }

    /// Claims only retiring or superseded targets, never a current plan: the
    /// generic reconciler's unbound path would configure a freshly inserted
    /// default at zero replicas and empty a pool that release preparation
    /// prewarmed, which `step` deliberately adopts instead.
    async fn retire_superseded(
        &self,
        stopping: &impl Fn() -> bool,
        writer: Option<StartupWriter<'_>>,
        unsettled: &mut BTreeMap<String, RetirementAttempt>,
    ) -> Result<(), WarmPoolError> {
        let Some(provider) = self.adapter.native_pools() else {
            return Ok(());
        };
        for target in self.store.list().await? {
            if stopping() {
                break;
            }
            if !self.startup_retirement_candidate(&target).await? {
                continue;
            }
            // Plans are immutable and retirement only advances, so a claimed
            // superseded target is still superseded. A target an earlier pod
            // marked retiring and deferred is resumed, not skipped.
            boot_effect(writer, async {
                if !stopping() {
                    let previous =
                        unsettled.insert(target.id.clone(), RetirementAttempt::Unacknowledged);
                    if let Some(claimed) = self.store.claim_retirement(&target.id).await? {
                        let attempt = self.retire_claimed(provider, claimed).await?;
                        unsettled.insert(target.id.clone(), attempt);
                    } else if let Some(attempt) = previous {
                        unsettled.insert(target.id.clone(), attempt);
                    } else {
                        unsettled.remove(&target.id);
                    }
                }
                Ok(())
            })
            .await?;
        }
        Ok(())
    }
}

fn durably_settled(target: &PoolTarget) -> bool {
    target.lease.is_none() && target.error.is_none() && target.applied_revision == target.revision
}

// A returned retirement may have published a durable failure. Require
// its recorded settlement before classifying startup as capacity-only.
async fn confirm_local_attempts(
    store: &dyn PoolTargetStore,
    attempts: &BTreeMap<String, RetirementAttempt>,
) -> Result<(), WarmPoolError> {
    for (id, attempt) in attempts {
        if matches!(attempt, RetirementAttempt::Unacknowledged) {
            return Err(WarmPoolError::Unavailable);
        }
        let target = store.find(id).await?.ok_or(WarmPoolError::Unavailable)?;
        let eligible = match attempt {
            RetirementAttempt::QuarantineDeferred { revision } => {
                // This attempt stopped before either retirement marker. A
                // prior unresolved effect must still fail the shortage check.
                target.revision == *revision && !store.candidate_protected(&target).await?
            }
            _ => target.retiring,
        };
        if !target.retired && (!eligible || !durably_settled(&target)) {
            return Err(WarmPoolError::Unavailable);
        }
    }
    Ok(())
}
