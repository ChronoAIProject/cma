//! Historical preparation fixtures; production only retires legacy warm records.
use crate::sandbox::runtime_mapping::environment_spec;
use layer2_core::now_rfc3339;
use layer2_core::sandbox::{RuntimeBinding, WarmSandbox, WarmSandboxState};
use layer3_adapter::{InstallRequest, RuntimeAdoptionRequest, RuntimeHandle, SandboxAdapter};

pub(in crate::sandbox) async fn prepare_environment(
    adapter: &dyn SandboxAdapter,
    deployment_id: &str,
    record: &WarmSandbox,
    home_policy: Option<&layer3_adapter::HomeVolumePolicy>,
) -> Result<WarmSandbox, layer3_adapter::AdapterError> {
    let mut next = record.clone();
    let mut spec = environment_spec(
        &record.id,
        &record.plan.profile,
        &record.plan.reaches,
        deployment_id,
    );
    spec.home_volume = record
        .home
        .as_ref()
        .and_then(|home| home.attachment())
        .map(Box::new);
    match record.state {
        WarmSandboxState::Preparing => {
            if let Some(home) = crate::sandbox::home::next_home(
                adapter,
                &record.id,
                record.home.as_deref(),
                home_policy,
            )
            .await?
            {
                next.home = Some(Box::new(home));
                return Ok(next);
            }
            let handle = adapter.provision(&spec).await?;
            let boot_id = crate::sandbox::runtime_attachment::provisioned_runtime_status(
                adapter, &spec, &handle, 1,
            )
            .await?
            .map(|status| status.boot_id);
            next.runtime = Some(RuntimeBinding {
                boot_id,
                runtime_id: handle.0,
                runtime_version: record.plan.runtime_version.clone(),
                provision_generation: Some(1),
                bound_at: now_rfc3339(),
            });
            next.state = WarmSandboxState::Installing;
        }
        WarmSandboxState::Installing | WarmSandboxState::Ready => {
            let runtime = record.runtime.as_ref().ok_or_else(|| {
                layer3_adapter::AdapterError::Rejected("warm runtime binding missing".into())
            })?;
            let handle = RuntimeHandle::new(&runtime.runtime_id);
            let adoption = RuntimeAdoptionRequest::new(1, handle.clone(), spec.clone())
                .with_expected_image(&record.plan.runtime_version);
            if adapter.adopt_runtime(&adoption).await?.is_none() {
                next.state = WarmSandboxState::Retiring;
                return Ok(next);
            }
            if let Some(status) = crate::sandbox::runtime_attachment::provisioned_runtime_status(
                adapter, &spec, &handle, 1,
            )
            .await?
            {
                if runtime.boot_id.as_ref() != Some(&status.boot_id)
                    || status.credential_ready
                    || usize::from(status.agent_processes)
                        != record
                            .workspaces
                            .iter()
                            .flat_map(|workspace| &workspace.agents)
                            .filter(|agent| agent.conversation_id.is_some())
                            .count()
                    || status.restore_pending
                {
                    next.state = WarmSandboxState::Retiring;
                    return Ok(next);
                }
            }
            let execution = record.plan.execution;
            let request = InstallRequest::with_timeouts(
                format!("sandbox-install-v1:{}", record.id),
                record
                    .plan
                    .profile
                    .content
                    .software_install_commands
                    .clone(),
                execution.command_timeout_seconds,
                execution.total_timeout_seconds,
                layer3_adapter::DEFAULT_COMMAND_OUTPUT_LIMIT_BYTES,
            );
            let receipt = adapter.run_install(&handle, &request).await?;
            if receipt.operation_id != request.operation_id
                || receipt.request_sha256 != request.request_sha256
                || receipt.commands.len() != request.commands.len()
                || receipt.commands.iter().enumerate().any(|(index, command)| {
                    command.index as usize != index || command.exit_code != 0
                })
            {
                return Err(layer3_adapter::AdapterError::Rejected(
                    "warm installation receipt does not prove completion".into(),
                ));
            }
            next.installed_request_sha256 = Some(receipt.request_sha256);
            if record.workspaces.is_empty() {
                next.state = WarmSandboxState::Ready;
            }
            if next.state == WarmSandboxState::Ready {
                super::hierarchy::verify_sessions(adapter, &next).await?;
            }
        }
        WarmSandboxState::Retiring => return super::retire_environment(adapter, record).await,
        WarmSandboxState::Claimed | WarmSandboxState::Retired => (),
    }
    Ok(next)
}
