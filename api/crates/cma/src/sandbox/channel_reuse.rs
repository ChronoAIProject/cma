//! Shared-parent allocation driver. Parent provenance, credential scope, install
//! commands and lifecycle stay under their existing ordinary resource workers.
pub(crate) mod children;
pub(crate) mod requests;
pub(crate) mod startup;
pub(crate) mod startup_model;
pub(crate) mod state;
use super::{
    channel_children::ChannelChildEvidence,
    channel_provisioning::{
        state::{ChannelProvisioningPhase, ChannelProvisioningSettings},
        ChannelControlPlaneVerifier,
    },
};
use crate::{bridge::BridgeControlPlaneLease, store::MongoBridgeAllocationStore};
use layer2_core::bridge::allocation::{
    AllocationAuthority, AllocationError, AllocationMutation, AllocationProgress, AllocationRecord,
    AllocationStore,
};
use layer2_core::sandbox::SandboxStatus;
use layer3_adapter::SandboxAdapter;
use state::ChannelReuseSnapshot;
use std::sync::Arc;

pub(crate) struct ChannelReuseDriver {
    pub allocations: Arc<MongoBridgeAllocationStore>,
    pub credentials: Arc<dyn ChannelControlPlaneVerifier>,
    pub adapter: Arc<dyn SandboxAdapter>,
    pub settings: ChannelProvisioningSettings,
    pub holder: String,
    pub startup: startup::ChannelReuseStartupDriver,
}
impl ChannelReuseDriver {
    pub(crate) async fn run_one(
        &self,
        allocation: AllocationRecord,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
    ) -> Result<AllocationRecord, AllocationError> {
        authority.require_record(&allocation)?;
        self.credentials
            .revalidate_control_plane(lease)
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        let mut allocation = self
            .allocations
            .mutate(AllocationMutation::claim(
                allocation,
                authority.clone(),
                &self.holder,
                now_ms(),
                60_000,
            )?)
            .await?;
        if !allocation.effect_started {
            let claim = allocation
                .lease
                .clone()
                .ok_or(AllocationError::LeaseUnavailable)?;
            allocation = self
                .allocations
                .mutate(AllocationMutation::progress(
                    allocation,
                    authority.clone(),
                    &claim,
                    AllocationProgress::Start,
                    now_ms(),
                )?)
                .await?;
        }
        let AllocationAuthority::Reuse(proof) = authority else {
            return Err(AllocationError::IdentityMismatch);
        };
        if matches!(
            proof.resources().lineage.sandbox.status,
            SandboxStatus::Paused | SandboxStatus::Reviving
        ) {
            self.allocations
                .revive_reuse_parent(authority, lease, &allocation)
                .await?;
            return Ok(allocation);
        }
        let snapshot = self
            .allocations
            .begin_channel_reuse(authority, lease, &allocation, &self.settings)
            .await?;
        if snapshot.state.phase == ChannelProvisioningPhase::AgentStart {
            return self.startup.advance(authority, lease, &snapshot).await;
        }
        self.require_effect(authority, lease, &snapshot).await?;
        let handle = super::runtime_attachment::adopt_bound_runtime(
            self.adapter.as_ref(),
            &snapshot.sandbox,
            &snapshot.state.settings.deployment_id,
        )
        .await
        .map_err(|_| AllocationError::Unavailable)?;
        self.require_effect(authority, lease, &snapshot).await?;
        let evidence = match snapshot.state.phase {
            ChannelProvisioningPhase::WorkspaceAdmission => {
                ChannelChildEvidence::WorkspaceAdmission
            }
            ChannelProvisioningPhase::WorkspacePrepare => {
                let workspace = snapshot
                    .workspace
                    .as_ref()
                    .ok_or(AllocationError::Corrupt)?;
                let request = requests::prepare(&snapshot, workspace);
                self.require_effect(authority, lease, &snapshot).await?;
                let receipt = self
                    .adapter
                    .prepare_workspace(&handle, &request)
                    .await
                    .map_err(|_| AllocationError::Unavailable)?;
                ChannelChildEvidence::WorkspacePrepare { request, receipt }
            }
            ChannelProvisioningPhase::WorkspaceSetup => {
                let workspace = snapshot
                    .workspace
                    .as_ref()
                    .ok_or(AllocationError::Corrupt)?;
                let request = requests::setup(&snapshot, workspace);
                self.require_effect(authority, lease, &snapshot).await?;
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
        self.credentials
            .revalidate_control_plane(lease)
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        Ok(self
            .allocations
            .commit_reuse_child(authority, lease, &snapshot, &evidence)
            .await?
            .allocation)
    }
    async fn require_effect(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        snapshot: &ChannelReuseSnapshot,
    ) -> Result<(), AllocationError> {
        snapshot.require(authority)?;
        self.credentials
            .revalidate_control_plane(lease)
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        let current = self
            .allocations
            .begin_channel_reuse(
                authority,
                lease,
                &snapshot.allocation,
                &snapshot.state.settings,
            )
            .await?;
        if &current != snapshot {
            return Err(AllocationError::Conflict);
        }
        Ok(())
    }
}
fn now_ms() -> i64 {
    mongodb::bson::DateTime::now().timestamp_millis()
}
