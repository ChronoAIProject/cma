use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{Sandbox, SandboxEventKind, SandboxStatus, SandboxWorkStep};
use layer3_adapter::RuntimeHandle;

use super::super::native_retirement_finalizer::{NativeRetirementFinalizer, RetirementOutcome};
use super::support::{
    adapter_failure, credential_failure, dependency_failure, operation_id, system_time,
    WorkerFailure,
};
use super::{retirement_credentials::custody_provider_ids, SandboxWorker};

impl SandboxWorker {
    pub(super) async fn stop_agents(
        &self,
        mut sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let finalizer = self
            .config
            .retirement_finalizer
            .as_ref()
            .filter(|_| NativeRetirementFinalizer::applies(&sandbox));
        if let Some(finalizer) = finalizer {
            if finalizer.holds_custody(&sandbox).await? {
                return self
                    .finalize_native_retirement(finalizer, sandbox, now)
                    .await;
            }
        }
        if let Some(failure) = self
            .remove_stopped_native_runtime(&mut sandbox, now)
            .await?
        {
            return self.retry_failure(sandbox, failure, now).await;
        }
        if let Some(finalizer) = finalizer {
            if let Err(error) = finalizer.offer_capture(&sandbox).await {
                // Continue the regular finalizer checks, including complete provider absence.
                tracing::debug!(%error, "retirement capture offer was not confirmed");
            }
        }
        match self.request_agent_stop(&sandbox).await {
            Ok(()) => {
                self.advance(
                    sandbox,
                    SandboxWorkStep::RevokeRetirementCredentials,
                    now,
                    SandboxEventKind::WorkStepCompleted {
                        step: SandboxWorkStep::StopAgents,
                    },
                )
                .await
            }
            Err(super::SandboxDependencyError::AgentsStopping)
                if finalizer.is_some_and(|finalizer| finalizer.admission_enabled()) =>
            {
                self.finalize_native_retirement(
                    finalizer.expect("selected finalizer"),
                    sandbox,
                    now,
                )
                .await
            }
            Err(error) => {
                self.retry_failure(sandbox, dependency_failure(error), now)
                    .await
            }
        }
    }

    async fn finalize_native_retirement(
        &self,
        finalizer: &NativeRetirementFinalizer,
        mut sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let mut result = finalizer.finalize(&sandbox, &self.config.holder).await;
        if matches!(result, Ok(RetirementOutcome::GracefulWithCustody)) {
            if let Err(error) = self.request_agent_stop(&sandbox).await {
                return self
                    .retry_failure(sandbox, dependency_failure(error), now)
                    .await;
            }
            let mut cleaned = sandbox.clone();
            if let Some(failure) = self.prepare_retirement_cleanup(&mut cleaned, now).await? {
                // Dispatch custody continues to own the original runtime binding.
                self.suspend_retiring_compute(&sandbox, &failure).await?;
                return self.retry_failure(sandbox, failure, now).await;
            }
            sandbox = cleaned;
            result = finalizer
                .finish_graceful_stop(&sandbox, &self.config.holder)
                .await;
        }
        let reason = match result {
            Ok(RetirementOutcome::Settled | RetirementOutcome::Superseded) => return Ok(()),
            Ok(RetirementOutcome::GracefulPending | RetirementOutcome::GracefulWithCustody) => {
                "sandbox agents are still stopping"
            }
            Ok(RetirementOutcome::ProviderPending(reason)) => reason,
            Err(SandboxStoreError::Conflict) => "native retirement authority changed",
            Err(error) => return Err(error),
        };
        self.retry_failure(sandbox, WorkerFailure::waiting(reason), now)
            .await
    }

    async fn request_agent_stop(
        &self,
        sandbox: &Sandbox,
    ) -> Result<(), super::SandboxDependencyError> {
        let runtime = sandbox
            .runtime
            .as_ref()
            .map(|binding| RuntimeHandle(binding.runtime_id.clone()));
        self.dependencies
            .agents
            .stop_live_agents(sandbox, runtime.as_ref(), &operation_id(sandbox))
            .await
    }

    pub(super) async fn revoke_for_retirement(
        &self,
        mut sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        if self.retirement_reauthorization_disabled(&sandbox) {
            let failure = super::support::WorkerFailure::new(
                "owner_reauthorization_disabled",
                "revision-fenced Operations resume is required",
                false,
            );
            return self.retry_failure(sandbox, failure, now).await;
        }
        if sandbox
            .work
            .retirement_credential_recovery
            .is_some_and(|recovery| recovery.admitted.is_none())
        {
            return self.probe_retirement_reauthorization(sandbox, now).await;
        }
        if let Err(failure) = self.settle_retirement_credentials(&mut sandbox, now).await {
            self.suspend_credential_blocked_retirement(&mut sandbox, &failure)
                .await?;
            return self.retry_failure(sandbox, failure, now).await;
        }
        self.advance(
            sandbox,
            SandboxWorkStep::TeardownRuntime,
            now,
            SandboxEventKind::WorkStepCompleted {
                step: SandboxWorkStep::RevokeRetirementCredentials,
            },
        )
        .await
    }

    pub(super) async fn mark_retired(
        &self,
        mut sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        if sandbox.runtime.is_some() || sandbox.lifecycle.suspended_runtime.is_some() {
            return Err(SandboxStoreError::Corrupt(
                "home release still has a bound runtime".into(),
            ));
        }
        if let Some(failure) = self.cleanup_native_home(&mut sandbox, now).await? {
            return self.retry_failure(sandbox, failure, now).await;
        }
        if sandbox.lifecycle.background_checkpoint.is_some() {
            return self
                .retry_failure(
                    sandbox,
                    WorkerFailure::waiting("finishing background snapshot cleanup"),
                    now,
                )
                .await;
        }
        if let Some(failure) = self.prepare_retirement_cleanup(&mut sandbox, now).await? {
            return self.retry_failure(sandbox, failure, now).await;
        }
        if let Some(home) = &mut sandbox.lifecycle.home {
            let was_released = home.released;
            if let Err(error) =
                crate::sandbox::home::release_home(self.dependencies.adapter.as_ref(), home).await
            {
                return self
                    .retry_failure(sandbox, adapter_failure(error), now)
                    .await;
            }
            if !was_released {
                let expected = sandbox.revision;
                sandbox.revision = expected.checked_add(1).ok_or(SandboxStoreError::Conflict)?;
                sandbox.updated_at = sandbox.updated_at.max(rfc3339_at(now));
                self.dependencies
                    .store
                    .save_sandbox(expected, sandbox.clone())
                    .await?;
            }
        }
        let policy = match self.config.policy.current_sandbox_policy().await {
            Ok(policy) => policy,
            Err(_) => {
                return self
                    .retry_failure(
                        sandbox,
                        WorkerFailure::retryable("sandbox policy is unavailable"),
                        now,
                    )
                    .await
            }
        };
        // Agent-sandbox owns the runtime state on the durable home PVC. CMA
        // does not purge checkpoint archives during retirement.
        let purge_at = now + policy.retention_grace;
        sandbox.purge_after = Some(rfc3339_at(purge_at));
        let next = if sandbox.credential.revoke_debt.is_empty() {
            SandboxWorkStep::WaitForPurge
        } else {
            SandboxWorkStep::RetryRetiredRevocations
        };
        let ready = if next == SandboxWorkStep::WaitForPurge {
            purge_at
        } else {
            now
        };
        self.advance_status_at(sandbox, SandboxStatus::Retired, next, now, ready)
            .await
    }

    pub(super) async fn retry_retired_revoke(
        &self,
        mut sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let Some(debt) = sandbox.credential.revoke_debt.first().cloned() else {
            let ready = sandbox
                .purge_after
                .as_deref()
                .and_then(system_time)
                .unwrap_or(now);
            return self
                .advance_at(
                    sandbox,
                    SandboxWorkStep::WaitForPurge,
                    now,
                    ready,
                    SandboxEventKind::WorkStepCompleted {
                        step: SandboxWorkStep::RetryRetiredRevocations,
                    },
                )
                .await;
        };
        match self
            .dependencies
            .credentials
            .revoke(&sandbox, &debt.provider_key_id)
            .await
        {
            Ok(()) => {
                if let Err(error) = self
                    .dependencies
                    .credentials
                    .forget_provider(&sandbox.id, &debt.provider_key_id)
                    .await
                {
                    return self
                        .retry_failure(sandbox, credential_failure(error), now)
                        .await;
                }
                sandbox.credential.revoke_debt.remove(0);
                let (next, ready) = if sandbox.credential.revoke_debt.is_empty() {
                    (
                        SandboxWorkStep::WaitForPurge,
                        sandbox
                            .purge_after
                            .as_deref()
                            .and_then(system_time)
                            .unwrap_or(now),
                    )
                } else {
                    (SandboxWorkStep::RetryRetiredRevocations, now)
                };
                self.advance_at(
                    sandbox,
                    next,
                    now,
                    ready,
                    SandboxEventKind::WorkStepCompleted {
                        step: SandboxWorkStep::RetryRetiredRevocations,
                    },
                )
                .await
            }
            Err(error) => {
                self.retry_failure(sandbox, credential_failure(error), now)
                    .await
            }
        }
    }

    pub(super) async fn ready_to_purge(
        &self,
        mut sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        if let Err(error) = self
            .dependencies
            .credentials
            .settle_revocations(&sandbox)
            .await
        {
            return self
                .retry_failure(sandbox, credential_failure(error), now)
                .await;
        }
        let custodied = match self.dependencies.credentials.custodied(&sandbox.id).await {
            Ok(values) => values,
            Err(error) => {
                return self
                    .retry_failure(sandbox, credential_failure(error), now)
                    .await
            }
        };
        let ids = match custody_provider_ids(&sandbox, &custodied) {
            Ok(ids) => ids,
            Err(failure) => return self.retry_failure(sandbox, failure, now).await,
        };
        if !ids.is_empty() || !sandbox.credential.revoke_debt.is_empty() {
            for id in ids {
                sandbox.credential.queue_revoke(id, rfc3339_at(now));
            }
            return self.defer_purge_for_revocation(sandbox, now).await;
        }
        self.advance(
            sandbox,
            SandboxWorkStep::Purge,
            now,
            SandboxEventKind::WorkStepCompleted {
                step: SandboxWorkStep::WaitForPurge,
            },
        )
        .await
    }

    pub(super) async fn purge(
        &self,
        mut sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        if !sandbox.credential.revoke_debt.is_empty() {
            return self.defer_purge_for_revocation(sandbox, now).await;
        }
        if let Err(error) = self
            .dependencies
            .credentials
            .settle_revocations(&sandbox)
            .await
        {
            return self
                .retry_failure(sandbox, credential_failure(error), now)
                .await;
        }
        let custodied = match self.dependencies.credentials.custodied(&sandbox.id).await {
            Ok(values) => values,
            Err(error) => {
                return self
                    .retry_failure(sandbox, credential_failure(error), now)
                    .await
            }
        };
        let ids = match custody_provider_ids(&sandbox, &custodied) {
            Ok(ids) => ids,
            Err(failure) => return self.retry_failure(sandbox, failure, now).await,
        };
        if !ids.is_empty() {
            for id in ids {
                sandbox.credential.queue_revoke(id, rfc3339_at(now));
            }
            return self.defer_purge_for_revocation(sandbox, now).await;
        }
        self.dependencies
            .store
            .purge_sandbox(&sandbox.id, sandbox.revision, &rfc3339_at(now))
            .await
    }

    async fn defer_purge_for_revocation(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let step = sandbox.work.step;
        self.advance(
            sandbox,
            SandboxWorkStep::RetryRetiredRevocations,
            now,
            SandboxEventKind::WorkStepFailed {
                step,
                failure: WorkerFailure::retryable("credential cleanup remained before purge")
                    .typed(),
            },
        )
        .await
    }

    pub(super) async fn fail_corrupt_claim(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        self.handle_failure(
            sandbox,
            WorkerFailure::terminal_security("sandbox lifecycle state is inconsistent"),
            now,
        )
        .await
    }
}
