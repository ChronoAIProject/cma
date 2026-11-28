use super::*;

async fn confirm_spare_shortage(
    store: &dyn PoolTargetStore,
    provider: &dyn NativePoolControl,
    plan: &NativePoolConfiguration,
    target_fencing: bool,
) -> Result<WarmPoolStartup, WarmPoolError> {
    let mut targets = store.list().await?.into_iter().filter(|target| {
        !target.machine && !target.retiring && !target.retired && &target.plan == plan
    });
    let target = targets.next().ok_or(WarmPoolError::Unavailable)?;
    let binding = target.binding.as_ref().ok_or(WarmPoolError::Unavailable)?;
    if targets.next().is_some()
        || target.target_replicas == 0
        || target.lease.is_some()
        || binding.spec != plan.spec
        || binding.pod_policy != plan.pod_policy
        || binding.config_sha256 != plan.config_sha256
    {
        return Err(WarmPoolError::Unavailable);
    }
    let inventory = provider
        .inventory()
        .await
        .map_err(|_| WarmPoolError::Unavailable)?;
    if observed_capacity(&target, &inventory) != Some(0)
        || inventory
            .iter()
            .filter(|status| {
                status.binding.config_sha256 == plan.config_sha256
                    || status.binding.name == binding.name
            })
            .count()
            != 1
    {
        return Err(WarmPoolError::Unavailable);
    }
    let confirmed = provider
        .observe_admitting_pool(binding, &provider_target(&target), target_fencing)
        .await
        .map_err(|error| match error {
            layer3_adapter::AdapterError::Rejected(_) => WarmPoolError::Conflict,
            _ => WarmPoolError::Unavailable,
        })?;
    if observed_capacity(&target, &[confirmed]) != Some(0) {
        return Err(WarmPoolError::Unavailable);
    }
    let current = store
        .find(&target.id)
        .await?
        .ok_or(WarmPoolError::Unavailable)?;
    if current.revision != target.revision
        || current.plan != target.plan
        || current.binding != target.binding
        || current.target_replicas != target.target_replicas
        || current.applied_revision != current.revision
        || current.error.is_some()
        || current.lease.is_some()
        || current.machine
        || current.retiring
        || current.retired
    {
        return Err(WarmPoolError::Unavailable);
    }
    Ok(WarmPoolStartup::CapacityDeferred(ConfirmedSpareShortage(
        Box::new(current),
    )))
}

// A returned retirement may have published a durable failure. Require
// its recorded settlement before classifying startup as capacity-only.
async fn confirm_retirement_settlement(
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
        if !target.retired
            && (!eligible
                || target.lease.is_some()
                || target.error.is_some()
                || target.applied_revision != target.revision)
        {
            return Err(WarmPoolError::Unavailable);
        }
    }
    Ok(())
}

pub(super) async fn confirm_after_retirement(
    store: &dyn PoolTargetStore,
    provider: &dyn NativePoolControl,
    plan: &NativePoolConfiguration,
    target_fencing: bool,
    admission: &Admission,
    attempts: &BTreeMap<String, RetirementAttempt>,
) -> Result<WarmPoolStartup, WarmPoolError> {
    tokio::select! {
        biased;
        () = admission.stopping() => Ok(WarmPoolStartup::Stopping),
        result = tokio::time::timeout(Duration::from_secs(8), async {
            confirm_retirement_settlement(store, attempts).await?;
            confirm_spare_shortage(store, provider, plan, target_fencing).await
        }) => result.map_err(|_| WarmPoolError::Unavailable)?,
    }
}

#[cfg(test)]
pub(in crate::warm_pools) async fn await_ready_with_fencing(
    store: &dyn PoolTargetStore,
    provider: &dyn NativePoolControl,
    plan: &NativePoolConfiguration,
    deadline: Instant,
    policy: StartupPoolPolicy,
    admission: Option<&Admission>,
    writer: Option<StartupWriter<'_>>,
) -> Result<WarmPoolStartup, WarmPoolError> {
    match await_ready_steps(store, provider, plan, deadline, policy, admission, writer).await? {
        Some(outcome) => Ok(outcome),
        None => {
            confirm_after_retirement(
                store,
                provider,
                plan,
                policy.target_fencing,
                admission.ok_or(WarmPoolError::Unavailable)?,
                &BTreeMap::new(),
            )
            .await
        }
    }
}
