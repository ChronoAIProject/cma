//! Production admin services use the same configured provider as reconciliation.
use std::sync::Arc;

use layer1_public::rest::admin::AdminState;
use layer2_core::{
    sandbox::SandboxOperatorService, ActivityService, PlatformSettingsService, ProviderInventory,
    RuntimeEntry, RuntimeInventory, RuntimeStatus, StoreError,
};
use layer3_adapter::{RuntimeObservation, SandboxAdapter};

pub(super) fn state(
    settings: Arc<PlatformSettingsService>,
    sandboxes: Arc<SandboxOperatorService>,
    activity: Arc<ActivityService>,
    adapter: Arc<dyn SandboxAdapter>,
) -> AdminState {
    AdminState::new(settings, sandboxes, activity)
        .with_runtime_inventory(Arc::new(ProviderRuntimeInventory(adapter)))
}

struct ProviderRuntimeInventory(Arc<dyn SandboxAdapter>);

#[async_trait::async_trait]
impl RuntimeInventory for ProviderRuntimeInventory {
    async fn snapshot(&self) -> Result<ProviderInventory, StoreError> {
        let observations = self.0.runtime_inventory().await.map_err(|_| {
            StoreError::Unavailable("provider runtime inventory is unavailable".into())
        })?;
        Ok(ProviderInventory {
            observed_at: layer2_core::now_rfc3339(),
            runtimes: observations.into_iter().map(entry).collect(),
        })
    }
}

fn entry(observation: RuntimeObservation) -> RuntimeEntry {
    let status = if observation.lifecycle.is_running() {
        RuntimeStatus::Ready
    } else if matches!(
        observation.lifecycle.state.to_ascii_lowercase().as_str(),
        "pending" | "starting" | "provisioning"
    ) {
        RuntimeStatus::Starting
    } else {
        // Suspended, terminating and unknown states are never reported Ready.
        RuntimeStatus::Unavailable
    };
    RuntimeEntry {
        runtime_id: observation.handle.0,
        generation: observation.generation,
        owner_reference: Some(observation.sandbox_id),
        status,
    }
}

#[cfg(test)]
mod tests;
