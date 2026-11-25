use super::{provider_request_identity, refused, MemoryRuntime};
use crate::wire::{ResponseFrame, WireOperation};
use crate::{RuntimeStatus, RuntimeStatusRequest};

pub(super) fn inspect(
    runtime: &MemoryRuntime,
    request: RuntimeStatusRequest,
) -> crate::Result<ResponseFrame> {
    request.validate()?;
    let identity = provider_request_identity(&runtime.request)?;
    if identity.sandbox_id != request.sandbox_id
        || identity.generation != request.generation
        || runtime.request.env.get("CHRONO_RUNTIME_HOME_IDENTITY") != Some(&request.home_identity)
    {
        return refused(
            WireOperation::RuntimeStatus,
            "runtime_status_identity_invalid",
        );
    }
    let restore_pending = runtime.restore_bootstrap.is_some() && runtime.restored_object.is_none();
    let status = RuntimeStatus {
        agent_filesystem: false,
        agent_attachment_upload: false,
        agent_skill_install: true,
        agent_skill_upgrade: true,
        agent_model_catalog: true,
        agent_browser: false,
        agent_resource_preparation: false,
        agent_process_restart: false,
        exact_github_credential_route: false,
        github_repository_scope: false,
        sandbox_id: request.sandbox_id.clone(),
        generation: request.generation,
        home_identity: request.home_identity.clone(),
        boot_id: runtime.boot_id.clone(),
        installation_ready: runtime.install.is_some()
            || (runtime.restore_bootstrap.is_some() && !restore_pending),
        credential_ready: runtime.credential_delivered,
        restore_pending,
        agent_processes: runtime
            .agents
            .values()
            .filter(|agent| agent.running)
            .count()
            .try_into()
            .map_err(|_| {
                crate::AdapterError::Rejected("runtime agent process count is invalid".into())
            })?,
    };
    status.validate_for(&request)?;
    Ok(ResponseFrame::RuntimeStatus(status))
}
