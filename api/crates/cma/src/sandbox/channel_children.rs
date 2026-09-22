//! Selected Workspace and Agent creation runs only under the Channel allocation
//! lease. The ordinary profile workers never observe pending aggregate work.
use std::sync::Arc;

use layer2_core::bridge::allocation::{AllocationAuthority, AllocationError, AllocationRecord};
use layer2_core::sandbox::{Workspace, WorkspaceWorkStep};
use layer3_adapter::{
    SandboxAdapter, SetupReceipt, WorkspaceReceipt, WorkspaceRequest, WorkspaceSetupRequest,
};

use super::channel_provisioning::{
    state::{ChannelProvisioningPhase, ChannelProvisioningSnapshot},
    ChannelControlPlaneVerifier,
};
use crate::{bridge::BridgeControlPlaneLease, store::MongoBridgeAllocationStore};

pub(crate) mod model;
#[cfg(test)]
mod tests;

pub(crate) enum ChannelChildEvidence {
    WorkspaceAdmission,
    WorkspacePrepare {
        request: WorkspaceRequest,
        receipt: WorkspaceReceipt,
    },
    WorkspaceSetup {
        request: WorkspaceSetupRequest,
        receipt: SetupReceipt,
    },
    AgentAdmission {
        capabilities: layer2_core::RuntimeCapabilities,
    },
}

pub(crate) struct ChannelChildDriver {
    pub store: Arc<MongoBridgeAllocationStore>,
    pub credentials: Arc<dyn ChannelControlPlaneVerifier>,
    pub adapter: Arc<dyn SandboxAdapter>,
}

impl ChannelChildDriver {
    pub(crate) async fn advance(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        snapshot: &ChannelProvisioningSnapshot,
    ) -> Result<AllocationRecord, AllocationError> {
        self.require_effect(authority, lease, snapshot).await?;
        let handle = super::runtime_attachment::adopt_bound_runtime(
            self.adapter.as_ref(),
            &snapshot.sandbox,
            &snapshot.state.settings.deployment_id,
        )
        .await
        .map_err(|_| AllocationError::Unavailable)?;
        self.require_effect(authority, lease, snapshot).await?;
        let evidence = match snapshot.state.phase {
            ChannelProvisioningPhase::WorkspaceAdmission => {
                ChannelChildEvidence::WorkspaceAdmission
            }
            ChannelProvisioningPhase::WorkspacePrepare => {
                let workspace = self.store.channel_workspace(snapshot).await?;
                let request = prepare_request(snapshot, &workspace);
                self.require_effect(authority, lease, snapshot).await?;
                let receipt = self
                    .adapter
                    .prepare_workspace(&handle, &request)
                    .await
                    .map_err(|_| AllocationError::Unavailable)?;
                ChannelChildEvidence::WorkspacePrepare { request, receipt }
            }
            ChannelProvisioningPhase::WorkspaceSetup => {
                let workspace = self.store.channel_workspace(snapshot).await?;
                let request = setup_request(snapshot, &workspace);
                self.require_effect(authority, lease, snapshot).await?;
                let receipt = self
                    .adapter
                    .run_workspace_setup(&handle, &request)
                    .await
                    .map_err(|_| AllocationError::Unavailable)?;
                ChannelChildEvidence::WorkspaceSetup { request, receipt }
            }
            ChannelProvisioningPhase::AgentAdmission => ChannelChildEvidence::AgentAdmission {
                capabilities: super::runtime_capabilities(self.adapter.capabilities()),
            },
            _ => return Err(AllocationError::InvalidTransition),
        };
        // Long provider operations replay their original operation IDs after
        // authority expires. Buffered success never renews the actor's lease.
        self.credentials
            .revalidate_control_plane(lease)
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        let progressed = self
            .store
            .commit_channel_child(authority, lease, snapshot, &evidence)
            .await?;
        Ok(progressed.allocation)
    }
    async fn require_effect(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        snapshot: &ChannelProvisioningSnapshot,
    ) -> Result<(), AllocationError> {
        snapshot.validate()?;
        authority.require_record(&snapshot.allocation)?;
        self.credentials
            .revalidate_control_plane(lease)
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        if snapshot.allocation.lease.as_ref().is_none_or(|claim| {
            claim.expires_at_ms <= mongodb::bson::DateTime::now().timestamp_millis()
        }) {
            return Err(AllocationError::LeaseUnavailable);
        }
        // A still-live local proof cannot authorize new provider work after a
        // Bridge suspension, allocation reset or parent resource change. Reuse
        // the same transactional write fences as the initial provider stages.
        let current = self
            .store
            .begin_channel_provisioning(
                authority,
                lease,
                &snapshot.allocation,
                &snapshot.state.settings,
            )
            .await?;
        if current.allocation != snapshot.allocation
            || current.sandbox != snapshot.sandbox
            || current.state != snapshot.state
            || current.creation != snapshot.creation
        {
            return Err(AllocationError::Conflict);
        }
        Ok(())
    }
}

pub(crate) fn prepare_request(
    snapshot: &ChannelProvisioningSnapshot,
    workspace: &Workspace,
) -> WorkspaceRequest {
    let source = match &workspace.source {
        layer2_core::sandbox::WorkspaceSource::GitRepository { url, reference } => {
            layer3_adapter::WorkspaceSource::Clone {
                url: url.clone(),
                reference: reference.clone(),
            }
        }
        layer2_core::sandbox::WorkspaceSource::Empty => layer3_adapter::WorkspaceSource::Init,
    };
    WorkspaceRequest::with_clone_depth(
        workspace.operation_id(WorkspaceWorkStep::Prepare),
        workspace.id.to_string(),
        workspace.name.clone(),
        source,
        snapshot.state.settings.execution.clone_depth,
    )
}

pub(crate) fn setup_request(
    snapshot: &ChannelProvisioningSnapshot,
    workspace: &Workspace,
) -> WorkspaceSetupRequest {
    let execution = snapshot.state.settings.execution;
    WorkspaceSetupRequest::with_timeouts(
        workspace.operation_id(WorkspaceWorkStep::Setup),
        workspace.id.to_string(),
        workspace.name.clone(),
        workspace.profile.content.setup_commands.clone(),
        execution.command_timeout_seconds,
        execution.total_timeout_seconds,
        layer3_adapter::DEFAULT_COMMAND_OUTPUT_LIMIT_BYTES,
    )
}

pub(crate) fn valid_prepare(
    request: &WorkspaceRequest,
    receipt: &WorkspaceReceipt,
    workspace: &Workspace,
) -> bool {
    receipt.operation_id == request.operation_id
        && receipt.request_sha256 == request.request_sha256
        && receipt.workspace_id == request.workspace_id
        && receipt.name == request.name
        && receipt.path == workspace.path
        && matches!(receipt.resolved_commit.len(), 40 | 64)
        && receipt
            .resolved_commit
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
}

pub(crate) fn valid_setup(request: &WorkspaceSetupRequest, receipt: &SetupReceipt) -> bool {
    receipt.operation_id == request.operation_id
        && receipt.request_sha256 == request.request_sha256
        && receipt.workspace_id == request.workspace_id
        && receipt.commands.len() == request.commands.len()
        && receipt.commands.iter().enumerate().all(|(i, command)| {
            command.index == i as u32
                && command.exit_code == 0
                && [&command.stdout_sha256, &command.stderr_sha256]
                    .iter()
                    .all(|digest| {
                        digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit())
                    })
        })
}
