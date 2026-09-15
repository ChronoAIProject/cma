//! Bounded, read-only creation inventory. Every scanned row, including filtered
//! rows and lookahead, needs current access before its pagination can be exposed.
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use super::store::{
    AgentProvenanceInventoryStore, AgentProvenanceSnapshot, Page, PageRequest, SandboxStoreError,
    MAX_PAGE_SIZE,
};
use super::{AgentCreationCategory, AgentListQuery, AgentStatus};
use crate::resource_access::{
    AccessDecision, AccessError, AccessPurpose, AuthorizedResource, CallerContext,
    ResourceAccessError, ResourceAccessService, ResourceAccessStoreError, ResourceCapability,
    ResourceOwner, ResourceTarget,
};
use crate::{AgentId, Caller, Capability, SubjectId};
use futures_util::{stream, StreamExt, TryStreamExt};

const READ_CAPABILITY: ResourceCapability = ResourceCapability::ViewDetails;

#[derive(Debug, Clone, Default)]
pub struct AgentProvenanceInventoryQuery {
    pub organization: Option<SubjectId>,
    pub agents: AgentListQuery,
    pub creation_origin: Option<AgentCreationCategory>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProvenanceInventoryRow {
    pub agent_id: AgentId,
    pub title: Option<String>,
    pub status: AgentStatus,
    pub created_at: String,
    pub updated_at: String,
    pub creation_origin: AgentCreationCategory,
}

pub struct AgentProvenanceInventoryService {
    store: Arc<dyn AgentProvenanceInventoryStore>,
    access: Arc<ResourceAccessService>,
}

struct InventoryAuthority {
    caller: Caller,
    scope: ResourceOwner,
    organization: Option<CallerContext>,
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
            let authority = match &query.organization {
                Some(organization) => InventoryAuthority {
                    caller: context.caller.clone(),
                    scope: ResourceOwner::Organization {
                        subject: organization.clone(),
                    },
                    organization: Some(
                        self.access
                            .inventory_context(context, credential, organization)
                            .await?,
                    ),
                },
                None => personal(&context.caller),
            };
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
        if query.organization.is_some() {
            return Err(AccessError::Forbidden.into());
        }
        tokio::time::timeout(
            Duration::from_secs(15),
            self.read(personal(caller), None, query, page),
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
            let decision = match &authority.organization {
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
            if query.creation_origin.is_some() {
                MAX_PAGE_SIZE
            } else {
                page.limit
            },
            page.cursor.clone(),
        )
        .map_err(|_| ResourceAccessStoreError::Invalid)?;
        let snapshots = self
            .store
            .scan_agent_provenance_inventory(&authority.scope, &query.agents, &scan)
            .await;
        self.finish_parents(&parent_decisions).await?;
        self.require_membership(&authority)?;
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
                snapshot.creation_provenance(&id).map_err(store_error)
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
        self.require_membership(&authority)?;
        for held in &authorized {
            held.decision.require(READ_CAPABILITY)?;
        }
        for (_, held) in &parent_decisions {
            held.decision.require(READ_CAPABILITY)?;
        }
        let records = records?;
        let mut rows = Vec::with_capacity(records.len());
        for (record, snapshot) in records.into_iter().zip(current) {
            super::provenance_service::require_identity(&record, &snapshot)?;
            let agent = snapshot
                .lineage
                .agent
                .ok_or(ResourceAccessStoreError::Corrupt)?;
            if !query.agents.matches(&agent) {
                return Err(ResourceAccessStoreError::Conflict.into());
            }
            rows.push(AgentProvenanceInventoryRow {
                agent_id: record.agent_id,
                title: agent.title,
                status: agent.status,
                created_at: record.agent_created_at,
                updated_at: agent.updated_at,
                creation_origin: record.creation_origin.category(),
            });
        }
        Ok(filtered_page(
            rows,
            query.creation_origin,
            &scan,
            page.limit,
        ))
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

    fn require_membership(
        &self,
        authority: &InventoryAuthority,
    ) -> Result<(), ResourceAccessError> {
        if let (Some(context), ResourceOwner::Organization { subject }) =
            (&authority.organization, &authority.scope)
        {
            self.access.require_inventory_membership(context, subject)?;
        }
        Ok(())
    }
}

fn personal(caller: &Caller) -> InventoryAuthority {
    InventoryAuthority {
        caller: caller.clone(),
        scope: ResourceOwner::Person {
            subject: caller.subject.clone(),
        },
        organization: None,
    }
}

fn validate(
    caller: &Caller,
    query: &AgentProvenanceInventoryQuery,
    page: &PageRequest,
) -> Result<(), ResourceAccessError> {
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

fn authorize_candidates(
    authority: &InventoryAuthority,
    query: &AgentListQuery,
    scan: &PageRequest,
    snapshots: &[AgentProvenanceSnapshot],
) -> Result<Vec<AuthorizedResource>, ResourceAccessError> {
    let mut previous = scan.cursor.as_deref();
    let mut result = Vec::with_capacity(snapshots.len());
    for snapshot in snapshots {
        let agent = snapshot
            .resource
            .lineage
            .agent
            .as_ref()
            .ok_or(ResourceAccessStoreError::Corrupt)?;
        if !query.matches(agent) || previous.is_some_and(|cursor| agent.id.as_str() <= cursor) {
            return Err(ResourceAccessStoreError::Corrupt.into());
        }
        previous = Some(agent.id.as_str());
        let scope = snapshot.resource.scope(&ResourceTarget::Agent {
            agent_id: agent.id.clone(),
        })?;
        let decision = match &authority.organization {
            Some(context) => {
                let ResourceOwner::Organization { subject } = &authority.scope else {
                    return Err(ResourceAccessStoreError::Corrupt.into());
                };
                snapshot.resource.organization_agent(subject)?;
                AccessDecision::evaluate(
                    context,
                    &scope,
                    &snapshot.resource.shares,
                    AccessPurpose::Read,
                    SystemTime::now(),
                )?
            }
            None => {
                if scope.owner != authority.scope {
                    return Err(ResourceAccessStoreError::Corrupt.into());
                }
                AccessDecision::personal(&authority.caller, &authority.caller.subject)
            }
        };
        decision.require(READ_CAPABILITY)?;
        result.push(AuthorizedResource {
            snapshot: snapshot.resource.clone(),
            decision,
        });
    }
    Ok(result)
}

fn filtered_page(
    mut rows: Vec<AgentProvenanceInventoryRow>,
    category: Option<AgentCreationCategory>,
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
        .filter(|row| category.is_none_or(|category| row.creation_origin == category))
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
