use crate::ids::{AgentProfileId, SandboxProfileId, SubjectId, WorkspaceProfileId};
use crate::profiles::{
    content_digest, AgentProfileRef, AgentProfileRevision, MarketplaceError, MarketplaceKind,
    MarketplaceRevision, MarketplaceSummary, ProfileError, ProfileVisibility,
    ResolvedProfileRevision, SandboxProfileRevision, WorkspaceProfileRef, WorkspaceProfileRevision,
};

use super::State;

pub(super) fn visible(visibility: ProfileVisibility, retired_at: &Option<String>) -> bool {
    visibility == ProfileVisibility::Public && retired_at.is_none()
}

pub(super) fn matches_query(name: &str, description: &str, query: Option<&str>) -> bool {
    query.is_none_or(|query| {
        let query = query.to_lowercase();
        name.to_lowercase().contains(&query) || description.to_lowercase().contains(&query)
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn summary(
    kind: MarketplaceKind,
    id: &str,
    name: &str,
    description: &str,
    owner: &SubjectId,
    revision: u32,
    published_at: &str,
    uses_count: u64,
) -> MarketplaceSummary {
    MarketplaceSummary {
        kind,
        id: id.into(),
        name: name.into(),
        description: description.into(),
        owner_display: owner.to_string(),
        latest_revision: revision,
        published_at: published_at.into(),
        uses_count,
    }
}

pub(super) fn row_cursor(row: &MarketplaceSummary) -> String {
    format!("{}|{}|{}", row.published_at, row.name, row.id)
}

pub(super) fn latest_agent_revision<'a>(
    state: &'a State,
    id: &AgentProfileId,
) -> Option<&'a AgentProfileRevision> {
    state
        .agent_revisions
        .iter()
        .filter(|((stored, _), _)| stored == id)
        .max_by_key(|((_, number), _)| *number)
        .map(|(_, revision)| revision)
}

pub(super) fn latest_workspace_revision<'a>(
    state: &'a State,
    id: &WorkspaceProfileId,
) -> Option<&'a WorkspaceProfileRevision> {
    state
        .workspace_revisions
        .iter()
        .filter(|((stored, _), _)| stored == id)
        .max_by_key(|((_, number), _)| *number)
        .map(|(_, revision)| revision)
}

pub(super) fn latest_sandbox_revision<'a>(
    state: &'a State,
    id: &SandboxProfileId,
) -> Option<&'a SandboxProfileRevision> {
    state
        .sandbox_revisions
        .iter()
        .filter(|((stored, _), _)| stored == id)
        .max_by_key(|((_, number), _)| *number)
        .map(|(_, revision)| revision)
}

pub(super) fn resolve_agent_revision(
    state: &State,
    reference: &AgentProfileRef,
) -> Result<ResolvedProfileRevision, MarketplaceError> {
    let profile = state
        .agents
        .get(&reference.id)
        .ok_or_else(|| corrupt_reference("agent profile"))?;
    if profile.id != reference.id {
        return Err(corrupt_reference("agent profile"));
    }
    let revision = state
        .agent_revisions
        .get(&(reference.id.clone(), reference.revision))
        .ok_or_else(|| corrupt_reference("agent profile revision"))?;
    validate_revision(
        revision.profile_id == reference.id,
        revision.revision == reference.revision,
        &revision.content,
        &revision.digest,
        "agent profile revision",
    )?;
    Ok(ResolvedProfileRevision {
        owner: profile.owner.clone(),
        name: profile.name.clone(),
        revision: MarketplaceRevision::Agent(revision.clone()),
    })
}

pub(super) fn resolve_workspace_revision(
    state: &State,
    reference: &WorkspaceProfileRef,
) -> Result<ResolvedProfileRevision, MarketplaceError> {
    let profile = state
        .workspaces
        .get(&reference.id)
        .ok_or_else(|| corrupt_reference("workspace profile"))?;
    if profile.id != reference.id {
        return Err(corrupt_reference("workspace profile"));
    }
    let revision = state
        .workspace_revisions
        .get(&(reference.id.clone(), reference.revision))
        .ok_or_else(|| corrupt_reference("workspace profile revision"))?;
    validate_revision(
        revision.profile_id == reference.id,
        revision.revision == reference.revision,
        &revision.content,
        &revision.digest,
        "workspace profile revision",
    )?;
    Ok(ResolvedProfileRevision {
        owner: profile.owner.clone(),
        name: profile.name.clone(),
        revision: MarketplaceRevision::Workspace(revision.clone()),
    })
}

fn validate_revision<T: serde::Serialize>(
    id_matches: bool,
    number_matches: bool,
    content: &T,
    stored_digest: &str,
    kind: &str,
) -> Result<(), MarketplaceError> {
    let actual_digest = content_digest(content)
        .map_err(|error| MarketplaceError::Corrupt(format!("{kind}: {error}")))?;
    if !id_matches || !number_matches || actual_digest != stored_digest {
        return Err(corrupt_reference(kind));
    }
    Ok(())
}

fn corrupt_reference(kind: &str) -> MarketplaceError {
    MarketplaceError::Corrupt(format!("referenced {kind} is unavailable or corrupt"))
}

pub(super) fn require_published_and_valid<T>(
    visibility: ProfileVisibility,
    revisions: &[T],
    validate: impl Fn(&T) -> Result<(), ProfileError>,
) -> Result<(), MarketplaceError> {
    if visibility == ProfileVisibility::Private {
        return Ok(());
    }
    if revisions.is_empty() {
        return Err(MarketplaceError::Unpublished);
    }
    revisions
        .iter()
        .try_for_each(validate)
        .map_err(|error| MarketplaceError::Validation(error.code.into()))
}

pub(super) fn fork_name(source: &str) -> String {
    let mut name: String = source.chars().take(75).collect();
    name.push_str(" copy");
    name
}
