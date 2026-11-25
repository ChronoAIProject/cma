use layer2_core::sandbox::Sandbox;
use layer3_adapter::{
    AdapterError, RuntimeAdoptionRequest, RuntimeHandle, RuntimeInspection, RuntimeStatus,
    RuntimeStatusRequest, SandboxAdapter, SandboxSpec,
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
            }) | Some(
                layer2_core::sandbox::SandboxLifecycleOperation::ImageRotation {
                    phase: layer2_core::sandbox::SandboxImageRotationPhase::TeardownRuntime,
                    ..
                }
            )
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
    let mut spec = sandbox_spec(sandbox, deployment)?;
    if spec.runtime_image.is_some() {
        spec.runtime_image = Some(binding.runtime_version.clone());
    }
    if sandbox.lifecycle.suspended_runtime.as_deref() == Some(binding) {
        // Grants are append-only and reserve a credential generation above the
        // previous authority. While this binding is suspended, managed custody
        // stays unchanged: the expanded credential is published only after the
        // resumed binding clears suspended_runtime. Reconstruct its old scope,
        // including earlier grants, rather than assigning new grants to old CRD
        // identity. The adapter still verifies the exact rendered old request.
        let current = sandbox
            .credential
            .current
            .as_ref()
            .filter(|current| current.generation > 0 && sandbox.credential.validate())
            .ok_or_else(|| {
                AdapterError::Rejected("suspended runtime credential scope is absent".into())
            })?;
        spec.reaches = if sandbox.credential.service_scope.is_some() {
            // Hot credential revisions retain the startup environment separately.
            // Credential custody now describes the current key, not the old Pod
            // template, so a credential generation cannot reconstruct that template.
            sandbox
                .credential
                .runtime_reaches(binding)
                .iter()
                .map(ToString::to_string)
                .collect()
        } else {
            sandbox
                .credential
                .reaches
                .iter()
                .chain(
                    sandbox
                        .credential
                        .service_grants
                        .iter()
                        .filter(|grant| grant.credential_generation_floor < current.generation)
                        .flat_map(|grant| &grant.services),
                )
                .map(ToString::to_string)
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect()
        };
    } else if sandbox.runtime.as_ref() != Some(binding) {
        return Err(AdapterError::Rejected(
            "retained runtime binding is not current or suspended".into(),
        ));
    }
    Ok(
        RuntimeAdoptionRequest::new(generation, RuntimeHandle::new(&binding.runtime_id), spec)
            .with_expected_image(&binding.runtime_version),
    )
}

/// Reconstruct only the already reserved, same-image failed revival. Both full
/// requests still require independent exact provider/home proof; never substitute
/// a generation read from provider metadata or the high-water fence alone.
pub(crate) fn reserved_recovery_runtime_request(
    snapshot: &layer2_core::sandbox::SandboxUnpublishedRecoverySnapshot,
    deployment: &str,
) -> layer3_adapter::Result<Option<RuntimeAdoptionRequest>> {
    let invalid = || AdapterError::Rejected("failed recovery reservation changed".into());
    let generation = snapshot
        .recovery_provision_reservation()
        .map_err(|_| invalid())?
        .map(|reservation| reservation.generation)
        .or_else(|| {
            snapshot
                .sandbox
                .legacy_unpublished_allocation()
                .then_some(snapshot.sandbox.lifecycle.runtime_generation_high_water)
        });
    let Some(generation) = generation else {
        return Ok(None);
    };
    let sandbox = &snapshot.sandbox;
    let binding = sandbox.unpublished_recovery_binding().ok_or_else(invalid)?;
    let mut request = retained_runtime_request(sandbox, binding, deployment)?;
    if sandbox_spec(sandbox, deployment)? != request.spec
        || sandbox
            .admitted_runtime_update_target()
            .is_some_and(|image| image != binding.runtime_version)
    {
        return Err(invalid());
    }
    // This candidate is never authority by itself. The caller must prove the
    // complete rendered request and original retained graph independently.
    request.generation = generation;
    Ok(Some(request))
}

/// Recovery may have proved the exact pending provider allocation while keeping
/// the older bound source. Use that durable evidence for suspension and the next
/// transition; unrelated retained-binding reconstruction remains unchanged.
pub(crate) fn recovery_runtime_request(
    sandbox: &Sandbox,
    binding: &layer2_core::sandbox::RuntimeBinding,
    deployment: &str,
) -> layer3_adapter::Result<RuntimeAdoptionRequest> {
    let mut request = retained_runtime_request(sandbox, binding, deployment)?;
    if let Some(state) = &sandbox.lifecycle.unpublished_recovery {
        state
            .validate_for(sandbox)
            .map_err(|_| AdapterError::Rejected("admitted recovery allocation changed".into()))?;
        if let Some(reservation) = &state.proof.reserved_provision {
            if sandbox.lifecycle.suspended_runtime.as_deref() != Some(binding) {
                return Err(AdapterError::Rejected(
                    "recovery allocation has no exact retained source".into(),
                ));
            }
            request.generation = reservation.generation;
        }
        if let Some(generation) = state.proof.observed_provision_generation {
            request.generation = generation;
        }
    }
    Ok(request)
}

/// Attach this replica to durable authority without allocating another runtime.
pub(crate) async fn adopt_bound_runtime(
    adapter: &dyn SandboxAdapter,
    sandbox: &Sandbox,
    deployment_id: &str,
) -> layer3_adapter::Result<RuntimeHandle> {
    Ok(inspect_bound_runtime(adapter, sandbox, deployment_id)
        .await?
        .0)
}

/// Fresh identity proof and optional capabilities from that same runtime process.
pub(crate) async fn inspect_bound_runtime(
    adapter: &dyn SandboxAdapter,
    sandbox: &Sandbox,
    deployment_id: &str,
) -> layer3_adapter::Result<(RuntimeHandle, Option<RuntimeStatus>)> {
    inspect_bound_runtime_after(
        adapter,
        sandbox,
        deployment_id,
        &layer3_adapter::InspectionAdmission::ready(),
    )
    .await
}

pub(crate) async fn inspect_bound_runtime_after(
    adapter: &dyn SandboxAdapter,
    sandbox: &Sandbox,
    deployment_id: &str,
    admission: &layer3_adapter::InspectionAdmission,
) -> layer3_adapter::Result<(RuntimeHandle, Option<RuntimeStatus>)> {
    let binding = sandbox
        .runtime
        .as_ref()
        .ok_or_else(|| AdapterError::Unavailable("sandbox runtime is not bound".into()))?;
    let handle = RuntimeHandle(binding.runtime_id.clone());
    let generation = match binding.provision_generation {
        Some(generation) => generation,
        None => {
            // Legacy inventory discovery is not part of speculative preparation.
            admission.wait().await?;
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
        sandbox_spec(sandbox, deployment_id)?,
    )
    .with_expected_image(&binding.runtime_version);
    let inspected = adapter
        .inspect_runtime_after(&request, admission)
        .await?
        .ok_or_else(runtime_absent)?;
    validate_bound_inspection(sandbox, generation, &inspected)?;
    Ok((handle, inspected.status))
}

pub(crate) fn validate_bound_inspection(
    sandbox: &Sandbox,
    generation: u64,
    inspected: &RuntimeInspection,
) -> layer3_adapter::Result<()> {
    let binding = sandbox.runtime.as_ref().ok_or_else(runtime_absent)?;
    let observed = &inspected.observation;
    if observed.handle.as_str() != binding.runtime_id
        || observed.sandbox_id != sandbox.id.as_str()
        || observed.generation != generation
        || !observed.lifecycle.is_running()
    {
        return Err(AdapterError::Rejected(
            "provider did not adopt the exact bound runtime".into(),
        ));
    }
    if sandbox.lifecycle.home.is_some() {
        let status = inspected
            .status
            .as_ref()
            .ok_or_else(|| AdapterError::Rejected("runtime process status missing".into()))?;
        if !sandbox
            .runtime_process_matches(status)
            .map_err(|_| AdapterError::Rejected("runtime process identity is invalid".into()))?
        {
            return Err(AdapterError::Unavailable(
                "bound runtime process was replaced; durable recovery is pending".into(),
            ));
        }
    }
    Ok(())
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
