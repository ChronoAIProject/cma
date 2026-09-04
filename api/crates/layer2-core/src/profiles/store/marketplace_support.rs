use crate::ids::{AgentProfileId, SandboxProfileId, SubjectId, WorkspaceProfileId};
use crate::profiles::{
    AgentProfileRevision, MarketplaceError, MarketplaceKind, MarketplaceSummary, ProfileError,
    ProfileVisibility, SandboxProfileRevision, WorkspaceProfileRevision,
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
