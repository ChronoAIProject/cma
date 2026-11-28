use super::*;
use layer3_adapter::{
    agent_sandbox::{NativePoolControl, NativePoolTarget},
    AdapterError,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PoolApply {
    Settled,
    ConfigurationDrift,
    Failed,
}

pub(super) async fn reconcile_claimed(
    store: &dyn PoolTargetStore,
    provider: &dyn NativePoolControl,
    mut target: PoolTarget,
    target_fencing: bool,
) -> Result<PoolApply, WarmPoolError> {
    if target.retiring {
        return Err(WarmPoolError::Conflict);
    }
    let result = async {
        if target.binding.is_none() {
            // Keep capacity at zero until the original definitions have durable bindings.
            store.authorize_provider_create(&target).await.map_err(|_| (
                PoolApply::Failed,
                PoolFailure::apply("create_fence", "The provider creation lease could not be confirmed. Check database availability and retry."),
            ))?;
            let status = provider
                .configure_planned(&target.plan, 0)
                .await
                .map_err(|_| (PoolApply::Failed, PoolFailure::apply("definition_creation", "The provider could not create the pool definitions. Check controller availability and its pool and template permissions.")))?;
            store.bind(&target, &status.binding).await.map_err(|_| {
                (PoolApply::Failed, PoolFailure::apply("binding_write", "The pool binding could not be saved. Check database availability and the pool record."))
            })?;
            target.binding = Some(status.binding);
        }
        let binding = target.binding.as_ref().ok_or((
            PoolApply::Failed,
            PoolFailure::apply(
                "missing_binding",
                "The pool binding is missing. Check the recorded target and its pool definitions.",
            ),
        ))?;
        if target_fencing {
            provider
                .resize_fenced(
                    binding,
                    &NativePoolTarget {
                        target_id: target.id.clone(),
                        revision: target.revision,
                        replicas: target.target_replicas,
                    },
                )
                .await
        } else {
            provider.resize(binding, target.target_replicas).await
        }
        .map_err(resize_failure)?;
        Ok(())
    };
    let (outcome, message) = match tokio::time::timeout(Duration::from_secs(30), result).await {
        Ok(Ok(())) => {
            store.finish(&target, true).await?;
            telemetry::applied(&target, true);
            return Ok(PoolApply::Settled);
        }
        Ok(Err(failure)) => failure,
        Err(_) => (
            PoolApply::Failed,
            PoolFailure::apply("timeout", "Applying pool capacity timed out. Check controller and database latency while reconciliation retries."),
        ),
    };
    store.finish_with_failure(&target, message).await?;
    telemetry::applied(&target, false);
    Ok(outcome)
}

fn resize_failure(error: AdapterError) -> (PoolApply, PoolFailure) {
    match error {
        AdapterError::NativePoolConfigurationDrift => (
            PoolApply::ConfigurationDrift,
            PoolFailure::apply("configuration_drift", "The provider configuration differs from the recorded plan. Restore the original pool and template policy before retrying."),
        ),
        AdapterError::NativePoolDefinitionAbsent { .. } => (
            PoolApply::Failed,
            PoolFailure::apply("definition_absent", "An original pool definition is missing. Check the pool and template before making further capacity changes."),
        ),
        _ => (
            PoolApply::Failed,
            PoolFailure::apply("capacity_unconfirmed", "The provider did not confirm the requested capacity. Check controller health and the recorded pool binding."),
        ),
    }
}
