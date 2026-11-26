//! Credential-free native preparation. Persist every returned state before its next effect.

use layer2_core::profiles::{AgentProvider, ModelAccess, WorkspaceLocation};
use layer2_core::sandbox::{RuntimeBinding, SandboxHome, WarmSandbox, WarmSandboxState};
use layer3_adapter::{
    AdapterError, HomeVolumePolicy, InstallRequest, RuntimeAdoptionRequest, RuntimeHandle,
    RuntimeProvisionRequest, RuntimeStatusRequest, SandboxAdapter, SandboxSpec,
};

mod completion;
mod hierarchy;
mod refill;
mod worker;
use crate::store::native_prepared::NativePreparedWorkspaceProof;
pub(super) use completion::complete_step;
pub(crate) use worker::NativePreparedWorker;

/// The durable caller owns target bounds, leases, retries, and publication.
/// This engine never allocates an owner credential or dispatches a prompt.
#[cfg(test)]
pub(crate) async fn prepare_step(
    adapter: &dyn SandboxAdapter,
    deployment_id: &str,
    record: &WarmSandbox,
    home_policy: &HomeVolumePolicy,
) -> layer3_adapter::Result<WarmSandbox> {
    prepare_step_with_proof(adapter, deployment_id, record, home_policy, None)
        .await
        .map(|(next, _)| next)
}

pub(crate) async fn prepare_step_with_proof(
    adapter: &dyn SandboxAdapter,
    deployment_id: &str,
    record: &WarmSandbox,
    home_policy: &HomeVolumePolicy,
    mut workspace_proof: Option<NativePreparedWorkspaceProof>,
) -> layer3_adapter::Result<(WarmSandbox, Option<NativePreparedWorkspaceProof>)> {
    if let Some(proof) = &workspace_proof {
        proof
            .validate(record)
            .map_err(|e| rejected(&e.to_string()))?;
    }
    let next = prepare_step_inner(
        adapter,
        deployment_id,
        record,
        home_policy,
        &mut workspace_proof,
    )
    .await?;
    if next.state == WarmSandboxState::Retiring {
        workspace_proof = None;
    }
    if let Some(proof) = &workspace_proof {
        proof
            .validate(&next)
            .map_err(|e| rejected(&e.to_string()))?;
    }
    Ok((next, workspace_proof))
}

async fn prepare_step_inner(
    adapter: &dyn SandboxAdapter,
    deployment_id: &str,
    record: &WarmSandbox,
    home_policy: &HomeVolumePolicy,
    workspace_proof: &mut Option<NativePreparedWorkspaceProof>,
) -> layer3_adapter::Result<WarmSandbox> {
    validate_plan(record)?;
    if !matches!(
        record.state,
        WarmSandboxState::Preparing | WarmSandboxState::Installing | WarmSandboxState::Ready
    ) {
        return Err(rejected(
            "native preparation no longer owns this reservation",
        ));
    }
    let mut next = record.clone();
    let mut spec = spec(record, deployment_id);
    if record.state == WarmSandboxState::Preparing {
        let home = match record.home.as_deref() {
            None => {
                let home = SandboxHome::reserve(&record.id, home_policy.clone())
                    .map_err(|e| rejected(&e.to_string()))?;
                // Prepared work was admitted against its durable plan's release.
                let request = RuntimeProvisionRequest::new(1, spec).with_released(
                    layer3_adapter::ReleasedRuntime {
                        revision: 0,
                        agent: record.plan.runtime_version.clone(),
                        machine: None,
                        lane_revisions: None,
                    },
                );
                let home = super::home_native::reserve(adapter, &request, home).await?;
                if home.native_pool.is_none() {
                    return Err(rejected(
                        "prepared capacity requires a native pool reservation",
                    ));
                }
                next.home = Some(Box::new(home));
                return Ok(next);
            }
            Some(home) => home,
        };
        if home.native_pool.is_none() {
            return Err(rejected("prepared native home authority changed"));
        }
        if let Some(home) =
            super::home::next_home(adapter, &record.id, Some(home), Some(home_policy)).await?
        {
            next.home = Some(Box::new(home));
            return Ok(next);
        }
        spec.home_volume = record
            .home
            .as_ref()
            .and_then(|home| home.attachment())
            .map(Box::new);
        let handle = adapter
            .provision_runtime(&RuntimeProvisionRequest::new(1, spec.clone()))
            .await?;
        let status =
            super::runtime_attachment::provisioned_runtime_status(adapter, &spec, &handle, 1)
                .await?
                .ok_or_else(|| rejected("prepared native process identity is missing"))?;
        if status.credential_ready || status.restore_pending || status.agent_processes != 0 {
            return Err(rejected(
                "new prepared native runtime already contains owner state",
            ));
        }
        next.runtime = Some(RuntimeBinding {
            boot_id: Some(status.boot_id),
            runtime_id: handle.0,
            runtime_version: record.plan.runtime_version.clone(),
            provision_generation: Some(1),
            bound_at: layer2_core::now_rfc3339(),
        });
        next.state = WarmSandboxState::Installing;
        return Ok(next);
    }

    let Some(handle) = verify_runtime(adapter, record, &spec).await? else {
        next.state = WarmSandboxState::Retiring;
        return Ok(next);
    };
    if record.state == WarmSandboxState::Ready {
        hierarchy::verify_session(adapter, record, &handle).await?;
        return Ok(next);
    }
    if record.installed_request_sha256.is_none() {
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
            || receipt
                .commands
                .iter()
                .enumerate()
                .any(|(index, command)| command.index as usize != index || command.exit_code != 0)
        {
            return Err(rejected(
                "prepared installation receipt does not prove completion",
            ));
        }
        next.installed_request_sha256 = Some(receipt.request_sha256);
        return Ok(next);
    }
    hierarchy::prepare_step(adapter, &mut next, &handle, workspace_proof).await?;
    Ok(next)
}

/// Native Claim/member cleanup is persisted before releasing its exact home.
pub(crate) async fn retire_step(
    adapter: &dyn SandboxAdapter,
    record: &WarmSandbox,
) -> layer3_adapter::Result<WarmSandbox> {
    record.validate().map_err(|e| rejected(&e.to_string()))?;
    if record.state == WarmSandboxState::Retired {
        return Ok(record.clone());
    }
    if record.state != WarmSandboxState::Retiring {
        return Err(rejected(
            "native prepared cleanup requires durable retirement",
        ));
    }
    let mut next = record.clone();
    if let Some(home) = record.home.as_deref() {
        if home.native_pool.is_none() {
            return Err(rejected(
                "prepared cleanup requires exact native home authority",
            ));
        }
        if let Some(home) = super::home_native::next_cleanup(adapter, home).await? {
            next.home = Some(Box::new(home));
            return Ok(next);
        }
        let home = next.home.as_mut().expect("retained exact home");
        super::home::release_home(adapter, home).await?;
    }
    next.state = WarmSandboxState::Retired;
    Ok(next)
}

fn spec(record: &WarmSandbox, deployment_id: &str) -> SandboxSpec {
    let mut spec = super::runtime_mapping::environment_spec(
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
    // Admission retains None in the default profile but pins the selected
    // native pool image into sandbox_spec. Preparation must render the same image.
    if spec.runtime_image.is_none() {
        spec.runtime_image = record
            .home
            .as_ref()
            .and_then(|home| home.native_pool.as_ref())
            .map(|native| native.request.pool.spec.image.uri.clone());
    }
    spec
}

async fn verify_runtime(
    adapter: &dyn SandboxAdapter,
    record: &WarmSandbox,
    spec: &SandboxSpec,
) -> layer3_adapter::Result<Option<RuntimeHandle>> {
    let runtime = record
        .runtime
        .as_ref()
        .ok_or_else(|| rejected("prepared runtime missing"))?;
    let handle = RuntimeHandle::new(&runtime.runtime_id);
    let request = RuntimeAdoptionRequest::new(1, handle.clone(), spec.clone())
        .with_expected_image(&record.plan.runtime_version);
    let Some(inspected) = adapter.inspect_runtime(&request).await? else {
        return Ok(None);
    };
    let observed = &inspected.observation;
    if observed.handle != handle
        || observed.sandbox_id != record.id.as_str()
        || observed.generation != 1
        || !observed.lifecycle.is_running()
    {
        return Err(rejected("prepared runtime observation changed"));
    }
    let status = inspected
        .status
        .ok_or_else(|| rejected("prepared runtime process identity missing"))?;
    let home = spec
        .home_volume
        .as_ref()
        .ok_or_else(|| rejected("prepared runtime process identity missing"))?;
    status.validate_for(&RuntimeStatusRequest {
        sandbox_id: record.id.to_string(),
        generation: 1,
        home_identity: home.identity()?,
    })?;
    let agents = record.workspaces[0]
        .agents
        .iter()
        .filter(|agent| agent.conversation_id.is_some())
        .count();
    if runtime.boot_id.as_ref() != Some(&status.boot_id)
        || status.credential_ready
        || status.restore_pending
        || (record.installed_request_sha256.is_some() && !status.installation_ready)
        || usize::from(status.agent_processes) != agents
    {
        return Ok(None);
    }
    if record.state == WarmSandboxState::Installing && record.workspaces[0].is_ready() {
        tracing::info!(
            sandbox_id = %record.id,
            boot_id = %status.boot_id,
            installation_ready = status.installation_ready,
            credential_ready = status.credential_ready,
            restore_pending = status.restore_pending,
            agent_processes = status.agent_processes,
            conversation_id = record.workspaces[0].agents[0].conversation_id.as_deref(),
            "native prepared hierarchy verified"
        );
    }
    Ok(Some(handle))
}

fn validate_plan(record: &WarmSandbox) -> layer3_adapter::Result<()> {
    record.validate().map_err(|e| rejected(&e.to_string()))?;
    let sandbox = &record.plan.profile.content;
    let [workspace] = record.plan.workspaces.as_slice() else {
        return Err(rejected("native prepared plan requires one workspace"));
    };
    let [agent] = workspace.agents.as_slice() else {
        return Err(rejected("native prepared plan requires one Agent"));
    };
    if sandbox.agent_provider != AgentProvider::Codex
        || sandbox.model_access != ModelAccess::ChronoaiManaged
        || !sandbox.software_install_commands.is_empty()
        || !valid_environment(sandbox)
        || !workspace.profile.content.setup_commands.is_empty()
        || !matches!(&workspace.profile.content.location, WorkspaceLocation::Directory { name } if name == "workspace")
        || workspace.profile.content.workspace_name().ok().as_deref() != Some("workspace")
        || !agent.content.skills.is_empty()
        || !crate::agent_lifecycle::guidance::is_prepared_prompt(&agent.content.system_prompt)
    {
        return Err(rejected(
            "native prepared plan is outside managed empty-workspace defaults",
        ));
    }
    Ok(())
}

pub(crate) fn valid_environment(profile: &layer2_core::profiles::SandboxProfileContent) -> bool {
    match profile.environment_variables.as_slice() {
        [] => true,
        [entry] => {
            entry.name == "CMA_MACHINE_INCLUDED"
                && entry.value == "1"
                && profile.runtime_image.is_some()
        }
        _ => false,
    }
}

fn rejected(message: &str) -> AdapterError {
    AdapterError::Rejected(message.into())
}
