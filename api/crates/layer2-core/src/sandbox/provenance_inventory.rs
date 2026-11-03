//! Bounded, read-only creation inventory. Every scanned row, including filtered
//! rows and lookahead, needs current access before its pagination can be exposed.
use std::{sync::Arc, time::Duration};

use super::store::{
    AgentChannelUsageSnapshot, AgentProvenanceInventoryStore, Page, PageRequest, SandboxStoreError,
    MAX_PAGE_SIZE,
};
use super::{AgentCreationCategory, AgentListQuery, AgentStatus};
use crate::resource_access::{
    AccessError, AccessPurpose, AuthorizedResource, CallerContext, ResourceAccessError,
    ResourceAccessService, ResourceAccessStoreError, ResourceCapability, ResourceTarget,
};
use crate::{AgentId, Caller, Capability, SubjectId};
use futures_util::{stream, StreamExt, TryStreamExt};

mod authority;
use authority::{authorize_candidates, InventoryAuthority};

const READ_CAPABILITY: ResourceCapability = ResourceCapability::ViewDetails;

#[derive(Debug, Clone, Default)]
pub struct AgentProvenanceInventoryQuery {
    pub task_status: Option<super::AgentTaskStatus>,
    pub organization: Option<SubjectId>,
    /// Explicit shares to current organizations, excluding the caller's own resources.
    /// Mutually exclusive with `organization`.
    pub shared: bool,
    pub agents: AgentListQuery,
    pub creation_origin: Option<AgentCreationCategory>,
    /// Explicit channel facts; neither filter changes immutable creation origin.
    pub channel_active: Option<bool>,
    pub channel_history: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProvenanceInventoryRow {
    pub task_status: super::AgentTaskStatus,
    pub agent_id: AgentId,
    pub title: Option<String>,
    pub status: AgentStatus,
    pub created_at: String,
    pub updated_at: String,
    pub creation_origin: AgentCreationCategory,
    pub channel_usage: super::channel_usage::ChannelUsageSummary,
}

pub struct AgentProvenanceInventoryService {
    store: Arc<dyn AgentProvenanceInventoryStore>,
    access: Arc<ResourceAccessService>,
}

impl AgentProvenanceInventoryService {
    pub fn new(
        store: Arc<dyn AgentProvenanceInventoryStore>,
        access: Arc<ResourceAccessService>,
    ) -> Arc<Self> {
        Arc::new(Self { store, access })
    }

    pub async fn list(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        query: &AgentProvenanceInventoryQuery,
        page: &PageRequest,
    ) -> Result<Page<AgentProvenanceInventoryRow>, ResourceAccessError> {
        if !context.validate_principal() {
            return Err(AccessError::NotFound.into());
        }
        validate(&context.caller, query, page)?;
        tokio::time::timeout(Duration::from_secs(15), async {
            let authority =
                InventoryAuthority::load(&self.access, context, credential, query).await?;
            self.read(authority, credential, query, page).await
        })
        .await
        .map_err(|_| ResourceAccessStoreError::Unavailable)?
    }

    pub async fn list_personal(
        &self,
        caller: &Caller,
        query: &AgentProvenanceInventoryQuery,
        page: &PageRequest,
    ) -> Result<Page<AgentProvenanceInventoryRow>, ResourceAccessError> {
        validate(caller, query, page)?;
        if query.organization.is_some() || query.shared {
            return Err(AccessError::Forbidden.into());
        }
        tokio::time::timeout(
            Duration::from_secs(15),
            self.read(InventoryAuthority::personal(caller), None, query, page),
        )
        .await
        .map_err(|_| ResourceAccessStoreError::Unavailable)?
    }

    async fn read(
        &self,
        authority: InventoryAuthority,
        credential: Option<&str>,
        query: &AgentProvenanceInventoryQuery,
        page: &PageRequest,
    ) -> Result<Page<AgentProvenanceInventoryRow>, ResourceAccessError> {
        let mut parents = Vec::new();
        if let Some(id) = &query.agents.sandbox_id {
            parents.push(ResourceTarget::Sandbox {
                sandbox_id: id.clone(),
            });
        }
        if let Some(id) = &query.agents.workspace_id {
            parents.push(ResourceTarget::Workspace {
                workspace_id: id.clone(),
            });
        }
        let mut parent_decisions = Vec::new();
        for target in parents {
            let decision = match &authority.context {
                Some(context) => {
                    self.access
                        .authorize(
                            context,
                            credential,
                            &target,
                            READ_CAPABILITY,
                            AccessPurpose::Read,
                        )
                        .await?
                }
                None => {
                    self.access
                        .authorize_personal(&authority.caller, &target, READ_CAPABILITY)
                        .await?
                }
            };
            parent_decisions.push((target, decision));
        }
        // A category filter may return a partial/empty page with continuation.
        // Its scan always advances and never accumulates an unbounded inventory.
        let scan = PageRequest::new(
            if query.task_status.is_some()
                || query.creation_origin.is_some()
                || query.channel_active.is_some()
                || query.channel_history.is_some()
            {
                MAX_PAGE_SIZE
            } else {
                page.limit
            },
            page.cursor.clone(),
        )
        .map_err(|_| ResourceAccessStoreError::Invalid)?;
        let snapshots = match &authority.scope {
            Some(scope) => {
                self.store
                    .scan_agent_provenance_inventory(scope, &query.agents, &scan)
                    .await
            }
            None => {
                self.store
                    .scan_shared_agent_provenance_inventory(
                        &authority.caller.subject,
                        &authority.organization_ids(),
                        &query.agents,
                        &scan,
                    )
                    .await
            }
        };
        self.finish_parents(&parent_decisions).await?;
        authority.require_membership(&self.access)?;
        let snapshots = snapshots.map_err(store_error)?;
        if snapshots.len() > usize::from(scan.limit) + 1 {
            return Err(ResourceAccessStoreError::Corrupt.into());
        }
        let authorized = authorize_candidates(&authority, &query.agents, &scan, &snapshots)?;
        let records = snapshots
            .iter()
            .zip(&authorized)
            .map(|(snapshot, held)| {
                let id = held
                    .snapshot
                    .lineage
                    .agent
                    .as_ref()
                    .ok_or(ResourceAccessStoreError::Corrupt)?
                    .id
                    .clone();
                snapshot.validate(&id).map_err(store_error)?;
                let creation = snapshot
                    .provenance
                    .creation_provenance(&id)
                    .map_err(store_error)?;
                Ok::<_, ResourceAccessError>((creation, snapshot.state.summary()))
            })
            .collect::<Result<Vec<_>, _>>();

        // Revalidate even a failed provenance projection. Management authority
        // and private creator/source references play no role in this inventory.
        let mut checks = Vec::with_capacity(authorized.len());
        for held in &authorized {
            checks.push(self.finish_agent(held));
        }
        let current = stream::iter(checks)
            .buffered(8)
            .try_collect::<Vec<_>>()
            .await?;
        self.finish_parents(&parent_decisions).await?;
        authority.require_membership(&self.access)?;
        for held in &authorized {
            held.decision.require(READ_CAPABILITY)?;
        }
        for (_, held) in &parent_decisions {
            held.decision.require(READ_CAPABILITY)?;
        }
        let records = records?;
        let mut rows = Vec::with_capacity(records.len());
        for (((record, channel_usage), snapshot), original) in
            records.into_iter().zip(current).zip(snapshots.iter())
        {
            super::provenance_service::require_identity(&record, &snapshot)?;
            let agent = snapshot
                .lineage
                .agent
                .ok_or(ResourceAccessStoreError::Corrupt)?;
            if !query.agents.matches(&agent) {
                return Err(ResourceAccessStoreError::Conflict.into());
            }
            rows.push(AgentProvenanceInventoryRow {
                task_status: (if original
                    .provenance
                    .resource
                    .lineage
                    .agent
                    .as_ref()
                    .is_some_and(|previous| {
                        previous.revision == agent.revision
                            && previous.conversation.last_seq == agent.conversation.last_seq
                    }) {
                    original.task_status
                } else {
                    super::AgentTaskStatus::from_latest(&agent, None, false)
                })
                .in_sandbox(&snapshot.lineage.sandbox),
                agent_id: record.agent_id,
                title: navigation_title(&agent),
                status: agent.status,
                created_at: record.agent_created_at,
                updated_at: agent.updated_at,
                creation_origin: record.creation_origin.category(),
                channel_usage,
            });
        }
        Ok(filtered_page(rows, query, &scan, page.limit))
    }

    async fn finish_agent(
        &self,
        held: &AuthorizedResource,
    ) -> Result<crate::resource_access::ResourceAccessSnapshot, ResourceAccessError> {
        let id = held
            .snapshot
            .lineage
            .agent
            .as_ref()
            .ok_or(ResourceAccessStoreError::Corrupt)?
            .id
            .clone();
        self.access
            .current_read_snapshot(
                held,
                &ResourceTarget::Agent { agent_id: id },
                READ_CAPABILITY,
            )
            .await
    }

    async fn finish_parents(
        &self,
        parents: &[(ResourceTarget, AuthorizedResource)],
    ) -> Result<(), ResourceAccessError> {
        for (target, held) in parents {
            self.access
                .validate_read_result(held, target, READ_CAPABILITY)
                .await?;
        }
        Ok(())
    }
}

fn validate(
    caller: &Caller,
    query: &AgentProvenanceInventoryQuery,
    page: &PageRequest,
) -> Result<(), ResourceAccessError> {
    if query.shared && query.organization.is_some() {
        return Err(ResourceAccessStoreError::Invalid.into());
    }
    if !caller.authorization.allows(Capability::UseOwnSandboxes) {
        return Err(AccessError::Forbidden.into());
    }
    PageRequest::new(page.limit, page.cursor.clone())
        .map_err(|_| ResourceAccessStoreError::Invalid)?;
    for id in [
        query.organization.as_ref().map(SubjectId::as_str),
        query.agents.sandbox_id.as_ref().map(|id| id.as_str()),
        query.agents.workspace_id.as_ref().map(|id| id.as_str()),
        page.cursor.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if id.is_empty() || id.trim() != id || id.len() > 256 {
            return Err(ResourceAccessStoreError::Invalid.into());
        }
    }
    if query
        .organization
        .as_ref()
        .is_some_and(|id| id.as_str().len() > 128)
    {
        return Err(ResourceAccessStoreError::Invalid.into());
    }
    Ok(())
}

fn filtered_page(
    mut rows: Vec<AgentProvenanceInventoryRow>,
    query: &AgentProvenanceInventoryQuery,
    scan: &PageRequest,
    limit: u16,
) -> Page<AgentProvenanceInventoryRow> {
    let has_more = rows.len() > usize::from(scan.limit);
    if has_more {
        rows.pop();
    }
    let scanned = rows.last().map(|row| row.agent_id.to_string());
    let mut matches: Vec<_> = rows
        .into_iter()
        .filter(|row| {
            query
                .task_status
                .is_none_or(|status| row.task_status == status)
                && query
                    .creation_origin
                    .is_none_or(|category| row.creation_origin == category)
                && query
                    .channel_active
                    .is_none_or(|active| row.channel_usage.active == active)
                && query
                    .channel_history
                    .is_none_or(|history| row.channel_usage.has_history == history)
        })
        .collect();
    let next = if matches.len() > usize::from(limit) {
        matches.truncate(usize::from(limit));
        matches.last().map(|row| row.agent_id.to_string())
    } else if has_more {
        scanned
    } else {
        None
    };
    Page {
        items: matches,
        next,
    }
}

fn navigation_title(agent: &super::Agent) -> Option<String> {
    let metadata = &agent.conversation.observations.metadata;
    let setup_preview = metadata.description_source
        == Some(super::AgentDescriptionSource::PromptPreview)
        && metadata.description_is_setup_preview;
    agent
        .conversation
        .preferences
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .or_else(|| {
            (!setup_preview)
                .then_some(metadata.description.as_deref())
                .flatten()
                .map(str::trim)
                .filter(|title| !title.is_empty())
        })
        .or_else(|| {
            agent
                .title
                .as_deref()
                .map(str::trim)
                .filter(|title| !title.is_empty())
        })
        .map(str::to_owned)
}

fn store_error(error: SandboxStoreError) -> ResourceAccessError {
    match error {
        SandboxStoreError::NotFound => ResourceAccessStoreError::NotFound,
        SandboxStoreError::Conflict => ResourceAccessStoreError::Conflict,
        SandboxStoreError::InvalidPage => ResourceAccessStoreError::Invalid,
        SandboxStoreError::Corrupt(_) => ResourceAccessStoreError::Corrupt,
        _ => ResourceAccessStoreError::Unavailable,
    }
    .into()
}
