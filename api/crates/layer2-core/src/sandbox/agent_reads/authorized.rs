use super::*;
use crate::resource_access::{
    AccessError, AccessPurpose, AuthorizedResource, CallerContext, ResourceAccessError,
    ResourceAccessStoreError, ResourceLineage, ResourceOwner,
};

/// Personal detail retains its established complete hierarchy. Other callers
/// receive an Agent-only projection; a visible Agent never grants parent data.
pub enum AgentDetailReading {
    Personal(Box<AgentLineage>),
    Shared(Box<SharedAgentDetail>),
}

#[derive(Debug, Clone)]
pub struct SharedAgentDetail {
    pub id: AgentId,
    pub status: super::super::AgentStatus,
    pub title: Option<String>,
    pub model: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub ready_at: Option<String>,
    pub stopped_at: Option<String>,
    pub deleted_at: Option<String>,
}

impl AgentReadService {
    pub async fn detail_authorized(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &AgentId,
    ) -> Result<AgentDetailReading, AgentReadError> {
        let authorized = self
            .authorized(context, credential, id, ResourceCapability::ViewDetails)
            .await?;
        detail_projection(authorized, context)
    }

    pub async fn organization_list_authorized(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        organization: &SubjectId,
        query: &AgentListQuery,
        page: &PageRequest,
    ) -> Result<Page<AgentDetailReading>, AgentReadError> {
        let access = self.access.as_ref().ok_or_else(|| {
            AgentReadError::Unavailable("resource access service is not composed".into())
        })?;
        let page = access
            .organization_agent_inventory(context, credential, organization, query, page)
            .await
            .map_err(access_error)?;
        Ok(Page {
            items: page
                .items
                .into_iter()
                .map(|value| detail_projection(value, context))
                .collect::<Result<_, _>>()?,
            next: page.next,
        })
    }

    pub async fn transcript_authorized(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &AgentId,
    ) -> Result<AgentTranscript, AgentReadError> {
        let authorized = self
            .authorized(context, credential, id, ResourceCapability::ViewHistory)
            .await?;
        let agent = authorized
            .snapshot
            .lineage
            .agent
            .as_ref()
            .ok_or_else(|| corrupt("authorized Agent snapshot has no Agent"))?;
        require_granted(agent, crate::RuntimeCapability::Conversation)?;
        let entries = self.complete_log(agent).await?;
        let mut transcript = fold_agent_log(agent, &entries)
            .map_err(AgentReadError::Corrupt)?
            .transcript;
        // Attribution/correlation metadata is not a history access grant. The
        // transcript projection includes conversation messages and outcomes only.
        if !is_personal(&authorized, context) {
            super::visibility::shared_transcript(&mut transcript);
        }
        self.finish_read(&authorized, id, ResourceCapability::ViewHistory)
            .await?;
        Ok(transcript)
    }

    /// Authorize access to the runtime-owned history and retain the exact
    /// Agent/Sandbox lineage needed to reach that runtime.
    pub async fn history_lineage_authorized(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &AgentId,
    ) -> Result<AgentLineage, AgentReadError> {
        let authorized = self
            .authorized(context, credential, id, ResourceCapability::ViewHistory)
            .await?;
        let agent = authorized
            .snapshot
            .lineage
            .agent
            .as_ref()
            .ok_or_else(|| corrupt("authorized Agent snapshot has no Agent"))?;
        require_granted(agent, crate::RuntimeCapability::Conversation)?;
        let lineage = super::authorized::lineage(authorized.snapshot.lineage.clone())?;
        self.finish_read(&authorized, id, ResourceCapability::ViewHistory)
            .await?;
        Ok(lineage)
    }

    pub async fn log_authorized(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &AgentId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<AgentVisibleLogEntry>, AgentReadError> {
        let authorized = self
            .authorized(context, credential, id, ResourceCapability::ViewHistory)
            .await?;
        validate_limit(limit)?;
        let agent = authorized
            .snapshot
            .lineage
            .agent
            .as_ref()
            .ok_or_else(|| corrupt("authorized Agent snapshot has no Agent"))?;
        let entries = self.complete_log(agent).await?;
        fold_agent_log(agent, &entries).map_err(AgentReadError::Corrupt)?;
        let personal = is_personal(&authorized, context);
        let page = entries
            .into_iter()
            .filter(|entry| entry.seq > after)
            .take(usize::from(limit))
            .map(|entry| AgentVisibleLogEntry::from_entry(entry, personal))
            .collect();
        self.finish_read(&authorized, id, ResourceCapability::ViewHistory)
            .await?;
        Ok(page)
    }

    pub async fn resources_authorized(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &AgentId,
    ) -> Result<AgentResourceReading, AgentReadError> {
        let authorized = self
            .authorized(context, credential, id, ResourceCapability::ViewDetails)
            .await?;
        let agent = authorized
            .snapshot
            .lineage
            .agent
            .as_ref()
            .ok_or_else(|| corrupt("authorized Agent snapshot has no Agent"))?;
        let access = self.access.as_ref().ok_or_else(|| {
            AgentReadError::Unavailable("resource access service is not composed".into())
        })?;
        let target = ResourceTarget::Sandbox {
            sandbox_id: agent.sandbox_id.clone(),
        };
        let sandbox = access
            .authorize(
                context,
                credential,
                &target,
                ResourceCapability::ViewDetails,
                AccessPurpose::Read,
            )
            .await
            .map_err(|error| match error {
                ResourceAccessError::Access(AccessError::NotFound) => AgentReadError::Forbidden,
                error => access_error(error),
            })?;
        require_granted(agent, crate::RuntimeCapability::Resources)?;
        let reading = self
            .resources
            .sandbox_resources(&sandbox.snapshot.lineage.sandbox)
            .await
            .map_err(resource_error)?;
        access
            .validate_read_result(&sandbox, &target, ResourceCapability::ViewDetails)
            .await
            .map_err(access_error)?;
        self.finish_read(&authorized, id, ResourceCapability::ViewDetails)
            .await?;
        Ok(AgentResourceReading {
            agent_id: id.clone(),
            sandbox_id: agent.sandbox_id.clone(),
            reading,
        })
    }

    pub async fn usage_authorized(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &AgentId,
    ) -> Result<AgentUsageReading, AgentReadError> {
        let authorized = self
            .authorized(context, credential, id, ResourceCapability::ViewDetails)
            .await?;
        let agent = authorized
            .snapshot
            .lineage
            .agent
            .as_ref()
            .ok_or_else(|| corrupt("authorized Agent snapshot has no Agent"))?;
        require_granted(agent, crate::RuntimeCapability::Usage)?;
        let entries = self.complete_log(agent).await?;
        let usage = fold_agent_log(agent, &entries)
            .map_err(AgentReadError::Corrupt)?
            .usage;
        self.finish_read(&authorized, id, ResourceCapability::ViewDetails)
            .await?;
        Ok(usage)
    }

    pub(super) async fn authorized(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &AgentId,
        capability: ResourceCapability,
    ) -> Result<AuthorizedResource, AgentReadError> {
        if ![
            Capability::UseOwnSandboxes,
            Capability::UseBindingConversations,
            Capability::ReadAllSandboxes,
            Capability::ControlAnySandbox,
        ]
        .into_iter()
        .any(|capability| context.caller.authorization.allows(capability))
        {
            return Err(AgentReadError::Forbidden);
        }
        self.access
            .as_ref()
            .ok_or_else(|| {
                AgentReadError::Unavailable("resource access service is not composed".into())
            })?
            .authorize(
                context,
                credential,
                &ResourceTarget::Agent {
                    agent_id: id.clone(),
                },
                capability,
                AccessPurpose::Read,
            )
            .await
            .map_err(access_error)
    }
    pub(super) async fn finish_read(
        &self,
        authorized: &AuthorizedResource,
        id: &AgentId,
        capability: ResourceCapability,
    ) -> Result<(), AgentReadError> {
        self.access
            .as_ref()
            .ok_or_else(|| {
                AgentReadError::Unavailable("resource access service is not composed".into())
            })?
            .validate_read_result(
                authorized,
                &ResourceTarget::Agent {
                    agent_id: id.clone(),
                },
                capability,
            )
            .await
            .map_err(access_error)
    }
}

pub(super) fn is_personal(authorized: &AuthorizedResource, context: &CallerContext) -> bool {
    if authorized.binding_authority().is_some() {
        return false;
    }
    match &authorized.snapshot.policy {
        Some(policy) => {
            matches!(&policy.owner, ResourceOwner::Person { subject } if subject == &context.caller.subject)
        }
        None => authorized.snapshot.lineage.sandbox.owner == context.caller.subject,
    }
}

pub(super) fn lineage(value: ResourceLineage) -> Result<AgentLineage, AgentReadError> {
    Ok(AgentLineage {
        agent: value
            .agent
            .ok_or_else(|| corrupt("Agent lineage is missing its Agent"))?,
        worktree: value
            .worktree
            .ok_or_else(|| corrupt("Agent lineage is missing its Worktree"))?,
        workspace: value
            .workspace
            .ok_or_else(|| corrupt("Agent lineage is missing its Workspace"))?,
        sandbox: value.sandbox,
    })
}
pub(super) fn access_error(error: ResourceAccessError) -> AgentReadError {
    match error {
        ResourceAccessError::Access(AccessError::NotFound)
        | ResourceAccessError::Store(ResourceAccessStoreError::NotFound) => {
            AgentReadError::NotFound
        }
        ResourceAccessError::Access(AccessError::Forbidden) => AgentReadError::Forbidden,
        error => AgentReadError::Access(error),
    }
}

fn detail_projection(
    authorized: AuthorizedResource,
    context: &CallerContext,
) -> Result<AgentDetailReading, AgentReadError> {
    authorized
        .decision
        .require(ResourceCapability::ViewDetails)
        .map_err(|error| access_error(error.into()))?;
    let personal = is_personal(&authorized, context);
    let lineage = lineage(authorized.snapshot.lineage)?;
    if personal {
        return Ok(AgentDetailReading::Personal(Box::new(lineage)));
    }
    let agent = lineage.agent;
    Ok(AgentDetailReading::Shared(Box::new(SharedAgentDetail {
        model: agent.selected_model(),
        id: agent.id,
        status: agent.status,
        title: agent.conversation.preferences.title.or(agent.title),
        created_at: agent.created_at,
        updated_at: agent.updated_at,
        ready_at: agent.ready_at,
        stopped_at: agent.stopped_at,
        deleted_at: agent.deleted_at,
    })))
}
