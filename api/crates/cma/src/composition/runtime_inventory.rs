use std::{sync::Arc, time::Duration};

use layer1_public::rest::admin::AdminState;
use layer2_core::{
    sandbox::SandboxOperatorService, ActivityService, PlatformSettingsService, ProviderInventory,
    RuntimeEntry, RuntimeInventory, RuntimeStatus, StoreError,
};
use layer3_adapter::SandboxAdapter;

use crate::config::FeatureFlags;

pub(super) fn admin_state(
    features: FeatureFlags,
    settings: Arc<PlatformSettingsService>,
    operators: Arc<SandboxOperatorService>,
    activity: Arc<ActivityService>,
    adapter: Arc<dyn SandboxAdapter>,
) -> AdminState {
    let state = AdminState::new(settings, operators, activity);
    if features.admin_runtime_inventory {
        state.with_runtime_inventory(Arc::new(AdapterInventory(adapter)))
    } else {
        state
    }
}

struct AdapterInventory(Arc<dyn SandboxAdapter>);

#[async_trait::async_trait]
impl RuntimeInventory for AdapterInventory {
    async fn snapshot(&self) -> Result<ProviderInventory, StoreError> {
        let observations =
            tokio::time::timeout(Duration::from_secs(10), self.0.runtime_inventory())
                .await
                .map_err(|_| StoreError::Unavailable("runtime inventory timed out".into()))?
                .map_err(|_| StoreError::Unavailable("provider inventory unavailable".into()))?;
        Ok(ProviderInventory {
            observed_at: layer2_core::now_rfc3339(),
            runtimes: observations
                .into_iter()
                .map(|observation| RuntimeEntry {
                    runtime_id: observation.handle.0,
                    generation: observation.generation,
                    owner_reference: Some(observation.sandbox_id),
                    status: lifecycle_status(&observation.lifecycle.state),
                })
                .collect(),
        })
    }
}

// Ready describes the provider lifecycle only; it does not prove Agent/ACP readiness.
fn lifecycle_status(state: &str) -> RuntimeStatus {
    if state.eq_ignore_ascii_case("running") {
        RuntimeStatus::Ready
    } else if state.eq_ignore_ascii_case("pending") {
        RuntimeStatus::Starting
    } else {
        RuntimeStatus::Unavailable
    }
}

#[cfg(test)]
mod tests;
