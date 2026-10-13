use layer2_core::sandbox::Sandbox;
use layer3_adapter::{
    AdapterError, RuntimeAdoptionRequest, RuntimeHandle, RuntimeStatus, RuntimeStatusRequest,
    SandboxAdapter, SandboxSpec,
};

use super::runtime_mapping::sandbox_spec;

pub(crate) fn native_pause_teardown(sandbox: &Sandbox) -> bool {
    sandbox.lifecycle.home.is_some()
        && sandbox
            .runtime
            .as_ref()
            .is_some_and(|binding| binding.runtime_id.starts_with("agent-sandbox:"))
        && matches!(
            sandbox.lifecycle.operation,
            Some(layer2_core::sandbox::SandboxLifecycleOperation::Pause {
                phase: layer2_core::sandbox::SandboxPausePhase::TeardownRuntime,
                ..
            })
        )
}

pub(crate) fn retained_runtime_request(
    sandbox: &Sandbox,
    binding: &layer2_core::sandbox::RuntimeBinding,
    deployment: &str,
) -> layer3_adapter::Result<RuntimeAdoptionRequest> {
    let generation = binding
        .provision_generation
        .filter(|generation| *generation > 0)
        .ok_or_else(|| AdapterError::Rejected("retained runtime generation is absent".into()))?;
    Ok(RuntimeAdoptionRequest::new(
        generation,
        RuntimeHandle::new(&binding.runtime_id),
        sandbox_spec(sandbox, deployment),
    )
    .with_expected_image(&binding.runtime_version))
}

/// Attach this replica to durable authority without allocating another runtime.
pub(crate) async fn adopt_bound_runtime(
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
    if sandbox.lifecycle.home.is_some() {
        let status = adapter.runtime_status(&handle).await?;
        if !sandbox
            .runtime_process_matches(&status)
            .map_err(|_| AdapterError::Rejected("runtime process identity is invalid".into()))?
        {
            return Err(AdapterError::Unavailable(
                "bound runtime process was replaced; durable recovery is pending".into(),
            ));
        }
    }
    Ok(handle)
}

/// Persist this process identity with the first binding, before installation or credentials.
pub(super) async fn provisioned_runtime_status(
    adapter: &dyn SandboxAdapter,
    spec: &SandboxSpec,
    handle: &RuntimeHandle,
    generation: u64,
) -> layer3_adapter::Result<Option<RuntimeStatus>> {
    let Some(home) = &spec.home_volume else {
        return Ok(None);
    };
    let status = adapter.runtime_status(handle).await?;
    status.validate_for(&RuntimeStatusRequest {
        sandbox_id: spec.sandbox_id.clone(),
        generation,
        home_identity: home.identity()?,
    })?;
    Ok(Some(status))
}

fn runtime_absent() -> AdapterError {
    // Only the reconciler may publish runtime loss against a fresh DB revision.
    AdapterError::Unavailable("bound runtime is unavailable pending reconciliation".into())
}
