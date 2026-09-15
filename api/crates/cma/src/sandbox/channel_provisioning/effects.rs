use super::{ChannelProvisioningDriver, ChannelProvisioningPhase, ChannelProvisioningSnapshot};
use crate::bridge::BridgeControlPlaneLease;
use layer2_core::{
    bridge::allocation::{AllocationAuthority, AllocationError},
    sandbox::{RuntimeBinding, SandboxEventKind, SandboxStatus, SandboxWorkStep},
};
use layer3_adapter::{InstallRequest, RuntimeAdoptionRequest, RuntimeProvisionRequest};

impl ChannelProvisioningDriver {
    pub(super) async fn home(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        snapshot: ChannelProvisioningSnapshot,
    ) -> Result<ChannelProvisioningSnapshot, AllocationError> {
        self.require_effect(authority, lease, &snapshot).await?;
        let next = crate::sandbox::home::next_home(
            self.dependencies.adapter.as_ref(),
            &snapshot.sandbox.id,
            snapshot.sandbox.lifecycle.home.as_deref(),
            snapshot.state.settings.home_policy.as_ref(),
        )
        .await
        .map_err(|_| AllocationError::Unavailable)?;
        let mut sandbox = snapshot.sandbox.clone();
        let phase = if let Some(home) = next {
            sandbox.lifecycle.home = Some(Box::new(home));
            ChannelProvisioningPhase::Home
        } else {
            ChannelProvisioningPhase::Runtime
        };
        self.commit(
            authority,
            lease,
            &snapshot,
            sandbox.clone(),
            phase,
            &sandbox.lifecycle.home,
            None,
        )
        .await
    }

    pub(super) async fn runtime(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        snapshot: ChannelProvisioningSnapshot,
    ) -> Result<ChannelProvisioningSnapshot, AllocationError> {
        self.require_effect(authority, lease, &snapshot).await?;
        if snapshot.sandbox.runtime.is_some() {
            return Err(AllocationError::Corrupt);
        }
        let generation = 1;
        let settings = &snapshot.state.settings;
        let spec = crate::sandbox::runtime_mapping::sandbox_spec(
            &snapshot.sandbox,
            &settings.deployment_id,
        );
        let handle = self
            .dependencies
            .adapter
            .provision_runtime(&RuntimeProvisionRequest::new(generation, spec.clone()))
            .await
            .map_err(|_| AllocationError::Unavailable)?;
        let observed = self
            .dependencies
            .adapter
            .adopt_runtime(
                &RuntimeAdoptionRequest::new(generation, handle.clone(), spec.clone())
                    .with_expected_image(&settings.runtime_version),
            )
            .await
            .map_err(|_| AllocationError::Unavailable)?
            .ok_or(AllocationError::Unavailable)?;
        if observed.handle != handle
            || observed.sandbox_id != snapshot.sandbox.id.as_str()
            || observed.generation != generation
            || !observed.lifecycle.is_running()
        {
            return Err(AllocationError::IdentityMismatch);
        }
        let status = crate::sandbox::runtime_attachment::provisioned_runtime_status(
            self.dependencies.adapter.as_ref(),
            &spec,
            &handle,
            generation,
        )
        .await
        .map_err(|_| AllocationError::Unavailable)?;
        let mut sandbox = snapshot.sandbox.clone();
        sandbox.runtime = Some(RuntimeBinding {
            boot_id: status.map(|s| s.boot_id),
            runtime_id: handle.0,
            runtime_version: settings.runtime_version.clone(),
            provision_generation: Some(generation),
            bound_at: layer2_core::rfc3339_at(std::time::SystemTime::now()),
        });
        sandbox.lifecycle.runtime_generation_high_water = generation;
        sandbox.status = SandboxStatus::Installing;
        self.commit(
            authority,
            lease,
            &snapshot,
            sandbox.clone(),
            ChannelProvisioningPhase::Install,
            &sandbox.runtime,
            Some(SandboxEventKind::StatusChanged {
                from: SandboxStatus::Provisioning,
                to: SandboxStatus::Installing,
            }),
        )
        .await
    }

    pub(super) async fn install(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        snapshot: ChannelProvisioningSnapshot,
    ) -> Result<ChannelProvisioningSnapshot, AllocationError> {
        self.require_effect(authority, lease, &snapshot).await?;
        let runtime = crate::sandbox::runtime_attachment::adopt_bound_runtime(
            self.dependencies.adapter.as_ref(),
            &snapshot.sandbox,
            &snapshot.state.settings.deployment_id,
        )
        .await
        .map_err(|_| AllocationError::Unavailable)?;
        let execution = snapshot.state.settings.execution;
        let request = InstallRequest::with_timeouts(
            format!("{}:install:v1", snapshot.allocation.operation_id),
            snapshot
                .sandbox
                .profile
                .content
                .software_install_commands
                .clone(),
            execution.command_timeout_seconds,
            execution.total_timeout_seconds,
            layer3_adapter::DEFAULT_COMMAND_OUTPUT_LIMIT_BYTES,
        );
        let receipt = self
            .dependencies
            .adapter
            .run_install(&runtime, &request)
            .await
            .map_err(|_| AllocationError::Unavailable)?;
        if receipt.operation_id != request.operation_id
            || receipt.request_sha256 != request.request_sha256
            || receipt.commands.len() != request.commands.len()
            || receipt.commands.iter().enumerate().any(|(index, command)| {
                command.index as usize != index
                    || command.exit_code != 0
                    || !digest(&command.stdout_sha256)
                    || !digest(&command.stderr_sha256)
            })
        {
            return Err(AllocationError::IdentityMismatch);
        }
        self.commit(
            authority,
            lease,
            &snapshot,
            snapshot.sandbox.clone(),
            ChannelProvisioningPhase::Mint,
            &receipt,
            Some(SandboxEventKind::WorkStepCompleted {
                step: SandboxWorkStep::RunInstall,
            }),
        )
        .await
    }
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
