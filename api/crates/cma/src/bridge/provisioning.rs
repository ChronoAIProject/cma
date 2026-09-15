//! Renewable authority for the exact retained Channel allocation. Provider
//! scope planning and profile checks are effect-free; admission remains fenced
//! by the live owner quota in the allocation/Sandbox transaction.
use std::{sync::Arc, time::Duration};

use layer2_core::bridge::{
    allocation::{
        AllocationAuthority, AllocationCapabilityObservation, AllocationIdentity,
        AllocationPlacement, AllocationProfilePlan, AllocationProfiles,
        AllocationProvisioningProof, AllocationRecord,
    },
    BridgeConfig, BridgeConnectionObservation, BridgeError, BridgeService, BridgeSetupBlock,
    BridgeValidation,
};
use layer2_core::profiles::{
    AgentProfileStore, ModelAccess, ProfileCapacityBounds, SandboxProfileStore,
    WorkspaceProfileStore,
};
use layer2_core::resource_access::ResourceOwner;
use layer2_core::sandbox::SandboxPolicySource;
use layer3_adapter::SandboxAdapter;

use super::{
    custody::{BridgeCredentialLease, BridgeCredentialResolver},
    BridgeControlPlaneLease, ManagedBridgeCredentials,
};
use crate::{auth::keys::KeyIssuer, sandbox::OrnnAgentSkillResolver};

mod connection;
mod profiles;
use connection::{connection, connection_with_target};
pub(crate) mod retirement;
mod reuse;
// Stage A is deliberately disconnected from deletion and public routes.
#[allow(
    dead_code,
    reason = "cleanup request seal awaits effect and restore integration"
)]
pub(crate) mod reuse_cleanup;
pub(crate) mod reuse_revival;
pub(crate) mod revival;

pub(crate) struct BridgeProvisioningDependencies {
    pub resources: Arc<crate::store::MongoSandboxStore>,
    pub resource_access: Arc<layer2_core::resource_access::ResourceAccessService>,
    pub sandbox_profiles: Arc<dyn SandboxProfileStore>,
    pub workspace_profiles: Arc<dyn WorkspaceProfileStore>,
    pub agent_profiles: Arc<dyn AgentProfileStore>,
    pub policy: Arc<dyn SandboxPolicySource>,
    pub adapter: Arc<dyn SandboxAdapter>,
    pub issuer: Arc<KeyIssuer>,
    pub skills: Arc<OrnnAgentSkillResolver>,
    pub capacity_bounds: ProfileCapacityBounds,
}

pub(crate) struct BridgeProvisioningAuthority {
    credentials: Arc<ManagedBridgeCredentials>,
    dependencies: Arc<BridgeProvisioningDependencies>,
}

impl BridgeProvisioningAuthority {
    pub(crate) fn new(
        credentials: Arc<ManagedBridgeCredentials>,
        dependencies: Arc<BridgeProvisioningDependencies>,
    ) -> Self {
        Self {
            credentials,
            dependencies,
        }
    }

    pub(crate) async fn resolve(
        &self,
        definitions: &BridgeService,
        record: &AllocationRecord,
    ) -> Result<(AllocationAuthority, BridgeControlPlaneLease), BridgeError> {
        record.validate().map_err(allocation_error)?;
        if matches!(record.placement, AllocationPlacement::ReuseParent { .. }) {
            return self
                .prepare_reuse(definitions, &record.identity, &record.pin, Some(record))
                .await;
        }
        let AllocationPlacement::NewSandbox { profiles, target } = &record.placement else {
            return Err(profile_unavailable());
        };
        let key = self
            .credentials
            .resolve(&record.owner, &record.config)
            .await?;
        let (_, observation) = self
            .dependencies
            .preflight(
                &key.control_plane,
                &record.config,
                target.sandbox_id.as_str(),
                Some(profiles),
            )
            .await?;
        let current = self
            .credentials
            .bridges
            .bridge(&record.pin.bridge_id)
            .await?
            .ok_or(BridgeError::NotFound)?;
        let mut management = definitions
            .management_lease(
                key.control_plane.actor_context(),
                Some(key.control_plane.credential()),
                &record.pin.bridge_id,
                current.revision,
            )
            .await?;
        management.bound_deadline(key.deadline);
        let plan = AllocationProfilePlan::renew_for_record(&management, record, observation)
            .map_err(allocation_error)?;
        self.credentials.revalidate(&key).await?;
        definitions.validate_management(&management).await?;
        let connection = connection(&record.owner, &record.config, &key)?;
        let proof = AllocationProvisioningProof::new(
            management.record().clone(),
            &record.identity,
            plan,
            connection,
        )
        .map_err(allocation_error)?;
        let authority = AllocationAuthority::Provisioning(Box::new(proof));
        authority.require_record(record).map_err(allocation_error)?;
        Ok((authority, key.control_plane))
    }

    /// The partition service must still construct AllocationReservation from
    /// its checked admission. An identity alone never admits a callback.
    pub(crate) async fn prepare_accepted(
        &self,
        definitions: &BridgeService,
        identity: &AllocationIdentity,
        pin: &layer2_core::bridge::BridgeWorkPin,
    ) -> Result<(AllocationAuthority, BridgeControlPlaneLease), BridgeError> {
        if pin.bridge_id != identity.bridge_id {
            return Err(BridgeError::Conflict);
        }
        let bridge = self
            .credentials
            .bridges
            .bridge(&identity.bridge_id)
            .await?
            .ok_or(BridgeError::NotFound)?;
        let config = &bridge.pinned(pin)?.config;
        if config.placement.reused_parent().is_some() {
            return self.prepare_reuse(definitions, identity, pin, None).await;
        }
        let key = self.credentials.resolve(&bridge.owner, config).await?;
        let (profiles, observation) = self
            .dependencies
            .preflight(
                &key.control_plane,
                config,
                &identity.id().map_err(allocation_error)?,
                None,
            )
            .await?;
        let mut management = definitions
            .management_lease(
                key.control_plane.actor_context(),
                Some(key.control_plane.credential()),
                &bridge.id,
                bridge.revision,
            )
            .await?;
        management.bound_deadline(key.deadline);
        let plan = AllocationProfilePlan::prepare_for_generation(
            &management,
            pin.clone(),
            profiles.sandbox,
            profiles.workspace,
            profiles.agent,
            &profiles.trust_digest,
            observation,
        )
        .map_err(allocation_error)?;
        self.credentials.revalidate(&key).await?;
        definitions.validate_management(&management).await?;
        let proof = AllocationProvisioningProof::new(
            bridge.clone(),
            identity,
            plan,
            connection(&bridge.owner, config, &key)?,
        )
        .map_err(allocation_error)?;
        Ok((
            AllocationAuthority::Provisioning(Box::new(proof)),
            key.control_plane,
        ))
    }
}

impl BridgeProvisioningDependencies {
    pub(super) async fn preflight(
        &self,
        lease: &BridgeControlPlaneLease,
        config: &BridgeConfig,
        credential_id: &str,
        retained: Option<&AllocationProfiles>,
    ) -> Result<(AllocationProfiles, AllocationCapabilityObservation), BridgeError> {
        let started = tokio::time::Instant::now();
        let mut profiles = self
            .load_profiles(lease.owner(), config, lease.actor_context().actor.clone())
            .await?;
        if let Some(retained) = retained {
            if retained.sandbox != profiles.sandbox
                || retained.workspace != profiles.workspace
                || retained.agent != profiles.agent
                || retained.trust_digest != profiles.trust_digest
            {
                return Err(profile_unavailable());
            }
            profiles.approved_by = retained.approved_by.clone();
        }
        profiles.validate(config).map_err(allocation_error)?;
        profiles
            .sandbox
            .content
            .validate(self.capacity_bounds)
            .map_err(|_| profile_unavailable())?;
        let policy = self
            .policy
            .current_sandbox_policy()
            .await
            .map_err(|_| BridgeError::AuthorityUnavailable)?;
        let caps = self.adapter.capabilities();
        let runtime_available = caps.conversation
            && caps.per_agent_home
            && caps.steer
            && caps.interrupt
            && caps.usage
            && caps.resources;
        if !runtime_available
            || profiles.sandbox.content.model_access != ModelAccess::ChronoaiManaged
        {
            return Err(super::control_plane::reconnect());
        }
        let services = profiles
            .sandbox
            .content
            .credential_services(&policy.baseline_services);
        let model_services = profiles
            .sandbox
            .content
            .agent_provider
            .connection_services();
        if policy.max_sandboxes_per_owner == 0
            || !services
                .iter()
                .any(|s| s.as_str() == model_services.managed)
            || (!profiles.agent.content.skills.is_empty()
                && !services.iter().any(|s| s.as_str() == "ornn-api"))
        {
            return Err(profile_unavailable());
        }
        self.issuer
            .plan_bridge_sandbox_key(lease, credential_id, &services)
            .await
            .map_err(|_| super::control_plane::reconnect())?;
        // This proves the selected model service and runtime contract, not a
        // live per-model catalogue. The real Agent open still validates model
        // acceptance before the allocation can become Ready.
        self.skills
            .resolve_public_with_credential(lease.credential(), &profiles.agent.content.skills)
            .await
            .map_err(|_| profile_unavailable())?;
        let current = self
            .load_profiles(lease.owner(), config, profiles.approved_by.clone())
            .await?;
        if current != profiles {
            return Err(profile_unavailable());
        }
        let deadline = lease.deadline().min(started + Duration::from_secs(30));
        if tokio::time::Instant::now() >= deadline {
            return Err(BridgeError::AuthorityUnavailable);
        }
        let observation = AllocationCapabilityObservation {
            owner: lease.owner().clone(),
            profiles_digest: profiles.trust_digest.clone(),
            capacity_bounds: self.capacity_bounds,
            owner_live_sandbox_limit: policy.max_sandboxes_per_owner,
            baseline_services: policy.baseline_services,
            // Exact one-child shape is validated above; the live quota count
            // is serialized with ordinary Sandbox admission in Mongo.
            one_sandbox_workspace_worktree_agent_available: profiles.sandbox.content.max_workspaces
                > 0
                && profiles.sandbox.content.max_agents_per_workspace > 0,
            runtime_model_services_skills_available: runtime_available,
            deadline,
        };
        Ok((profiles, observation))
    }
}

fn profile_unavailable() -> BridgeError {
    BridgeError::ConnectionRequired(BridgeSetupBlock::ProfileUnavailable)
}

fn allocation_error(error: layer2_core::bridge::allocation::AllocationError) -> BridgeError {
    use layer2_core::bridge::allocation::AllocationError;
    match error {
        AllocationError::InvalidProfiles => profile_unavailable(),
        AllocationError::IncompatibleTarget => {
            BridgeError::ConnectionRequired(BridgeSetupBlock::IncompatibleTarget)
        }
        AllocationError::Corrupt => BridgeError::Corrupt,
        AllocationError::Conflict | AllocationError::IdentityMismatch => BridgeError::Conflict,
        _ => BridgeError::AuthorityUnavailable,
    }
}
