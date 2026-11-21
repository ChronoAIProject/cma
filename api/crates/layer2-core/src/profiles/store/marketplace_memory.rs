use async_trait::async_trait;

use crate::profiles::{
    ForkedProfile, MarketplaceEntry, MarketplaceError, MarketplaceKind, MarketplaceLocator,
    MarketplacePage, MarketplaceQuery, MarketplaceRevision, MarketplaceStore, ProfileStoreError,
    ProfileUseRecorder,
};

use super::marketplace_support::*;
use super::*;

const SYSTEM_OWNER: &str = "system";

#[async_trait]
impl MarketplaceStore for InMemoryProfileStore {
    async fn list_marketplace(
        &self,
        query: &MarketplaceQuery,
    ) -> Result<MarketplacePage, MarketplaceError> {
        if query.limit == 0 || query.limit > 50 {
            return Err(MarketplaceError::InvalidPage);
        }
        let state = self.lock();
        let needle = query
            .text
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty());
        let mut rows = Vec::new();
        if query.kind.is_none_or(|kind| kind == MarketplaceKind::Agent) {
            for profile in state.agents.values().filter(|profile| {
                !profile.internal && visible(profile.visibility, &profile.retired_at)
            }) {
                if matches_query(&profile.name, &profile.description, needle) {
                    if let Some(revision) = latest_agent_revision(&state, &profile.id) {
                        rows.push(summary(
                            MarketplaceKind::Agent,
                            profile.id.as_str(),
                            &profile.name,
                            &profile.description,
                            &profile.owner,
                            revision.revision,
                            &revision.published_at,
                            profile.uses_count,
                        ));
                    }
                }
            }
        }
        if query
            .kind
            .is_none_or(|kind| kind == MarketplaceKind::Workspace)
        {
            for profile in state.workspaces.values().filter(|profile| {
                !profile.internal && visible(profile.visibility, &profile.retired_at)
            }) {
                if matches_query(&profile.name, &profile.description, needle) {
                    if let Some(revision) = latest_workspace_revision(&state, &profile.id) {
                        rows.push(summary(
                            MarketplaceKind::Workspace,
                            profile.id.as_str(),
                            &profile.name,
                            &profile.description,
                            &profile.owner,
                            revision.revision,
                            &revision.published_at,
                            profile.uses_count,
                        ));
                    }
                }
            }
        }
        if query
            .kind
            .is_none_or(|kind| kind == MarketplaceKind::Sandbox)
        {
            for profile in state.sandboxes.values().filter(|profile| {
                !profile.internal && visible(profile.visibility, &profile.retired_at)
            }) {
                if matches_query(&profile.name, &profile.description, needle) {
                    if let Some(revision) = latest_sandbox_revision(&state, &profile.id) {
                        rows.push(summary(
                            MarketplaceKind::Sandbox,
                            profile.id.as_str(),
                            &profile.name,
                            &profile.description,
                            &profile.owner,
                            revision.revision,
                            &revision.published_at,
                            profile.uses_count,
                        ));
                    }
                }
            }
        }
        rows.sort_by(|left, right| {
            right
                .published_at
                .cmp(&left.published_at)
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.id.cmp(&right.id))
        });
        if let Some(cursor) = &query.cursor {
            let position = rows
                .iter()
                .position(|row| row_cursor(row) == *cursor)
                .ok_or(MarketplaceError::InvalidPage)?;
            rows.drain(..=position);
        }
        let has_more = rows.len() > usize::from(query.limit);
        rows.truncate(usize::from(query.limit));
        let next = has_more.then(|| rows.last().map(row_cursor)).flatten();
        Ok(MarketplacePage { items: rows, next })
    }

    async fn read_marketplace(
        &self,
        locator: &MarketplaceLocator,
    ) -> Result<MarketplaceEntry, MarketplaceError> {
        let state = self.lock();
        match locator {
            MarketplaceLocator::Agent(id) => {
                let profile = state
                    .agents
                    .get(id)
                    .filter(|profile| {
                        !profile.internal && visible(profile.visibility, &profile.retired_at)
                    })
                    .ok_or(MarketplaceError::NotFound)?;
                let revision =
                    latest_agent_revision(&state, id).ok_or(MarketplaceError::NotFound)?;
                Ok(MarketplaceEntry {
                    owner: profile.owner.clone(),
                    summary: summary(
                        MarketplaceKind::Agent,
                        id.as_str(),
                        &profile.name,
                        &profile.description,
                        &profile.owner,
                        revision.revision,
                        &revision.published_at,
                        profile.uses_count,
                    ),
                    revision: MarketplaceRevision::Agent(revision.clone()),
                    resolved_profiles: vec![],
                    resolved_revisions: vec![],
                })
            }
            MarketplaceLocator::Workspace(id) => {
                let profile = state
                    .workspaces
                    .get(id)
                    .filter(|profile| {
                        !profile.internal && visible(profile.visibility, &profile.retired_at)
                    })
                    .ok_or(MarketplaceError::NotFound)?;
                let revision =
                    latest_workspace_revision(&state, id).ok_or(MarketplaceError::NotFound)?;
                let resolved_revisions = revision
                    .content
                    .agent_profiles
                    .iter()
                    .map(|reference| resolve_agent_revision(&state, reference))
                    .collect::<Result<Vec<_>, _>>()?;
                let resolved_profiles = resolved_revisions
                    .iter()
                    .map(|resolved| resolved.resolved_name())
                    .collect();
                Ok(MarketplaceEntry {
                    owner: profile.owner.clone(),
                    summary: summary(
                        MarketplaceKind::Workspace,
                        id.as_str(),
                        &profile.name,
                        &profile.description,
                        &profile.owner,
                        revision.revision,
                        &revision.published_at,
                        profile.uses_count,
                    ),
                    revision: MarketplaceRevision::Workspace(revision.clone()),
                    resolved_profiles,
                    resolved_revisions,
                })
            }
            MarketplaceLocator::Sandbox(id) => {
                let profile = state
                    .sandboxes
                    .get(id)
                    .filter(|profile| {
                        !profile.internal && visible(profile.visibility, &profile.retired_at)
                    })
                    .ok_or(MarketplaceError::NotFound)?;
                let revision =
                    latest_sandbox_revision(&state, id).ok_or(MarketplaceError::NotFound)?;
                let resolved_revisions = revision
                    .content
                    .workspace_profiles
                    .iter()
                    .map(|reference| resolve_workspace_revision(&state, reference))
                    .collect::<Result<Vec<_>, _>>()?;
                let resolved_profiles = resolved_revisions
                    .iter()
                    .map(|resolved| resolved.resolved_name())
                    .collect();
                Ok(MarketplaceEntry {
                    owner: profile.owner.clone(),
                    summary: summary(
                        MarketplaceKind::Sandbox,
                        id.as_str(),
                        &profile.name,
                        &profile.description,
                        &profile.owner,
                        revision.revision,
                        &revision.published_at,
                        profile.uses_count,
                    ),
                    revision: MarketplaceRevision::Sandbox(revision.clone()),
                    resolved_profiles,
                    resolved_revisions,
                })
            }
        }
    }

    async fn set_profile_visibility(
        &self,
        owner: &SubjectId,
        locator: &MarketplaceLocator,
        visibility: ProfileVisibility,
        updated_at: String,
        bounds: ProfileCapacityBounds,
        scanner: &dyn crate::profiles::PublicValueScanner,
    ) -> Result<(), MarketplaceError> {
        if owner.as_str() == SYSTEM_OWNER {
            return Err(MarketplaceError::Forbidden);
        }
        let inspected = super::marketplace_public_values::inspect(
            self, owner, locator, visibility, bounds, scanner,
        )
        .await?;
        let mut state = self.lock();
        if let Some(inspected) = inspected {
            inspected.verify(&state)?;
        }
        match locator {
            MarketplaceLocator::Agent(id) => {
                let revisions: Vec<_> = state
                    .agent_revisions
                    .iter()
                    .filter(|((stored, _), _)| stored == id)
                    .map(|(_, revision)| revision.clone())
                    .collect();
                require_published_and_valid(visibility, &revisions, |revision| {
                    revision.content.validate()
                })?;
                let profile = state
                    .agents
                    .get_mut(id)
                    .filter(|profile| !profile.internal && &profile.owner == owner)
                    .ok_or(MarketplaceError::NotFound)?;
                profile.visibility = visibility;
                profile.updated_at = updated_at;
            }
            MarketplaceLocator::Workspace(id) => {
                let revisions: Vec<_> = state
                    .workspace_revisions
                    .iter()
                    .filter(|((stored, _), _)| stored == id)
                    .map(|(_, revision)| revision.clone())
                    .collect();
                require_published_and_valid(visibility, &revisions, |revision| {
                    revision.content.validate()
                })?;
                let profile = state
                    .workspaces
                    .get_mut(id)
                    .filter(|profile| !profile.internal && &profile.owner == owner)
                    .ok_or(MarketplaceError::NotFound)?;
                profile.visibility = visibility;
                profile.updated_at = updated_at;
            }
            MarketplaceLocator::Sandbox(id) => {
                let revisions: Vec<_> = state
                    .sandbox_revisions
                    .iter()
                    .filter(|((stored, _), _)| stored == id)
                    .map(|(_, revision)| revision.clone())
                    .collect();
                require_published_and_valid(visibility, &revisions, |revision| {
                    revision.content.validate(bounds)
                })?;
                let profile = state
                    .sandboxes
                    .get_mut(id)
                    .filter(|profile| !profile.internal && &profile.owner == owner)
                    .ok_or(MarketplaceError::NotFound)?;
                if visibility != ProfileVisibility::Public {
                    profile.availability.warmup_enabled = false;
                }
                profile.visibility = visibility;
                profile.updated_at = updated_at;
            }
        }
        Ok(())
    }

    async fn fork_profile(
        &self,
        owner: &SubjectId,
        locator: &MarketplaceLocator,
        revision: u32,
        now: String,
    ) -> Result<ForkedProfile, MarketplaceError> {
        let mut state = self.lock();
        match locator {
            MarketplaceLocator::Agent(id) => {
                let source = state
                    .agents
                    .get(id)
                    .filter(|profile| {
                        !profile.internal && visible(profile.visibility, &profile.retired_at)
                    })
                    .ok_or(MarketplaceError::NotFound)?
                    .clone();
                let revision = state
                    .agent_revisions
                    .get(&(id.clone(), revision))
                    .ok_or(MarketplaceError::NotFound)?
                    .clone();
                let profile = AgentProfile {
                    internal: false,
                    id: AgentProfileId::generate(),
                    owner: owner.clone(),
                    name: fork_name(&source.name),
                    description: source.description,
                    visibility: ProfileVisibility::Private,
                    forked_from: Some(super::super::AgentProfileRef::new(
                        id.clone(),
                        revision.revision,
                    )),
                    uses_count: 0,
                    draft: revision.content,
                    draft_version: 1,
                    created_at: now.clone(),
                    updated_at: now,
                    retired_at: None,
                };
                state.agents.insert(profile.id.clone(), profile.clone());
                Ok(ForkedProfile::Agent(profile))
            }
            MarketplaceLocator::Workspace(id) => {
                let source = state
                    .workspaces
                    .get(id)
                    .filter(|profile| {
                        !profile.internal && visible(profile.visibility, &profile.retired_at)
                    })
                    .ok_or(MarketplaceError::NotFound)?
                    .clone();
                let revision = state
                    .workspace_revisions
                    .get(&(id.clone(), revision))
                    .ok_or(MarketplaceError::NotFound)?
                    .clone();
                let profile = WorkspaceProfile {
                    internal: false,
                    id: WorkspaceProfileId::generate(),
                    owner: owner.clone(),
                    name: fork_name(&source.name),
                    description: source.description,
                    visibility: ProfileVisibility::Private,
                    forked_from: Some(super::super::WorkspaceProfileRef::new(
                        id.clone(),
                        revision.revision,
                    )),
                    uses_count: 0,
                    draft: revision.content,
                    draft_version: 1,
                    created_at: now.clone(),
                    updated_at: now,
                    retired_at: None,
                };
                state.workspaces.insert(profile.id.clone(), profile.clone());
                Ok(ForkedProfile::Workspace(profile))
            }
            MarketplaceLocator::Sandbox(id) => {
                let source = state
                    .sandboxes
                    .get(id)
                    .filter(|profile| {
                        !profile.internal && visible(profile.visibility, &profile.retired_at)
                    })
                    .ok_or(MarketplaceError::NotFound)?
                    .clone();
                let revision = state
                    .sandbox_revisions
                    .get(&(id.clone(), revision))
                    .ok_or(MarketplaceError::NotFound)?
                    .clone();
                let profile = SandboxProfile {
                    internal: false,
                    availability: Default::default(),
                    id: SandboxProfileId::generate(),
                    owner: owner.clone(),
                    name: fork_name(&source.name),
                    description: source.description,
                    visibility: ProfileVisibility::Private,
                    forked_from: Some(super::super::SandboxProfileRef::new(
                        id.clone(),
                        revision.revision,
                    )),
                    uses_count: 0,
                    draft: revision.content,
                    draft_version: 1,
                    created_at: now.clone(),
                    updated_at: now,
                    retired_at: None,
                };
                state.sandboxes.insert(profile.id.clone(), profile.clone());
                Ok(ForkedProfile::Sandbox(profile))
            }
        }
    }
}

#[async_trait]
impl ProfileUseRecorder for InMemoryProfileStore {
    async fn record_profile_use(
        &self,
        locator: &MarketplaceLocator,
    ) -> Result<(), ProfileStoreError> {
        let mut state = self.lock();
        let count = match locator {
            MarketplaceLocator::Agent(id) => {
                &mut state
                    .agents
                    .get_mut(id)
                    .ok_or(ProfileStoreError::NotFound)?
                    .uses_count
            }
            MarketplaceLocator::Workspace(id) => {
                &mut state
                    .workspaces
                    .get_mut(id)
                    .ok_or(ProfileStoreError::NotFound)?
                    .uses_count
            }
            MarketplaceLocator::Sandbox(id) => {
                &mut state
                    .sandboxes
                    .get_mut(id)
                    .ok_or(ProfileStoreError::NotFound)?
                    .uses_count
            }
        };
        *count = count.saturating_add(1);
        Ok(())
    }
}
