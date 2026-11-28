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
use layer3_adapter::{
    AdapterError, RuntimeAdoptionRequest, RuntimeObservation, RuntimeRetirement, SandboxAdapter,
};

use crate::store::native_retirement_action::{
    capture::RetirementCaptureConsumer, RetirementAction, RetirementPhase, RetirementRecords,
};

mod dispatch;
mod receiver;

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
    GracefulWithCustody,
    ProviderPending(&'static str),
    Settled,
    Superseded,
}

pub(crate) struct NativeRetirementFinalizer {
    store: Arc<dyn RetirementStore>,
    adapter: Arc<dyn SandboxAdapter>,
    deployment: String,
    clock: Arc<dyn Fn() -> SystemTime + Send + Sync>,
    records: Arc<dyn RetirementRecords>,
    admit: bool,
    capture: Option<Arc<dyn RetirementCaptureConsumer>>,
    capture_admission: Option<crate::agent_lifecycle::LifecycleDrain>,
    probe_unreachable: bool,
}

impl NativeRetirementFinalizer {
    pub(crate) fn new<S>(
        store: Arc<S>,
        adapter: Arc<dyn SandboxAdapter>,
        deployment: String,
        records: Arc<dyn RetirementRecords>,
        admit: bool,
    ) -> Self
    where
        S: SandboxStore + SandboxReconciliationStore + SandboxLifecycleExecutionStore + 'static,
    {
        Self {
            store,
            adapter,
            deployment,
            records,
            admit,
            capture: None,
            capture_admission: None,
            probe_unreachable: false,
            clock: Arc::new(SystemTime::now),
        }
    }

    pub(crate) fn with_capture(
        mut self,
        capture: Arc<dyn RetirementCaptureConsumer>,
        admission: Option<crate::agent_lifecycle::LifecycleDrain>,
    ) -> Self {
        self.capture = Some(capture);
        self.capture_admission = admission;
        self
    }

    pub(crate) fn with_unreachable_probe(mut self, enabled: bool) -> Self {
        self.probe_unreachable = enabled;
        self
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
        self.finalize_attempt(original, holder, false).await
    }

    pub(crate) async fn finish_graceful_stop(
        &self,
        original: &Sandbox,
        holder: &str,
    ) -> Result<RetirementOutcome, SandboxStoreError> {
        self.finalize_attempt(original, holder, true).await
    }

    async fn finalize_attempt(
        &self,
        original: &Sandbox,
        holder: &str,
        after_graceful_stop: bool,
    ) -> Result<RetirementOutcome, SandboxStoreError> {
        if !Self::applies(original) || !self.current(original, holder).await? {
            return Ok(RetirementOutcome::Superseded);
        }
        let mut action = self.records.action(&original.id).await?;
        if after_graceful_stop {
            let Some(action) = &action else {
                return Ok(RetirementOutcome::Superseded);
            };
            if !self.graceful_stop_complete(original, action).await? {
                return Ok(RetirementOutcome::GracefulWithCustody);
            }
        }
        // Give the ordinary child workers three opportunities to close cleanly.
        // Work already in custody continues after the admission switch is off.
        if action.is_none() && (!self.admit || original.work.attempts < 3) {
            return Ok(RetirementOutcome::GracefulPending);
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
        let present = match self.present(original, &request).await {
            Ok(present) => present,
            Err(reason) => return Ok(RetirementOutcome::ProviderPending(reason)),
        };
        if action
            .as_ref()
            .is_some_and(|existing| !existing.matches(original))
        {
            return Ok(RetirementOutcome::ProviderPending(
                "retirement custody identity changed",
            ));
        }
        if let Some(observed) = present {
            let stopped = matches!(
                observed.lifecycle.state.as_str(),
                "suspended" | "terminating"
            ) && action
                .as_ref()
                .is_some_and(|action| action.phase == RetirementPhase::Dispatching);
            if stopped && !after_graceful_stop {
                return Ok(RetirementOutcome::GracefulWithCustody);
            }
            let probe_owns_identity = action.as_ref().map_or(self.probe_unreachable, |action| {
                action.observation.receiver.pod.is_uid_only()
            });
            if !stopped
                && !probe_owns_identity
                && !self.runtime_identity_unchanged(original, &request).await
            {
                return Ok(RetirementOutcome::ProviderPending(
                    "retirement runtime identity changed or was refused",
                ));
            }
            if action.is_none() {
                action = self.admit_action(original, provider, &request).await?;
            }
            let Some(current) = action.as_mut() else {
                return Ok(RetirementOutcome::GracefulPending);
            };
            let outcome = if stopped {
                self.dispatch_suspended(original, holder, provider, &request, current)
                    .await?
            } else {
                self.dispatch(
                    original,
                    holder,
                    provider,
                    &request,
                    current,
                    after_graceful_stop,
                )
                .await?
            };
            if let Some(outcome) = outcome {
                return Ok(outcome);
            }
        }
        // The freshness boundary precedes the complete absence proof.
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
        let settlement = SandboxRuntimeLossRequest {
            sandbox_id: original.id.clone(),
            expected_revision: original.revision,
            expected_runtime: original.runtime.clone(),
            retain_runtime: false,
            native_retirement_finality: true,
            interruption_cause: SandboxInterruptionCause::RuntimeUnavailable,
            observed_at: rfc3339_at(observed_at),
        };
        let outcome = match action {
            Some(action) => self.records.settle(settlement, &action).await?,
            None => self.store.recover_runtime_loss(settlement).await?,
        };
        Ok(match outcome {
            SandboxRuntimeLossOutcome::RecoveryScheduled => RetirementOutcome::Settled,
            SandboxRuntimeLossOutcome::Superseded => RetirementOutcome::Superseded,
            SandboxRuntimeLossOutcome::NotEligible => {
                RetirementOutcome::ProviderPending("retirement settlement requires fresh evidence")
            }
            SandboxRuntimeLossOutcome::Failed => return Err(SandboxStoreError::Conflict),
        })
    }

    pub(crate) async fn holds_custody(&self, sandbox: &Sandbox) -> Result<bool, SandboxStoreError> {
        Ok(Self::applies(sandbox) && self.records.action(&sandbox.id).await?.is_some())
    }

    pub(crate) async fn has_dispatched(
        &self,
        sandbox: &Sandbox,
    ) -> Result<bool, SandboxStoreError> {
        Ok(Self::applies(sandbox)
            && self
                .records
                .action(&sandbox.id)
                .await?
                .is_some_and(|action| {
                    action.matches(sandbox) && action.phase == RetirementPhase::Dispatching
                }))
    }

    pub(crate) fn admission_enabled(&self) -> bool {
        self.admit
    }

    async fn graceful_stop_complete(
        &self,
        parent: &Sandbox,
        action: &RetirementAction,
    ) -> Result<bool, SandboxStoreError> {
        if action.phase != RetirementPhase::Dispatching
            || !action.matches(parent)
            || parent.credential.current.is_some()
            || parent.credential.pending.is_some()
            || parent.credential.delivered
            || !parent.credential.revoke_debt.is_empty()
        {
            return Ok(false);
        }
        let children = super::live_agent_stopper::retirement_agents(self.store.as_ref(), parent)
            .await
            .map_err(|_| SandboxStoreError::Conflict)?;
        Ok(children.iter().all(|agent| !agent.status.is_live())
            && action.covers(parent, &children)?)
    }

    async fn runtime_identity_unchanged(
        &self,
        sandbox: &Sandbox,
        request: &RuntimeAdoptionRequest,
    ) -> bool {
        let (inspection, unavailable) =
            layer3_adapter::receiver_failure::observe(tokio::time::timeout(
                Duration::from_secs(5),
                self.adapter.inspect_runtime(request),
            ))
            .await;
        // A readable status must still identify the persisted process. Missing
        // status supplies no authority. The exact ownership probe is required.
        match inspection {
            Ok(Ok(Some(inspected))) => {
                inspected.observation.handle == request.handle
                    && inspected.observation.sandbox_id == sandbox.id.as_str()
                    && inspected.observation.generation == request.generation
                    && inspected
                        .status
                        .as_ref()
                        .is_none_or(|status| sandbox.runtime_process_matches(status) == Ok(true))
            }
            Ok(Ok(None) | Err(AdapterError::Unavailable(_))) | Err(_) => true,
            Ok(Err(_)) => unavailable.is_some(),
        }
    }

    async fn present(
        &self,
        sandbox: &Sandbox,
        request: &RuntimeAdoptionRequest,
    ) -> Result<Option<RuntimeObservation>, &'static str> {
        let inventory =
            match tokio::time::timeout(Duration::from_secs(30), self.adapter.runtime_inventory())
                .await
            {
                Ok(Ok(inventory)) => inventory,
                _ => return Err("retirement inventory is unavailable"),
            };
        if inventory.iter().any(|item| {
            (item.sandbox_id == sandbox.id.as_str() || item.handle == request.handle)
                && (item.sandbox_id != sandbox.id.as_str()
                    || item.handle != request.handle
                    || item.generation != request.generation)
        }) {
            return Err("retirement inventory contains changed authority");
        }
        Ok(inventory
            .into_iter()
            .find(|item| item.handle == request.handle))
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

async fn probe(
    provider: &dyn RuntimeRetirement,
    request: &RuntimeAdoptionRequest,
    action: &RetirementAction,
) -> Option<bool> {
    tokio::time::timeout(
        Duration::from_secs(if action.observation.receiver.pod.is_uid_only() {
            15
        } else {
            5
        }),
        provider.challenge_retirement_receiver(request, &action.observation.receiver),
    )
    .await
    .ok()?
    .ok()
}
