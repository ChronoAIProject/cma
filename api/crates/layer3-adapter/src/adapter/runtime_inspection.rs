//! Default combined inspection for adapters without a native combined call.
use crate::{Result, RuntimeAdoptionRequest, RuntimeInspection, SandboxAdapter};

pub(super) async fn inspect<A: SandboxAdapter + ?Sized>(
    adapter: &A,
    request: &RuntimeAdoptionRequest,
) -> Result<Option<RuntimeInspection>> {
    let Some(observation) = adapter.adopt_runtime(request).await? else {
        return Ok(None);
    };
    if observation.handle != request.handle
        || observation.sandbox_id != request.spec.sandbox_id
        || observation.generation != request.generation
        || !observation.lifecycle.is_running()
    {
        return Err(crate::AdapterError::Rejected(
            "provider did not adopt the exact bound runtime".into(),
        ));
    }
    let status = if request.spec.home_volume.is_some() {
        Some(adapter.runtime_status(&request.handle).await?)
    } else {
        None
    };
    Ok(Some(crate::RuntimeInspection {
        observation,
        status,
    }))
}
