//! Reuse only a parent the current retained configuring actor may manage.
//! No parent credential issuance, scope expansion or global profile mutation.
use super::*;
use layer2_core::bridge::{
    allocation::{AllocationReuseObservation, AllocationReuseProof},
    BridgePlacementPolicy, BridgeWorkPin,
};
use layer2_core::resource_access::{AccessPurpose, AuthorizedResource, ResourceCapability};

impl BridgeProvisioningAuthority {
    pub(super) async fn prepare_reuse(
        &self,
        definitions: &BridgeService,
        identity: &AllocationIdentity,
        pin: &BridgeWorkPin,
        retained: Option<&AllocationRecord>,
    ) -> Result<(AllocationAuthority, BridgeControlPlaneLease), BridgeError> {
        let bridge = self
            .credentials
            .bridges
            .bridge(&identity.bridge_id)
            .await?
            .ok_or(BridgeError::NotFound)?;
        let config = &bridge.pinned(pin)?.config;
        let key = self.credentials.resolve(&bridge.owner, config).await?;
        let (profiles, authorized, observation) = self
            .dependencies
            .reuse_preflight(
                &key.control_plane,
                config,
                retained.and_then(|record| record.placement.profiles()),
            )
            .await?;
        let connection = connection_with_target(
            &bridge.owner,
            config,
            &key,
            Some(authorized.snapshot.clone()),
        )?;
        let mut management = definitions
            .management_lease(
                key.control_plane.actor_context(),
                Some(key.control_plane.credential()),
                &bridge.id,
                bridge.revision,
            )
            .await?;
        management.bound_deadline(key.deadline);
        let proof = if let Some(record) = retained {
            AllocationReuseProof::renew_for_record(
                &management,
                record,
                authorized,
                connection,
                observation,
            )
        } else {
            AllocationReuseProof::prepare(
                &management,
                identity,
                pin.clone(),
                profiles,
                authorized,
                connection,
                observation,
            )
        }
        .map_err(allocation_error)?;
        self.credentials.revalidate(&key).await?;
        definitions.validate_management(&management).await?;
        let authority = AllocationAuthority::Reuse(Box::new(proof));
        if let Some(record) = retained {
            authority.require_record(record).map_err(allocation_error)?;
        }
        Ok((authority, key.control_plane))
    }
}

impl BridgeProvisioningDependencies {
    pub(crate) async fn reuse_preflight(
        &self,
        lease: &BridgeControlPlaneLease,
        config: &BridgeConfig,
        retained: Option<&AllocationProfiles>,
    ) -> Result<
        (
            AllocationProfiles,
            AuthorizedResource,
            AllocationReuseObservation,
        ),
        BridgeError,
    > {
        let started = tokio::time::Instant::now();
        let target = config.placement.reused_parent().ok_or_else(incompatible)?;
        let authorized = self
            .resource_access
            .authorize(
                lease.actor_context(),
                Some(lease.credential()),
                &target,
                ResourceCapability::ManageSandbox,
                AccessPurpose::Mutation {
                    started_at: std::time::SystemTime::now(),
                },
            )
            .await
            .map_err(|_| incompatible())?;
        let scope = authorized
            .snapshot
            .scope(&target)
            .map_err(|_| incompatible())?;
        if &scope.owner != lease.owner() {
            return Err(incompatible());
        }
        let (workspace_pin, agent_pin) = match &config.placement {
            BridgePlacementPolicy::WorkspacePerPartition {
                workspace_profile,
                agent_profile,
                ..
            } => (Some(workspace_profile), agent_profile),
            BridgePlacementPolicy::WorktreeAgentPerPartition { agent_profile, .. } => {
                (None, agent_profile)
            }
            _ => return Err(incompatible()),
        };
        let agent = self
            .agent_profiles
            .agent_profile(&agent_pin.id)
            .await
            .map_err(profiles::store_error)?
            .ok_or_else(profile_unavailable)?;
        if !profiles::visible(
            lease.owner(),
            &agent.owner,
            agent.visibility,
            agent.retired_at.as_deref(),
        ) {
            return Err(profile_unavailable());
        }
        let agent = self
            .agent_profiles
            .agent_profile_revision(&agent_pin.id, agent_pin.revision)
            .await
            .map_err(profiles::store_error)?
            .ok_or_else(profile_unavailable)?;
        let agent = (&agent).into();
        let workspace = if let Some(pin) = workspace_pin {
            let definition = self
                .workspace_profiles
                .workspace_profile(&pin.id)
                .await
                .map_err(profiles::store_error)?
                .ok_or_else(profile_unavailable)?;
            if !profiles::visible(
                lease.owner(),
                &definition.owner,
                definition.visibility,
                definition.retired_at.as_deref(),
            ) {
                return Err(profile_unavailable());
            }
            let revision = self
                .workspace_profiles
                .workspace_profile_revision(&pin.id, pin.revision)
                .await
                .map_err(profiles::store_error)?
                .ok_or_else(profile_unavailable)?;
            (&revision).into()
        } else {
            authorized
                .snapshot
                .lineage
                .workspace
                .as_ref()
                .ok_or_else(incompatible)?
                .profile
                .clone()
        };
        let sandbox = &authorized.snapshot.lineage.sandbox;
        let profiles = AllocationProfiles {
            sandbox: sandbox.profile.clone(),
            workspace,
            agent,
            approved_by: retained.map_or_else(
                || lease.actor_context().actor.clone(),
                |original| original.approved_by.clone(),
            ),
            trust_digest: String::new(),
        };
        let mut profiles = profiles;
        profiles.trust_digest =
            AllocationProfiles::digest(&profiles.sandbox, &profiles.workspace, &profiles.agent)
                .map_err(allocation_error)?;
        profiles.validate(config).map_err(allocation_error)?;
        if retained.is_some_and(|old| old != &profiles) {
            return Err(incompatible());
        }
        let caps = self.adapter.capabilities();
        let services = sandbox.credential.effective_reaches();
        let model_service = sandbox
            .profile
            .content
            .agent_provider
            .connection_services()
            .managed;
        if !caps.conversation
            || !caps.per_agent_home
            || !caps.steer
            || !caps.interrupt
            || !caps.usage
            || !caps.resources
            || sandbox.profile.content.model_access != ModelAccess::ChronoaiManaged
            || !services
                .iter()
                .any(|service| service.as_str() == model_service)
            || (!profiles.agent.content.skills.is_empty()
                && !services
                    .iter()
                    .any(|service| service.as_str() == "ornn-api"))
            || matches!(
                sandbox.status,
                layer2_core::sandbox::SandboxStatus::Retiring
                    | layer2_core::sandbox::SandboxStatus::Retired
            )
        {
            return Err(incompatible());
        }
        self.skills
            .resolve_public_with_credential(lease.credential(), &profiles.agent.content.skills)
            .await
            .map_err(|_| incompatible())?;
        // Re-obtain the whole parent decision after profile/provider reads.
        let current = self
            .resource_access
            .authorize(
                lease.actor_context(),
                Some(lease.credential()),
                &target,
                ResourceCapability::ManageSandbox,
                AccessPurpose::Mutation {
                    started_at: std::time::SystemTime::now(),
                },
            )
            .await
            .map_err(|_| incompatible())?;
        if current.snapshot.fence() != authorized.snapshot.fence()
            || current.snapshot.lineage != authorized.snapshot.lineage
        {
            return Err(BridgeError::Conflict);
        }
        self.resources
            .require_ordinary_reuse_parent(&current.snapshot.lineage.sandbox)
            .await
            .map_err(allocation_error)?;
        let observation = AllocationReuseObservation {
            ordinary_parent_lifecycle_available: true,
            profiles_digest: profiles.trust_digest.clone(),
            runtime_model_services_skills_available: true,
            // Pinned supervisor.rs dispatch and workspace_runtime::dispatch both
            // hold the same operation_gate for repository mutations. Agent
            // turns continue on their existing independent runtime bridges.
            workspace_writer_coordination_available: true,
            deadline: lease.deadline().min(started + Duration::from_secs(15)),
        };
        Ok((profiles, current, observation))
    }
}

fn incompatible() -> BridgeError {
    BridgeError::ConnectionRequired(BridgeSetupBlock::IncompatibleTarget)
}
