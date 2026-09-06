use layer2_core::sandbox::Sandbox;
use layer3_adapter::{AdapterError, RuntimeAdoptionRequest, RuntimeHandle, SandboxAdapter};

use super::runtime_mapping::sandbox_spec;

/// Attach this replica to durable authority without allocating another runtime.
pub(super) async fn adopt_bound_runtime(
    adapter: &dyn SandboxAdapter,
    sandbox: &Sandbox,
    deployment_id: &str,
) -> layer3_adapter::Result<RuntimeHandle> {
    let binding = sandbox
        .runtime
        .as_ref()
        .ok_or_else(|| AdapterError::Unavailable("sandbox runtime is not bound".into()))?;
    let handle = RuntimeHandle(binding.runtime_id.clone());
    let generation = match binding.provision_generation {
        Some(generation) => generation,
        None => {
            let observation = adapter
                .runtime_inventory()
                .await?
                .into_iter()
                .find(|observation| observation.handle == handle)
                .ok_or_else(runtime_absent)?;
            if observation.sandbox_id != sandbox.id.as_str() {
                return Err(AdapterError::Rejected(
                    "bound runtime belongs to another sandbox".into(),
                ));
            }
            observation.generation
        }
    };
    let request = RuntimeAdoptionRequest::new(
        generation,
        handle.clone(),
        sandbox_spec(sandbox, deployment_id),
    )
    .with_expected_image(&binding.runtime_version);
    let observed = adapter
        .adopt_runtime(&request)
        .await?
        .ok_or_else(runtime_absent)?;
    if observed.handle != handle
        || observed.sandbox_id != sandbox.id.as_str()
        || observed.generation != generation
        || !observed.lifecycle.is_running()
    {
        return Err(AdapterError::Rejected(
            "provider did not adopt the exact bound runtime".into(),
        ));
    }
    Ok(handle)
}

fn runtime_absent() -> AdapterError {
    // Only the reconciler may publish runtime loss against a fresh DB revision.
    AdapterError::Unavailable("bound runtime is unavailable pending reconciliation".into())
}
