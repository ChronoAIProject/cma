use layer2_core::profiles::{references, AgentProfileRef, WorkspaceProfileRef};
use layer2_core::{SandboxProfileId, SubjectId, WorkspaceProfileId};

use super::profiles::{generated_id_is_valid, ProfilesState};
use super::profiles_problem::ProfileProblem;

pub(super) async fn validate_agent_references(
    state: &ProfilesState,
    owner: &SubjectId,
    parent: Option<&WorkspaceProfileId>,
    references: &[AgentProfileRef],
) -> Result<(), ProfileProblem> {
    for (index, reference) in references.iter().enumerate() {
        let path = format!("content.agent_profiles[{index}]");
        if (reference.id.as_str() != layer2_core::DEFAULT_AGENT_PROFILE_ID
            && !generated_id_is_valid(reference.id.as_str(), "agp_"))
            || reference.revision == 0
        {
            return Err(ProfileProblem::ReferenceInvalid { path });
        }
    }
    references::validate_agent_references(
        state.agents.as_ref(),
        state.workspaces.as_ref(),
        owner,
        parent,
        references,
    )
    .await
    .map_err(|error| reference_problem(error, "content.agent_profiles"))
}

pub(super) async fn validate_workspace_references(
    state: &ProfilesState,
    owner: &SubjectId,
    parent: Option<&SandboxProfileId>,
    references: &[WorkspaceProfileRef],
) -> Result<(), ProfileProblem> {
    for (index, reference) in references.iter().enumerate() {
        let path = format!("content.workspace_profiles[{index}]");
        if (reference.id.as_str() != layer2_core::DEFAULT_WORKSPACE_PROFILE_ID
            && !generated_id_is_valid(reference.id.as_str(), "wkp_"))
            || reference.revision == 0
        {
            return Err(ProfileProblem::ReferenceInvalid { path });
        }
    }
    references::validate_workspace_references(
        state.workspaces.as_ref(),
        state.sandboxes.as_ref(),
        owner,
        parent,
        references,
    )
    .await
    .map_err(|error| reference_problem(error, "content.workspace_profiles"))
}

fn reference_problem(error: references::ReferenceValidationError, field: &str) -> ProfileProblem {
    let path = error
        .index
        .map_or_else(|| field.to_owned(), |index| format!("{field}[{index}]"));
    ProfileProblem::from_store(error.error, &path)
}
