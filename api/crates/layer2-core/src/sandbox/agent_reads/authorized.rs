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
    /// Public transcript with certified segmented dispatch and legacy compatibility.
    /// Segmented proof refusal never falls back to a complete legacy scan.
    pub async fn transcript_bounded(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<AgentTranscript, AgentReadError> {
        let (lineage, held) = self
            .owner_projection_authority(caller, id, crate::RuntimeCapability::Conversation)
            .await?;
        let mut result = self.read_transcript(&lineage.agent).await?;
        crate::sandbox::sanitize_agent_transcript(&mut result);
        self.finish_owner_projection(
            caller,
            id,
            &lineage,
            held,
            crate::RuntimeCapability::Conversation,
        )
        .await?;
        Ok(result)
    }

    /// Public usage with captured-source dispatch and final owner authority.
    pub async fn usage_bounded(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<AgentUsageReading, AgentReadError> {
        let (lineage, held) = self
            .owner_projection_authority(caller, id, crate::RuntimeCapability::Usage)
            .await?;
        let result = self.read_usage(&lineage.agent).await?;
        self.finish_owner_projection(caller, id, &lineage, held, crate::RuntimeCapability::Usage)
            .await?;
        Ok(result)
    }

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
        let mut transcript = self.read_transcript(agent).await?;
        crate::sandbox::sanitize_agent_transcript(&mut transcript);
        // Attribution/correlation metadata is not a history access grant. The
        // transcript projection includes conversation messages and outcomes only.
        if !is_personal(&authorized, context) {
            super::visibility::shared_transcript(&mut transcript);
        }
        self.finish_read(&authorized, id, ResourceCapability::ViewHistory)
            .await?;
        Ok(transcript)
    }

    pub(super) async fn read_transcript(
        &self,
        agent: &Agent,
    ) -> Result<AgentTranscript, AgentReadError> {
        if let Some(transcript) = self
            .store
            .agent_log_transcript_projection(agent)
            .await
            .map_err(|error| match error {
                // A complete transcript is one implicit page. The segmented
                // store refuses that page when it exceeds its result budget.
                SandboxStoreError::InvalidPage | SandboxStoreError::AgentLogCapacityExceeded => {
                    AgentReadError::TranscriptPaginationRequired
                }
                error => super::store_error(error),
            })?
        {
            return Ok(transcript);
        }
        let (entries, bounded) = match self.store.agent_log_transcript(agent).await {
            Ok(Some(entries)) => (entries, true),
            Ok(None) => (self.complete_log(agent).await?, false),
            Err(error) => return Err(super::store_error(error)),
        };
        match fold_agent_log(agent, &entries) {
            Ok(projection) => Ok(projection.transcript),
            Err(_error) if bounded => {
                let complete = self.complete_log(agent).await?;
                fold_agent_log(agent, &complete)
                    .map(|projection| projection.transcript)
                    .map_err(AgentReadError::Corrupt)
            }
            Err(error) => Err(AgentReadError::Corrupt(error)),
        }
    }

    pub(super) async fn read_usage(
        &self,
        agent: &Agent,
    ) -> Result<AgentUsageReading, AgentReadError> {
        if let Some(reading) = self
            .store
            .agent_log_usage_projection(agent)
            .await
            .map_err(super::store_error)?
        {
            return Ok(reading);
        }
        if let Some(entries) = self
            .store
            .agent_log_usage(agent)
            .await
            .map_err(super::store_error)?
        {
            match fold_agent_usage(agent, &entries) {
                Ok(reading) => return Ok(reading),
                Err(_) => {
                    // A selected-row projection is an optimization boundary,
                    // not a second semantic authority. If its proof or fold
                    // disagrees with the exact history, retry the established
                    // complete compatibility read.
                }
            }
        }
        let entries = self.complete_log(agent).await?;
        fold_agent_log(agent, &entries)
            .map(|projection| projection.usage)
            .map_err(AgentReadError::Corrupt)
    }

    pub async fn log_authorized(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &AgentId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<AgentVisibleLogEntry>, AgentReadError> {
        self.log_page_authorized(context, credential, id, after, limit)
            .await
            .map(|(entries, _)| entries)
    }

    pub async fn log_page_authorized(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &AgentId,
        after: u64,
        limit: u16,
    ) -> Result<(Vec<AgentVisibleLogEntry>, Option<u64>), AgentReadError> {
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
        let entries = self.read_log_page(agent, after, limit).await?;
        let next = super::log_pages::next_cursor(&entries, agent.conversation.last_seq, limit);
        let personal = is_personal(&authorized, context);
        let page = entries
            .into_iter()
            .map(|entry| AgentVisibleLogEntry::from_entry(entry, personal))
            .collect();
        self.finish_read(&authorized, id, ResourceCapability::ViewHistory)
            .await?;
        Ok((page, next))
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
        let usage = self.read_usage(agent).await?;
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

    pub(super) async fn finish_conversation_read(
        &self,
        caller: &Caller,
        source: &Agent,
        held: Option<&AuthorizedResource>,
    ) -> Result<Agent, AgentReadError> {
        let current = if let Some(held) = held {
            self.access
                .as_ref()
                .ok_or_else(|| {
                    AgentReadError::Unavailable("resource access service is not composed".into())
                })?
                .current_read_snapshot(
                    held,
                    &ResourceTarget::Agent {
                        agent_id: source.id.clone(),
                    },
                    ResourceCapability::ViewHistory,
                )
                .await
                .map_err(access_error)?
                .lineage
                .agent
                .ok_or_else(|| corrupt("Agent is absent at release"))?
        } else {
            self.owned_lineage(caller, &source.id).await?.agent
        };
        require_granted(&current, crate::RuntimeCapability::Conversation)?;
        if current.id != source.id
            || current.created_at != source.created_at
            || current.sandbox_id != source.sandbox_id
            || current.workspace_id != source.workspace_id
            || current.worktree_id != source.worktree_id
            || current.creation_worktree_id() != source.creation_worktree_id()
            || current.owner != source.owner
        {
            return Err(AgentReadError::Unavailable(
                "conversation source changed".into(),
            ));
        }
        Ok(current)
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
