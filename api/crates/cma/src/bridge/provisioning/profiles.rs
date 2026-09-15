use super::*;
use layer2_core::bridge::BridgePlacementPolicy;
use layer2_core::profiles::{ProfileStoreError, ProfileVisibility};
use layer2_core::resource_access::ResourceActor;
use layer2_core::SubjectId;

impl BridgeProvisioningDependencies {
    pub(super) async fn load_profiles(
        &self,
        owner: &ResourceOwner,
        config: &BridgeConfig,
        approved_by: ResourceActor,
    ) -> Result<AllocationProfiles, BridgeError> {
        let BridgePlacementPolicy::NewSandbox {
            sandbox_profile,
            workspace_profile,
            agent_profile,
        } = &config.placement
        else {
            return Err(profile_unavailable());
        };
        let (sandbox, workspace, agent) = tokio::try_join!(
            self.sandbox_profiles.sandbox_profile(&sandbox_profile.id),
            self.workspace_profiles
                .workspace_profile(&workspace_profile.id),
            self.agent_profiles.agent_profile(&agent_profile.id),
        )
        .map_err(store_error)?;
        let sandbox = sandbox.ok_or_else(profile_unavailable)?;
        let workspace = workspace.ok_or_else(profile_unavailable)?;
        let agent = agent.ok_or_else(profile_unavailable)?;
        if sandbox.id != sandbox_profile.id
            || workspace.id != workspace_profile.id
            || agent.id != agent_profile.id
            || !visible(
                owner,
                &sandbox.owner,
                sandbox.visibility,
                sandbox.retired_at.as_deref(),
            )
            || !visible(
                owner,
                &workspace.owner,
                workspace.visibility,
                workspace.retired_at.as_deref(),
            )
            || !visible(
                owner,
                &agent.owner,
                agent.visibility,
                agent.retired_at.as_deref(),
            )
        {
            return Err(profile_unavailable());
        }
        let (sandbox, workspace, agent) = tokio::try_join!(
            self.sandbox_profiles
                .sandbox_profile_revision(&sandbox_profile.id, sandbox_profile.revision),
            self.workspace_profiles
                .workspace_profile_revision(&workspace_profile.id, workspace_profile.revision),
            self.agent_profiles
                .agent_profile_revision(&agent_profile.id, agent_profile.revision),
        )
        .map_err(store_error)?;
        let sandbox = (&sandbox.ok_or_else(profile_unavailable)?).into();
        let workspace = (&workspace.ok_or_else(profile_unavailable)?).into();
        let agent = (&agent.ok_or_else(profile_unavailable)?).into();
        let profiles = AllocationProfiles {
            trust_digest: AllocationProfiles::digest(&sandbox, &workspace, &agent)
                .map_err(allocation_error)?,
            sandbox,
            workspace,
            agent,
            approved_by,
        };
        profiles.validate(config).map_err(allocation_error)?;
        Ok(profiles)
    }
}

fn visible(
    owner: &ResourceOwner,
    publisher: &SubjectId,
    visibility: ProfileVisibility,
    retired: Option<&str>,
) -> bool {
    retired.is_none()
        && (visibility == ProfileVisibility::Public
            || publisher == super::super::types::owner_subject(owner))
}

fn store_error(error: ProfileStoreError) -> BridgeError {
    match error {
        ProfileStoreError::Unavailable(_) => BridgeError::AuthorityUnavailable,
        _ => profile_unavailable(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn organization_profiles_never_borrow_configuring_admin_private_templates() {
        let org = ResourceOwner::Organization {
            subject: SubjectId::new("org"),
        };
        let admin = SubjectId::new("admin");
        assert!(!visible(&org, &admin, ProfileVisibility::Private, None));
        assert!(visible(&org, &admin, ProfileVisibility::Public, None));
        assert!(!visible(
            &org,
            &admin,
            ProfileVisibility::Public,
            Some("2026-09-15T10:00:00Z")
        ));
        assert!(visible(
            &org,
            &SubjectId::new("org"),
            ProfileVisibility::Private,
            None
        ));
    }
}
