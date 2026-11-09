//! Exact retained native/home checks, before and after provider history inspection.
use layer2_core::sandbox::Sandbox;
use layer3_adapter::recovery_diagnostics::{Observation, Reason, Stage};
use layer3_adapter::{AdapterError, RuntimeRecoveryInspectionRequest, SandboxAdapter};

pub(crate) async fn verify_source(
    adapter: &dyn SandboxAdapter,
    sandbox: &Sandbox,
    source: &RuntimeRecoveryInspectionRequest,
    deployment: &str,
) -> layer3_adapter::Result<()> {
    verify_source_at(adapter, sandbox, source, deployment, Stage::NativeBefore).await
}

pub(super) async fn verify_source_at(
    adapter: &dyn SandboxAdapter,
    sandbox: &Sandbox,
    source: &RuntimeRecoveryInspectionRequest,
    deployment: &str,
    stage: Stage,
) -> layer3_adapter::Result<()> {
    let mut observation = Observation::new(stage);
    match check(adapter, sandbox, source, deployment).await {
        Ok(()) => {
            observation.finish(Reason::Success);
            Ok(())
        }
        Err((reason, error)) => {
            observation.finish(reason);
            Err(error)
        }
    }
}

async fn check(
    adapter: &dyn SandboxAdapter,
    sandbox: &Sandbox,
    source: &RuntimeRecoveryInspectionRequest,
    deployment: &str,
) -> Result<(), (Reason, AdapterError)> {
    let binding = sandbox
        .runtime
        .as_ref()
        .or(sandbox.lifecycle.suspended_runtime.as_deref())
        .ok_or_else(|| refusal(Reason::NativeChanged))?;
    if binding.runtime_id != source.runtime.as_str()
        || binding.provision_generation != Some(source.identity.generation)
        || binding.boot_id.as_deref() != Some(source.boot_id.as_str())
        || sandbox.id.as_str() != source.identity.sandbox_id
    {
        return Err(refusal(Reason::NativeChanged));
    }
    let attachment = sandbox
        .lifecycle
        .home
        .as_ref()
        .filter(|home| !home.released)
        .and_then(|home| home.attachment())
        .ok_or_else(|| refusal(Reason::NativeChanged))?;
    if attachment
        .identity()
        .map_err(|error| classified(error, Reason::HomeChanged))?
        != source.identity.home_identity
    {
        return Err(refusal(Reason::HomeChanged));
    }
    let volumes = adapter
        .home_volumes()
        .ok_or_else(|| refusal(Reason::NativeChanged))?;
    volumes
        .verify(&attachment)
        .await
        .map_err(|error| classified(error, Reason::HomeChanged))?;
    let request =
        super::super::runtime_attachment::retained_runtime_request(sandbox, binding, deployment)
            .map_err(|error| classified(error, Reason::NativeChanged))?;
    let inspection = adapter
        .inspect_runtime(&request)
        .await
        .map_err(|error| classified(error, Reason::NativeChanged))?
        .ok_or_else(|| refusal(Reason::NativeChanged))?;
    if inspection.observation.handle != source.runtime
        || inspection.observation.sandbox_id != source.identity.sandbox_id
        || inspection.observation.generation != source.identity.generation
        || !inspection.observation.lifecycle.is_running()
    {
        return Err(refusal(Reason::NativeChanged));
    }
    let status = inspection
        .status
        .ok_or_else(|| refusal(Reason::NativeChanged))?;
    status
        .validate_for(&source.identity)
        .map_err(|error| classified(error, Reason::NativeChanged))?;
    if status.boot_id != source.boot_id {
        return Err(refusal(Reason::BootMismatch));
    }
    if status.restore_pending {
        return Err(refusal(Reason::RestorePending));
    }
    volumes
        .verify(&attachment)
        .await
        .map_err(|error| classified(error, Reason::HomeChanged))
}

fn classified(error: AdapterError, fallback: Reason) -> (Reason, AdapterError) {
    (Reason::adapter(&error, fallback), error)
}

fn refusal(reason: Reason) -> (Reason, AdapterError) {
    (
        reason,
        AdapterError::Rejected("retained recovery runtime or home authority changed".into()),
    )
}
