use super::*;
use crate::resource_access::{
    AuthorizedResource, CallerContext, ResourceAccessSnapshot, ResourceAccessStoreError,
};
use std::{future::Future, time::Duration};

const IO_BOUND: Duration = Duration::from_secs(15);

/// Opaque, nonrenewing read authority. Shared observation retains only this
/// subscriber's selected credential for fresh membership checks; it is never
/// serialized, shared between callers, or used to extend the original deadline.
#[derive(Clone)]
pub struct AgentEventStreamAuthorization {
    held: Arc<AuthorizedResource>,
    pub(in crate::sandbox) id: AgentId,
    personal_caller: Option<Caller>,
    fresh_principal: Option<Arc<(CallerContext, Option<String>)>>,
    pub(in crate::sandbox) observing: Option<u64>,
}

impl AgentEventStreamAuthorization {
    pub(super) fn personal(held: AuthorizedResource, caller: Caller, id: AgentId) -> Self {
        Self {
            held: Arc::new(held),
            id,
            personal_caller: Some(caller),
            fresh_principal: None,
            observing: None,
        }
    }
    pub fn incarnation(&self) -> Option<&str> {
        self.held
            .snapshot
            .lineage
            .agent
            .as_ref()
            .map(|agent| agent.created_at.as_str())
    }
    pub(crate) fn resource(&self) -> (&AuthorizedResource, &AgentId, bool) {
        (&self.held, &self.id, self.personal_caller.is_some())
    }

    /// Personal ownership is independent of recipient grants. Shared and
    /// organization decisions still require a fresh release fence.
    pub fn is_personal_owner(&self) -> bool {
        self.personal_caller.is_some()
    }

    /// Use the owner's independently fenced durable read path after losing
    /// shared read custody. Keep the original caller, lineage, and grant.
    pub fn use_personal_durable_reads(&mut self) -> bool {
        self.is_personal_owner() && self.observing.take().is_some()
    }

    pub fn can_review_external_approval(&self) -> bool {
        self.is_personal_owner() && self.held.decision.allows(ResourceCapability::Interact)
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
#[derive(serde::Serialize)]
pub struct AgentEventStreamSnapshot {
    /// Exact verified source identity for pure fold reuse; never authority.
    pub source_generation: Option<Vec<u8>>,
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
            observing: None,
            fresh_principal: self
                .observation
                .as_ref()
                .map(|_| Arc::new((context.clone(), credential.map(str::to_owned)))),
            personal_caller: authorized::is_personal(&held, context)
                .then(|| context.caller.clone()),
            held: Arc::new(held),
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

    /// A live batch retains its existing policy observation, while account
    /// deletion is checked again before each buffered fragment is released.
    /// This reads only standing and never loads the resource aggregate.
    pub async fn validate_event_stream_standing(
        &self,
        authorization: &AgentEventStreamAuthorization,
    ) -> Result<(), AgentReadError> {
        authorization
            .bounded(async {
                let access = self.access.as_ref().ok_or_else(|| {
                    AgentReadError::Unavailable("resource access is absent".into())
                })?;
                access
                    .validate_read_standing(
                        &authorization.held,
                        &ResourceTarget::Agent {
                            agent_id: authorization.id.clone(),
                        },
                    )
                    .await
                    .map_err(authorized::access_error)
            })
            .await
    }

    pub(super) async fn current_event_stream_snapshot(
        &self,
        authorization: &AgentEventStreamAuthorization,
    ) -> Result<ResourceAccessSnapshot, AgentReadError> {
        if authorization.observing.is_some() {
            return self
                .observation
                .as_ref()
                .ok_or_else(|| AgentReadError::Unavailable("observation service is absent".into()))?
                .fresh(authorization)
                .await;
        }
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
        Self::validate_observation_identity(authorization, &snapshot)?;
        Ok(snapshot)
    }

    pub fn observe_event_stream(
        &self,
        authorization: &mut AgentEventStreamAuthorization,
    ) -> Result<Option<super::super::agent_observation::AgentObservationSubscription>, AgentReadError>
    {
        if super::super::agent_observation::durable_read_fallback() {
            return Ok(None);
        }
        let Some(service) = &self.observation else {
            return Ok(None);
        };
        let subscription = service.subscribe(authorization.clone())?;
        authorization.observing = Some(subscription.id());
        Ok(Some(subscription))
    }

    pub(in crate::sandbox) async fn validate_observation_membership(
        &self,
        authorization: &AgentEventStreamAuthorization,
    ) -> Result<(), AgentReadError> {
        let Some(principal) = &authorization.fresh_principal else {
            return authorization.require();
        };
        let access = self
            .access
            .as_ref()
            .ok_or_else(|| AgentReadError::Unavailable("resource access absent".into()))?;
        authorization
            .bounded(async {
                access
                    .observation_memberships(
                        &authorization.held,
                        &ResourceTarget::Agent {
                            agent_id: authorization.id.clone(),
                        },
                        &principal.0,
                        principal.1.as_deref(),
                    )
                    .await
                    .map_err(authorized::access_error)
            })
            .await
    }

    pub(in crate::sandbox) async fn validate_observation_snapshot(
        &self,
        authorization: &AgentEventStreamAuthorization,
        snapshot: &ResourceAccessSnapshot,
    ) -> Result<(), AgentReadError> {
        authorization.require()?;
        let access = self
            .access
            .as_ref()
            .ok_or_else(|| AgentReadError::Unavailable("resource access is absent".into()))?;
        let target = ResourceTarget::Agent {
            agent_id: authorization.id.clone(),
        };
        if let Some(caller) = &authorization.personal_caller {
            let crate::resource_access::ResourceOwner::Person { subject } = snapshot
                .scope(&target)
                .map_err(|e| authorized::access_error(e.into()))?
                .owner
            else {
                return Err(AgentReadError::NotFound);
            };
            AccessDecision::personal(caller, &subject)
                .require(ResourceCapability::ViewHistory)
                .map_err(|e| authorized::access_error(e.into()))?;
        } else {
            access
                .validate_current_read_snapshot(
                    &authorization.held,
                    &target,
                    ResourceCapability::ViewHistory,
                    snapshot,
                )
                .await
                .map_err(authorized::access_error)?;
        }
        Self::validate_observation_identity(authorization, snapshot)?;
        authorization.require()
    }

    fn validate_observation_identity(
        authorization: &AgentEventStreamAuthorization,
        snapshot: &ResourceAccessSnapshot,
    ) -> Result<(), AgentReadError> {
        let target = ResourceTarget::Agent {
            agent_id: authorization.id.clone(),
        };
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
        if identity(snapshot) != identity(original)
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
        Ok(())
    }
}
