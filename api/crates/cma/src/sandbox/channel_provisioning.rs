//! Dedicated Channel provisioning. Legacy aggregate workers see no pending work.
mod credentials;
mod effects;
pub(crate) mod state;

use crate::{
    auth::keys::KeyIssuer,
    bridge::{BridgeControlPlaneLease, ManagedBridgeCredentials},
    store::{MongoBridgeAllocationStore, MongoSandboxCredentialStore},
};
use layer2_core::bridge::allocation::{
    AllocationAuthority, AllocationError, AllocationMutation, AllocationProgress, AllocationRecord,
    AllocationStore, ChannelSandboxAdmissionStore, ChannelSandboxReservation,
};
use layer3_adapter::SandboxAdapter;
use state::{ChannelProvisioningPhase, ChannelProvisioningSettings, ChannelProvisioningSnapshot};
use std::sync::Arc;

#[async_trait::async_trait]
pub(crate) trait ChannelControlPlaneVerifier: Send + Sync {
    async fn revalidate_control_plane(
        &self,
        lease: &BridgeControlPlaneLease,
    ) -> Result<(), layer2_core::bridge::BridgeError>;
}

#[async_trait::async_trait]
impl ChannelControlPlaneVerifier for ManagedBridgeCredentials {
    async fn revalidate_control_plane(
        &self,
        lease: &BridgeControlPlaneLease,
    ) -> Result<(), layer2_core::bridge::BridgeError> {
        ManagedBridgeCredentials::revalidate_control_plane(self, lease).await
    }
}

pub(crate) struct ChannelProvisioningDependencies {
    pub allocations: Arc<MongoBridgeAllocationStore>,
    pub credentials: Arc<dyn ChannelControlPlaneVerifier>,
    pub issuer: Arc<KeyIssuer>,
    pub custody: Arc<MongoSandboxCredentialStore>,
    pub adapter: Arc<dyn SandboxAdapter>,
}

pub(crate) struct ChannelProvisioningDriver {
    dependencies: ChannelProvisioningDependencies,
    settings: ChannelProvisioningSettings,
    holder: String,
}

pub(crate) enum ChannelProvisioningOutcome {
    Progressed(AllocationRecord),
    /// The downstream creator receives the still-current exact allocation claim.
    Children(Box<ChannelProvisioningSnapshot>),
    /// A non-idempotent key POST may have escaped. A new POST is never inferred.
    MintOutcomeUnknown(AllocationRecord),
}

impl ChannelProvisioningDriver {
    pub(crate) fn new(
        dependencies: ChannelProvisioningDependencies,
        settings: ChannelProvisioningSettings,
        holder: String,
    ) -> Result<Self, AllocationError> {
        settings.validate()?;
        Ok(Self {
            dependencies,
            settings,
            holder,
        })
    }

    pub(crate) async fn run_one(
        &self,
        mut allocation: AllocationRecord,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
    ) -> Result<ChannelProvisioningOutcome, AllocationError> {
        authority.require_record(&allocation)?;
        self.dependencies
            .credentials
            .revalidate_control_plane(lease)
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        if !allocation.effect_started {
            allocation = self
                .dependencies
                .allocations
                .admit_channel_sandbox(ChannelSandboxReservation::prepare(
                    authority.clone(),
                    allocation,
                )?)
                .await?
                .allocation;
        }
        let claim = AllocationMutation::claim(
            allocation,
            authority.clone(),
            &self.holder,
            now_ms()?,
            60_000,
        )?;
        let claimed = self.dependencies.allocations.mutate(claim).await?;
        let snapshot = self
            .dependencies
            .allocations
            .begin_channel_provisioning(authority, lease, &claimed, &self.settings)
            .await?;
        use ChannelProvisioningPhase::*;
        let progressed = match snapshot.state.phase {
            Home => self.home(authority, lease, snapshot).await?,
            Runtime => self.runtime(authority, lease, snapshot).await?,
            Install => self.install(authority, lease, snapshot).await?,
            Mint => self.mint(authority, lease, snapshot).await?,
            Deliver => self.deliver(authority, lease, snapshot).await?,
            MintStarted => {
                // The provider has no key-create idempotency contract. Missing
                // local custody cannot establish that the POST did not run.
                let claim = snapshot
                    .allocation
                    .lease
                    .clone()
                    .ok_or(AllocationError::LeaseUnavailable)?;
                let record = self
                    .dependencies
                    .allocations
                    .mutate(AllocationMutation::progress(
                        snapshot.allocation,
                        authority.clone(),
                        &claim,
                        AllocationProgress::NeedsConnection,
                        now_ms()?,
                    )?)
                    .await?;
                return Ok(ChannelProvisioningOutcome::MintOutcomeUnknown(record));
            }
            WorkspaceAdmission | WorkspacePrepare | WorkspaceSetup | AgentAdmission
            | AgentStart => return Ok(ChannelProvisioningOutcome::Children(Box::new(snapshot))),
            Ready => return Err(AllocationError::InvalidTransition),
        };
        Ok(ChannelProvisioningOutcome::Progressed(
            progressed.allocation,
        ))
    }

    async fn require_effect(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        snapshot: &ChannelProvisioningSnapshot,
    ) -> Result<(), AllocationError> {
        snapshot.validate()?;
        authority.require_record(&snapshot.allocation)?;
        self.dependencies
            .credentials
            .revalidate_control_plane(lease)
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        // Read/write fences run immediately before each new external operation.
        let current = self
            .dependencies
            .allocations
            .begin_channel_provisioning(
                authority,
                lease,
                &snapshot.allocation,
                &snapshot.state.settings,
            )
            .await?;
        if current.sandbox != snapshot.sandbox
            || current.state != snapshot.state
            || current.creation != snapshot.creation
        {
            return Err(AllocationError::Conflict);
        }
        Ok(())
    }

    // Keep the independently checked authorities and persisted evidence explicit.
    #[allow(clippy::too_many_arguments)]
    async fn commit(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        previous: &ChannelProvisioningSnapshot,
        mut sandbox: layer2_core::sandbox::Sandbox,
        phase: ChannelProvisioningPhase,
        receipt: &impl serde::Serialize,
        event: Option<layer2_core::sandbox::SandboxEventKind>,
    ) -> Result<ChannelProvisioningSnapshot, AllocationError> {
        self.dependencies
            .credentials
            .revalidate_control_plane(lease)
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        sandbox.revision = previous
            .sandbox
            .revision
            .checked_add(1)
            .ok_or(AllocationError::Corrupt)?;
        sandbox.updated_at = layer2_core::rfc3339_at(std::time::SystemTime::now());
        let state = previous.state.next(phase, receipt)?;
        self.dependencies
            .allocations
            .commit_channel_provisioning(authority, lease, previous, sandbox, state, event)
            .await
    }
}

fn now_ms() -> Result<i64, AllocationError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|v| i64::try_from(v.as_millis()).ok())
        .ok_or(AllocationError::Unavailable)
}
