//! Parent retirement authority plus physical finality settles abandoned child work.
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use layer2_core::sandbox::store::{
    SandboxLifecycleExecutionStore, SandboxReconciliationStore, SandboxRuntimeLossOutcome,
    SandboxRuntimeLossRequest, SandboxStore, SandboxStoreError, WorkAuthority,
};
use layer2_core::sandbox::{Sandbox, SandboxInterruptionCause, SandboxStatus, SandboxWorkStep};
use layer2_core::{rfc3339_at, seconds_from_rfc3339};
use layer3_adapter::{AdapterError, SandboxAdapter};

trait RetirementStore:
    SandboxStore + SandboxReconciliationStore + SandboxLifecycleExecutionStore
{
}
impl<S: SandboxStore + SandboxReconciliationStore + SandboxLifecycleExecutionStore> RetirementStore
    for S
{
}

pub(crate) enum RetirementOutcome {
    GracefulPending,
    ProviderPending(&'static str),
    Settled,
    Superseded,
}

pub(crate) struct NativeRetirementFinalizer {
    store: Arc<dyn RetirementStore>,
    adapter: Arc<dyn SandboxAdapter>,
    deployment: String,
    clock: Arc<dyn Fn() -> SystemTime + Send + Sync>,
    failures: Arc<super::retirement_control::RetirementControlFailures>,
}

impl NativeRetirementFinalizer {
    pub(crate) fn new<S>(
        store: Arc<S>,
        adapter: Arc<dyn SandboxAdapter>,
        deployment: String,
        failures: Arc<super::retirement_control::RetirementControlFailures>,
    ) -> Self
    where
        S: SandboxStore + SandboxReconciliationStore + SandboxLifecycleExecutionStore + 'static,
    {
        Self {
            store,
            adapter,
            deployment,
            failures,
            clock: Arc::new(SystemTime::now),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_clock(mut self, clock: Arc<dyn Fn() -> SystemTime + Send + Sync>) -> Self {
        self.clock = clock;
        self
    }

    pub(crate) fn applies(sandbox: &Sandbox) -> bool {
        sandbox.status == SandboxStatus::Retiring
            && sandbox.work.step == SandboxWorkStep::StopAgents
            && sandbox
                .runtime
                .as_ref()
                .is_some_and(|b| b.runtime_id.starts_with("agent-sandbox:"))
    }

    pub(crate) async fn finalize(
        &self,
        original: &Sandbox,
        holder: &str,
    ) -> Result<RetirementOutcome, SandboxStoreError> {
        // Three ordinary retry opportunities let the asynchronous child workers
        // execute FenceInteraction, CloseProcess and PublishStopped. Retry count
        // permits an attempt only; it never proves physical absence.
        if original.work.attempts < 3 {
            return Ok(RetirementOutcome::GracefulPending);
        }
        if !Self::applies(original) || !self.current(original, holder).await? {
            return Ok(RetirementOutcome::Superseded);
        }
        let Some(provider) = self.adapter.runtime_retirement() else {
            return Ok(RetirementOutcome::ProviderPending(
                "runtime retirement finality is unsupported",
            ));
        };
        let Some(home) = original.lifecycle.home.as_ref() else {
            return Ok(RetirementOutcome::ProviderPending(
                "retirement home authority is missing",
            ));
        };
        if home.released || home.validate(&original.id).is_err() || home.attachment().is_none() {
            return Ok(RetirementOutcome::ProviderPending(
                "retirement home authority is invalid",
            ));
        }
        let request = match super::runtime_attachment::retained_runtime_request(
            original,
            original.runtime.as_ref().expect("bound retirement"),
            &self.deployment,
        ) {
            Ok(request) => request,
            Err(_) => {
                return Ok(RetirementOutcome::ProviderPending(
                    "retirement runtime identity is incomplete",
                ))
            }
        };
        let children =
            match super::live_agent_stopper::retirement_agents(self.store.as_ref(), original).await
            {
                Ok(children) => children,
                Err(_) => {
                    return Ok(RetirementOutcome::ProviderPending(
                        "retirement child authority is incomplete",
                    ))
                }
            };
        // Only effects confined to this runtime/home are discharged by deletion.
        // Unknown or future obligations retain their original evidence.
        if children
            .iter()
            .any(|child| !layer2_core::sandbox::store::retirement_debt_is_local(child))
        {
            return Ok(RetirementOutcome::ProviderPending(
                "retirement retains unresolved cleanup obligations",
            ));
        }
        let present =
            match tokio::time::timeout(Duration::from_secs(30), self.adapter.runtime_inventory())
                .await
            {
                Ok(Ok(inventory)) => {
                    if inventory.iter().any(|item| {
                        item.sandbox_id == original.id.as_str() && item.handle != request.handle
                    }) {
                        return Ok(RetirementOutcome::ProviderPending(
                            "retirement inventory contains a replacement",
                        ));
                    }
                    inventory.iter().any(|item| item.handle == request.handle)
                }
                _ => {
                    return Ok(RetirementOutcome::ProviderPending(
                        "retirement inventory is unavailable",
                    ))
                }
            };
        // A reachable replacement process is an identity conflict. Unreachability
        // only permits the provider path; it supplies no finality evidence.
        let mut failed_inspection = false;
        if present {
            let (inspection, failure) =
                layer3_adapter::receiver_failure::observe(tokio::time::timeout(
                    Duration::from_secs(5),
                    self.adapter.inspect_runtime(&request),
                ))
                .await;
            failed_inspection = failure.is_some();
            match inspection {
                Ok(Ok(Some(inspected))) => {
                    if inspected.observation.handle != request.handle
                        || inspected.observation.sandbox_id != original.id.as_str()
                        || inspected.observation.generation != request.generation
                        || inspected.status.as_ref().is_some_and(|status| {
                            original.runtime_process_matches(status) != Ok(true)
                        })
                    {
                        return Ok(RetirementOutcome::ProviderPending(
                            "retirement runtime identity changed",
                        ));
                    }
                }
                Ok(Ok(None) | Err(AdapterError::Unavailable(_))) | Err(_) => {}
                Ok(Err(_)) if failed_inspection => {}
                Ok(Err(_)) => {
                    return Ok(RetirementOutcome::ProviderPending(
                        "retirement runtime inspection was refused",
                    ))
                }
            }
        }
        // An already absent original can be proved after a crash/restart without
        // a receiver. A present runtime requires a fresh actual control failure.
        if present && !failed_inspection && !self.failures.failed_now(original).await {
            return Ok(RetirementOutcome::GracefulPending);
        }
        if !self.current(original, holder).await? {
            return Ok(RetirementOutcome::Superseded);
        }
        if present
            && !matches!(
                tokio::time::timeout(
                    Duration::from_secs(180),
                    provider.retire_unreachable(&request)
                )
                .await,
                Ok(Ok(()))
            )
        {
            return Ok(RetirementOutcome::ProviderPending(
                "native retirement deletion remains unconfirmed",
            ));
        }
        // This boundary precedes all final reads, after the deletion attempt.
        // It is never advanced to hide a concurrent parent/child publication.
        let observed_at = (self.clock)();
        if !matches!(
            tokio::time::timeout(Duration::from_secs(150), provider.verify_retired(&request)).await,
            Ok(Ok(()))
        ) {
            return Ok(RetirementOutcome::ProviderPending(
                "native retirement finality remains unconfirmed",
            ));
        }
        match tokio::time::timeout(Duration::from_secs(30), self.adapter.runtime_inventory()).await
        {
            Ok(Ok(inventory))
                if inventory.iter().all(|runtime| {
                    runtime.sandbox_id != original.id.as_str() && runtime.handle != request.handle
                }) => {}
            _ => {
                return Ok(RetirementOutcome::ProviderPending(
                    "retirement inventory is not absent",
                ))
            }
        }
        if !self.current(original, holder).await? {
            return Ok(RetirementOutcome::Superseded);
        }
        let outcome = self
            .store
            .recover_runtime_loss(SandboxRuntimeLossRequest {
                sandbox_id: original.id.clone(),
                expected_revision: original.revision,
                expected_runtime: original.runtime.clone(),
                retain_runtime: false,
                native_retirement_finality: true,
                interruption_cause: SandboxInterruptionCause::RuntimeUnavailable,
                observed_at: rfc3339_at(observed_at),
            })
            .await?;
        Ok(match outcome {
            SandboxRuntimeLossOutcome::RecoveryScheduled => RetirementOutcome::Settled,
            SandboxRuntimeLossOutcome::Superseded => RetirementOutcome::Superseded,
            SandboxRuntimeLossOutcome::NotEligible => {
                RetirementOutcome::ProviderPending("retirement settlement requires fresh evidence")
            }
            SandboxRuntimeLossOutcome::Failed => return Err(SandboxStoreError::Conflict),
        })
    }

    async fn current(&self, original: &Sandbox, holder: &str) -> Result<bool, SandboxStoreError> {
        WorkAuthority::current()?;
        let Some(current) = self.store.get_sandbox(&original.id).await? else {
            return Ok(false);
        };
        let now = seconds_from_rfc3339(&rfc3339_at((self.clock)())).unwrap_or(u64::MAX);
        Ok(Self::applies(&current)
            && current.validate().is_ok()
            && current.revision == original.revision
            && current.runtime == original.runtime
            && current.lifecycle == original.lifecycle
            && current.owner == original.owner
            && !current.retirement_archive_pending()
            && current.lifecycle.checkpoint_capture.is_none()
            && current.lifecycle.operation.is_none()
            && current.lifecycle.suspended_runtime.is_none()
            && matches!((&current.work.lease, &original.work.lease), (Some(a), Some(b))
                if a.holder == holder && a.holder == b.holder && a.fence == b.fence
                    && seconds_from_rfc3339(&a.expires_at).is_some_and(|expires| expires > now)))
    }
}
