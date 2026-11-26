//! A retained CRD must be deleted before paused Agents can settle retirement.
use std::time::SystemTime;

use layer2_core::sandbox::{
    store::{sandbox_content_matches, WorkAuthority},
    RuntimeBinding, Sandbox, SandboxStatus, SandboxStoreError,
};
use layer3_adapter::{AdapterError, RuntimeHandle};

use super::{
    support::{adapter_failure, WorkerFailure},
    SandboxWorker,
};

impl SandboxWorker {
    pub(super) async fn remove_stopped_native_runtime(
        &self,
        sandbox: &mut Sandbox,
        now: SystemTime,
    ) -> Result<Option<WorkerFailure>, SandboxStoreError> {
        let binding = match self.stopped_native_runtime(sandbox).await {
            Ok(Some(binding)) => binding,
            Ok(None) => return Ok(None),
            Err(error) => return Ok(Some(adapter_failure(error))),
        };
        if let Some(failure) = self.prepare_retirement_cleanup(sandbox, now).await? {
            self.suspend_credential_blocked_retirement(sandbox, &failure)
                .await?;
            return Ok(Some(failure));
        }
        let result = async {
            if let Some(home) = &sandbox.lifecycle.home {
                self.dependencies.adapter.verify_retained_home_route(
                    &RuntimeHandle::new(&binding.runtime_id),
                    &home.request,
                )?;
            }
            self.dependencies
                .adapter
                .teardown_retained_runtime(&RuntimeHandle::new(&binding.runtime_id))
                .await
        }
        .await;
        if let Err(error) = result {
            return Ok(Some(adapter_failure(error)));
        }
        sandbox.runtime = None;
        sandbox.lifecycle.suspended_runtime = None;
        Ok(None)
    }

    /// Call only after Agents stopped or the exact native runtime is already
    /// stopped or proved empty. Suspending removes compute while retaining
    /// the provider object, home and unfinished credential obligation.
    pub(super) async fn suspend_credential_blocked_retirement(
        &self,
        sandbox: &mut Sandbox,
        failure: &WorkerFailure,
    ) -> Result<(), SandboxStoreError> {
        if self
            .config
            .retirement_finalizer
            .as_ref()
            .is_some_and(|finalizer| finalizer.admission_enabled())
            && self.suspend_retiring_compute(sandbox, failure).await?
        {
            sandbox.lifecycle.suspended_runtime = sandbox.runtime.take().map(Box::new);
        }
        Ok(())
    }

    pub(super) async fn suspend_retiring_compute(
        &self,
        sandbox: &Sandbox,
        failure: &WorkerFailure,
    ) -> Result<bool, SandboxStoreError> {
        if !failure.credential_issuance_pending
            || sandbox.status != SandboxStatus::Retiring
            || sandbox.lifecycle.suspended_runtime.is_some()
            || sandbox.lifecycle.home.as_ref().is_none_or(|home| {
                home.released || home.attachment().is_none() || home.validate(&sandbox.id).is_err()
            })
        {
            return Ok(false);
        }
        let Some(binding) = sandbox
            .runtime
            .as_ref()
            .filter(|binding| binding.runtime_id.starts_with("agent-sandbox:"))
        else {
            return Ok(false);
        };
        let Some(provider) = self.dependencies.adapter.runtime_retirement() else {
            return Ok(false);
        };
        let request = match crate::sandbox::runtime_attachment::retained_runtime_request(
            sandbox,
            binding,
            &self.config.deployment_id,
        ) {
            Ok(request) => request,
            Err(_) => return Ok(false),
        };
        let current = self.dependencies.store.get_sandbox(&sandbox.id).await?;
        if !sandbox_content_matches(current.as_ref(), sandbox) {
            return Err(SandboxStoreError::Conflict);
        }
        WorkAuthority::current()?;
        if let Err(error) = provider.suspend_for_retirement(&request).await {
            tracing::debug!(error = %error, "retirement compute suspension remains unconfirmed");
            return Ok(false);
        }
        WorkAuthority::current()?;
        tracing::info!("Retiring Sandbox compute stopped while credential settlement is pending");
        Ok(true)
    }

    async fn stopped_native_runtime(
        &self,
        sandbox: &Sandbox,
    ) -> layer3_adapter::Result<Option<RuntimeBinding>> {
        let defer_unproved_runtime = self
            .config
            .retirement_finalizer
            .as_ref()
            .is_some_and(|finalizer| finalizer.admission_enabled())
            && super::super::native_retirement_finalizer::NativeRetirementFinalizer::applies(
                sandbox,
            );
        let binding = if let Some(binding) = sandbox.lifecycle.suspended_runtime.as_deref() {
            Some(binding)
        } else if let Some(binding) = sandbox.runtime.as_ref().filter(|binding| {
            sandbox.lifecycle.home.is_some() && binding.runtime_id.starts_with("agent-sandbox:")
        }) {
            // The pause PATCH may have succeeded before its acknowledgement was stored.
            let inventory = self.dependencies.adapter.runtime_inventory().await?;
            let observed = inventory
                .iter()
                .find(|runtime| runtime.handle.as_str() == binding.runtime_id);
            let removable = match observed {
                None => !defer_unproved_runtime,
                Some(runtime) if runtime.sandbox_id == sandbox.id.as_str() => {
                    matches!(
                        runtime.lifecycle.state.as_str(),
                        "suspended" | "terminating"
                    ) || (runtime.lifecycle.is_running()
                        && match self
                            .uncredentialed_native_runtime_is_idle(sandbox, binding)
                            .await
                        {
                            Ok(idle) => idle,
                            // The finalizer repeats actual failed control and
                            // retains the binding until aggregate settlement.
                            Err(_) if defer_unproved_runtime => false,
                            Err(error) => return Err(error),
                        })
                }
                Some(_) => false,
            };
            removable.then_some(binding)
        } else {
            None
        };
        Ok(binding.cloned())
    }

    async fn uncredentialed_native_runtime_is_idle(
        &self,
        sandbox: &Sandbox,
        binding: &RuntimeBinding,
    ) -> layer3_adapter::Result<bool> {
        let request = crate::sandbox::runtime_attachment::retained_runtime_request(
            sandbox,
            binding,
            &self.config.deployment_id,
        )?;
        let inspected = self
            .dependencies
            .adapter
            .inspect_runtime(&request)
            .await?
            .ok_or_else(|| AdapterError::Unavailable("retiring native runtime is absent".into()))?;
        let observed = inspected.observation;
        if observed.handle != request.handle
            || observed.sandbox_id != sandbox.id.as_str()
            || observed.generation != request.generation
            || !observed.lifecycle.is_running()
        {
            return Err(AdapterError::Rejected(
                "retiring native runtime identity changed".into(),
            ));
        }
        let status = inspected.status.ok_or_else(|| {
            AdapterError::Rejected("retiring native runtime process status missing".into())
        })?;
        if !sandbox.runtime_process_matches(&status).map_err(|_| {
            AdapterError::Rejected("retiring native runtime process identity is invalid".into())
        })? {
            return Err(AdapterError::Unavailable(
                "retiring native runtime process was replaced".into(),
            ));
        }
        // A failed revive can bind a replacement before credential delivery. Its old
        // paused Agents cannot close through that supervisor's credential gate.
        // Delete only a verified empty, uncredentialed process; ordinary running
        // Agents still close gracefully and provider absence still gates settlement.
        Ok(!status.credential_ready && status.agent_processes == 0)
    }
}
