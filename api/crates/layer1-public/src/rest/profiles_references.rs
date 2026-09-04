use layer2_core::profiles::{AgentProfileRef, ProfileVisibility, WorkspaceProfileRef};
use layer2_core::SubjectId;

use super::profiles::{generated_id_is_valid, ProfilesState};
use super::profiles_problem::ProfileProblem;

pub(super) async fn validate_agent_references(
    state: &ProfilesState,
    owner: &SubjectId,
    references: &[AgentProfileRef],
) -> Result<(), ProfileProblem> {
    for (index, reference) in references.iter().enumerate() {
        let path = format!("content.agent_profiles[{index}]");
        if !generated_id_is_valid(reference.id.as_str(), "agp_") || reference.revision == 0 {
            return Err(ProfileProblem::ReferenceInvalid { path });
        }
        let profile = state
            .agents
            .agent_profile(&reference.id)
            .await
            .map_err(|error| ProfileProblem::from_store(error, &path))?
            .filter(|profile| {
                profile.retired_at.is_none()
                    && (&profile.owner == owner || profile.visibility == ProfileVisibility::Public)
            })
            .ok_or_else(|| ProfileProblem::ReferenceInvalid { path: path.clone() })?;
        let revision = state
            .agents
            .agent_profile_revision(&profile.id, reference.revision)
            .await
            .map_err(|error| ProfileProblem::from_store(error, &path))?;
        if revision.is_none() {
            return Err(ProfileProblem::ReferenceInvalid { path });
        }
    }
    Ok(())
}

pub(super) async fn validate_workspace_references(
    state: &ProfilesState,
    owner: &SubjectId,
    references: &[WorkspaceProfileRef],
) -> Result<(), ProfileProblem> {
    for (index, reference) in references.iter().enumerate() {
        let path = format!("content.workspace_profiles[{index}]");
        if !generated_id_is_valid(reference.id.as_str(), "wkp_") || reference.revision == 0 {
            return Err(ProfileProblem::ReferenceInvalid { path });
        }
        let profile = state
            .workspaces
            .workspace_profile(&reference.id)
            .await
            .map_err(|error| ProfileProblem::from_store(error, &path))?
            .filter(|profile| {
                profile.retired_at.is_none()
                    && (&profile.owner == owner || profile.visibility == ProfileVisibility::Public)
            })
            .ok_or_else(|| ProfileProblem::ReferenceInvalid { path: path.clone() })?;
        let revision = state
            .workspaces
            .workspace_profile_revision(&profile.id, reference.revision)
            .await
            .map_err(|error| ProfileProblem::from_store(error, &path))?;
        if revision.is_none() {
            return Err(ProfileProblem::ReferenceInvalid { path });
        }
    }
    Ok(())
}
