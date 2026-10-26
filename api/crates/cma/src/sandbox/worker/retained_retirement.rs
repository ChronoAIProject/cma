//! A retained CRD must be deleted before paused Agents can settle retirement.
use layer2_core::sandbox::{RuntimeBinding, Sandbox};
use layer3_adapter::{AdapterError, RuntimeHandle};

use super::SandboxWorker;

impl SandboxWorker {
    pub(super) async fn remove_stopped_native_runtime(
        &self,
        sandbox: &mut Sandbox,
    ) -> layer3_adapter::Result<()> {
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
                None => true,
                Some(runtime) if runtime.sandbox_id == sandbox.id.as_str() => {
                    matches!(
                        runtime.lifecycle.state.as_str(),
                        "suspended" | "terminating"
                    ) || (runtime.lifecycle.is_running()
                        && self
                            .uncredentialed_native_runtime_is_idle(sandbox, binding)
                            .await?)
                }
                Some(_) => false,
            };
            removable.then_some(binding)
        } else {
            None
        };
        if let Some(binding) = binding {
            if let Some(home) = &sandbox.lifecycle.home {
                self.dependencies.adapter.verify_retained_home_route(
                    &RuntimeHandle::new(&binding.runtime_id),
                    &home.request,
                )?;
            }
            self.dependencies
                .adapter
                .teardown_retained_runtime(&RuntimeHandle::new(&binding.runtime_id))
                .await?;
            sandbox.runtime = None;
            sandbox.lifecycle.suspended_runtime = None;
        }
        Ok(())
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
