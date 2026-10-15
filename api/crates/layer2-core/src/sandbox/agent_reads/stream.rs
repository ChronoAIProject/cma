use super::*;
use crate::resource_access::{
    AuthorizedResource, CallerContext, ResourceAccessSnapshot, ResourceAccessStoreError,
};
use std::{future::Future, time::Duration};

const IO_BOUND: Duration = Duration::from_secs(15);

/// Opaque, nonrenewing read authority. Contains no request credential. A new
/// connection must authorize again with the actual requesting credential.
pub struct AgentEventStreamAuthorization {
    held: AuthorizedResource,
    id: AgentId,
    personal_caller: Option<Caller>,
}

impl AgentEventStreamAuthorization {
    pub(crate) fn resource(&self) -> (&AuthorizedResource, &AgentId, bool) {
        (&self.held, &self.id, self.personal_caller.is_some())
    }

    pub fn revalidation_deadline(&self) -> Option<tokio::time::Instant> {
        self.held
            .decision
            .revalidation_deadline(ResourceCapability::ViewHistory)
    }

    fn require(&self) -> Result<(), AgentReadError> {
        self.held
            .decision
            .require(ResourceCapability::ViewHistory)
            .map_err(|error| authorized::access_error(error.into()))
    }

    pub(in crate::sandbox) async fn bounded<T>(
        &self,
        operation: impl Future<Output = Result<T, AgentReadError>>,
    ) -> Result<T, AgentReadError> {
        self.require()?;
        let deadline = tokio::time::Instant::now() + IO_BOUND;
        let deadline = self
            .revalidation_deadline()
            .map_or(deadline, |expires| expires.min(deadline));
        let result = tokio::time::timeout_at(deadline, operation).await;
        self.require()?;
        result.map_err(|_| AgentReadError::Unavailable("event read timed out".into()))?
    }
}

/// Only Agent event state needed to validate live fragments, plus approved
/// history. Parent configuration, native conversation and credentials are absent.
pub struct AgentEventStreamSnapshot {
    pub id: AgentId,
    pub created_at: String,
    pub model: Option<String>,
    /// Immutable creation model, used only for legacy turns before settings edits.
    pub initial_model: Option<String>,
    pub runtime_generation: Option<u64>,
    pub active_turn: Option<u64>,
    pub head: u64,
    pub history: Vec<AgentVisibleLogEntry>,
}

impl AgentReadService {
    pub async fn authorize_event_stream(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &AgentId,
    ) -> Result<AgentEventStreamAuthorization, AgentReadError> {
        let held = tokio::time::timeout(
            IO_BOUND,
            self.authorized(context, credential, id, ResourceCapability::ViewHistory),
        )
        .await
        .map_err(|_| AgentReadError::Unavailable("event authorization timed out".into()))??;
        Ok(AgentEventStreamAuthorization {
            personal_caller: authorized::is_personal(&held, context)
                .then(|| context.caller.clone()),
            held,
            id: id.clone(),
        })
    }

    /// Called immediately before each frame is released, including frames that
    /// have waited under consumer backpressure. Any policy change closes the
    /// connection; a reconnect evaluates the remaining independent grants anew.
    pub async fn validate_event_stream(
        &self,
        authorization: &AgentEventStreamAuthorization,
    ) -> Result<(), AgentReadError> {
        authorization
            .bounded(self.current_event_stream_snapshot(authorization))
            .await
            .map(|_| ())
    }

    pub async fn event_stream_snapshot(
        &self,
        authorization: &AgentEventStreamAuthorization,
    ) -> Result<AgentEventStreamSnapshot, AgentReadError> {
        let result = authorization
            .bounded(self.read_event_stream_snapshot(authorization))
            .await;
        // Private dependency failures and timeouts must pass the same fence as
        // successful buffered history. No error may extend expired authority.
        self.validate_event_stream(authorization).await?;
        result
    }

    async fn read_event_stream_snapshot(
        &self,
        authorization: &AgentEventStreamAuthorization,
    ) -> Result<AgentEventStreamSnapshot, AgentReadError> {
        let snapshot = self.current_event_stream_snapshot(authorization).await?;
        let agent = snapshot
            .lineage
            .agent
            .ok_or_else(|| corrupt("authorized event snapshot has no Agent"))?;
        let entries = self.complete_log(&agent).await?;
        fold_agent_log(&agent, &entries).map_err(AgentReadError::Corrupt)?;
        Ok(AgentEventStreamSnapshot {
            model: agent.selected_model(),
            id: agent.id,
            created_at: agent.created_at,
            initial_model: agent.profile.content.model,
            runtime_generation: agent.conversation.runtime_generation,
            active_turn: agent.conversation.active_turn,
            head: agent.conversation.last_seq,
            history: entries
                .into_iter()
                .map(|entry| {
                    AgentVisibleLogEntry::from_entry(entry, authorization.personal_caller.is_some())
                })
                .collect(),
        })
    }

    pub(super) async fn current_event_stream_snapshot(
        &self,
        authorization: &AgentEventStreamAuthorization,
    ) -> Result<ResourceAccessSnapshot, AgentReadError> {
        let access = self
            .access
            .as_ref()
            .ok_or_else(|| AgentReadError::Unavailable("resource access is absent".into()))?;
        let target = ResourceTarget::Agent {
            agent_id: authorization.id.clone(),
        };
        // The owner's independent stream survives recipient grant changes.
        // Reuse the actual authenticated caller and verify current owner kind;
        // never construct a caller from the resource's recorded owner.
        let snapshot = if let Some(caller) = &authorization.personal_caller {
            access
                .authorize_personal(caller, &target, ResourceCapability::ViewHistory)
                .await
                .map(|value| value.snapshot)
        } else {
            access
                .current_read_snapshot(
                    &authorization.held,
                    &target,
                    ResourceCapability::ViewHistory,
                )
                .await
        }
        .map_err(authorized::access_error)?;
        let original = &authorization.held.snapshot;
        let identity = |value: &ResourceAccessSnapshot| {
            (
                value.lineage.sandbox.id.clone(),
                value.lineage.sandbox.created_at.clone(),
                value
                    .lineage
                    .workspace
                    .as_ref()
                    .map(|value| (value.id.clone(), value.created_at.clone())),
                value
                    .lineage
                    .worktree
                    .as_ref()
                    .map(|value| value.id.clone()),
                value
                    .lineage
                    .agent
                    .as_ref()
                    .map(|value| (value.id.clone(), value.created_at.clone())),
            )
        };
        if identity(&snapshot) != identity(original)
            || snapshot
                .scope(&target)
                .map_err(|error| authorized::access_error(error.into()))?
                != original
                    .scope(&target)
                    .map_err(|error| authorized::access_error(error.into()))?
        {
            return Err(authorized::access_error(
                ResourceAccessStoreError::Conflict.into(),
            ));
        }
        Ok(snapshot)
    }
}
