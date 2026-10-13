//! A retained CRD must be deleted before paused Agents can settle retirement.
use layer2_core::sandbox::Sandbox;
use layer3_adapter::RuntimeHandle;

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
            // A running provider still follows the ordinary graceful Agent stop path.
            let inventory = self.dependencies.adapter.runtime_inventory().await?;
            inventory
                .iter()
                .find(|runtime| runtime.handle.as_str() == binding.runtime_id)
                .is_none_or(|runtime| {
                    runtime.sandbox_id == sandbox.id.as_str()
                        && matches!(
                            runtime.lifecycle.state.as_str(),
                            "suspended" | "terminating"
                        )
                })
                .then_some(binding)
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
}
