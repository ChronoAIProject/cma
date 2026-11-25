//! Explicit composition of journal execution with the opt-in provider transport.
//! This module installs no ingress, scheduler, delivery loop or retry policy.
use super::{dispatcher::current_target, *};
use crate::{
    channels::resolver::ResolvedChannel,
    ingress::routing_store::{AdmittedPlan, EventIdentity, PlanStore, StoredTarget},
    providers::{
        FrozenSubmission, ProviderClient, ProviderError, RecoveryIdentity, RecoverySnapshot,
        RecoveryStream,
    },
};
use cmaeg_core::channel::{ProviderBinding, Secret};
use futures_util::future::BoxFuture;
use std::sync::Arc;

/// Preparation is supplied separately so existing media/conversation preparation
/// can freeze its result once. It must not start a turn or alter a saved body.
pub trait FrozenTurnBuilder: Send + Sync {
    fn prepare<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        current: &'a ResolvedChannel,
    ) -> BoxFuture<'a, Result<FrozenTurn, ExecutionError>>;
    fn prepare_at_generation<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        current: &'a ResolvedChannel,
        generation: Option<u64>,
    ) -> BoxFuture<'a, Result<FrozenTurn, ExecutionError>> {
        if generation.is_some() {
            Box::pin(async { Err(ExecutionError::Unsupported) })
        } else {
            self.prepare(plan, target, current)
        }
    }
}

pub struct HttpExecutionPort {
    providers: Arc<ProviderClient>,
    builder: Arc<dyn FrozenTurnBuilder>,
    plans: PlanStore,
    journal: Journal,
    authority: Arc<dyn Authority>,
    media_work: Option<super::media_work::MediaWorkIndex>,
}
pub struct ReattachedResponse {
    pub snapshot: RecoverySnapshot,
    pub stream: RecoveryStream,
}
#[derive(Debug)]
pub enum ReattachError {
    Execution(ExecutionError),
    Provider(ProviderError),
}
impl From<ExecutionError> for ReattachError {
    fn from(error: ExecutionError) -> Self {
        Self::Execution(error)
    }
}
impl From<ProviderError> for ReattachError {
    fn from(error: ProviderError) -> Self {
        Self::Provider(error)
    }
}

impl HttpExecutionPort {
    pub fn new(
        providers: Arc<ProviderClient>,
        builder: Arc<dyn FrozenTurnBuilder>,
        plans: PlanStore,
        journal: Journal,
        authority: Arc<dyn Authority>,
    ) -> Self {
        Self {
            providers,
            builder,
            plans,
            journal,
            authority,
            media_work: None,
        }
    }
    pub fn with_media_continuations(mut self, enabled: bool) -> Self {
        if enabled {
            self.media_work = Some(super::media_work::MediaWorkIndex::new(
                self.journal.clone(),
                self.plans.clone(),
            ));
        }
        self
    }
    /// The dispatcher shares this adapter's exact authority and storage handles.
    pub fn dispatcher(self: Arc<Self>) -> Dispatcher {
        Dispatcher::new(
            self.plans.clone(),
            self.journal.clone(),
            self.authority.clone(),
            self,
        )
    }
    /// Reattachment reloads the committed target and authenticated frozen body.
    /// Neither response ID nor cursor comes from an untrusted call argument.
    pub async fn reattach(
        &self,
        identity: &EventIdentity,
        target_id: &str,
        lease: &Lease,
        native_agent_id: Option<&str>,
    ) -> Result<ReattachedResponse, ReattachError> {
        self.journal.guard(lease).await?;
        let plan = self
            .plans
            .load(identity)
            .await
            .map_err(super::dispatcher::admission_error)?
            .ok_or(ExecutionError::Invalid)?;
        let target = plan
            .targets
            .iter()
            .find(|target| target.id == target_id)
            .ok_or(ExecutionError::Invalid)?;
        let state = lease.state();
        if state.id != target.id
            || state.root != plan.admission_id
            || state.owner != plan.identity.owner
            || state.request_hash != plan.request_hash
            || state.retained_until != plan.retained_until
            || state.event_expires_at != target.launch.event.expires_at
        {
            return Err(ExecutionError::Invalid.into());
        }
        let Phase::Accepted {
            response_id,
            durable_cursor,
        } = &state.phase
        else {
            return Err(ExecutionError::Invalid.into());
        };
        let body = self.journal.body(lease).await?;
        let mut expected = RecoveryIdentity {
            response_id: response_id.clone(),
            native_agent_id: native_agent_id.map(str::to_owned),
            metadata: body.metadata.clone(),
        };
        let current = current_target(self.authority.as_ref(), &plan, target).await?;
        self.journal.guard(lease).await?;
        let current_credential = credential(&current, target)?;
        let snapshot = self
            .providers
            .response_snapshot_with_trace(
                &target.launch.provider.slug,
                &expected,
                current_credential,
                self.providers.nyxid_trace_contract(&target.launch.provider),
            )
            .await?;
        expected.native_agent_id = Some(snapshot.native_agent_id.clone());
        // Rotate/revoke between snapshot and replay is respected, rather than
        // retaining the snapshot request's credential for the next HTTP call.
        let current = current_target(self.authority.as_ref(), &plan, target).await?;
        self.journal.guard(lease).await?;
        let stream = self
            .providers
            .replay_response_with_trace(
                &target.launch.provider.slug,
                &expected,
                *durable_cursor,
                credential(&current, target)?,
                self.providers.nyxid_trace_contract(&target.launch.provider),
            )
            .await?;
        // No checkpoint advances here. A delivery worker must guard/renew its
        // lease and confirm durable effects before acknowledging any frame.
        Ok(ReattachedResponse { snapshot, stream })
    }
}
pub(super) fn credential<'a>(
    current: &'a ResolvedChannel,
    target: &StoredTarget,
) -> Result<&'a Secret, ExecutionError> {
    current
        .definition
        .consumer(Some(&target.launch.consumer))
        .and_then(|consumer| consumer.credential)
        .ok_or(ExecutionError::AuthorityChanged)
}
impl ExecutionPort for HttpExecutionPort {
    fn prepare_continuations<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        body: &'a FrozenTurn,
    ) -> BoxFuture<'a, Result<(), ExecutionError>> {
        Box::pin(async move {
            match &self.media_work {
                Some(index) => index.prepare(plan, target, body).await,
                None if body.metadata.contains_key(super::media_schedule::KEY) => {
                    Err(ExecutionError::Unsupported)
                }
                None => Ok(()),
            }
        })
    }
    fn support(&self, binding: &ProviderBinding) -> RecoverySupport {
        RecoverySupport {
            exact_body_idempotency: false,
            response_replay: self
                .providers
                .supports_response_replay(&binding.slug)
                .unwrap_or(false),
        }
    }
    fn prepare<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        current: &'a ResolvedChannel,
    ) -> BoxFuture<'a, Result<FrozenTurn, ExecutionError>> {
        self.builder.prepare(plan, target, current)
    }
    fn inspect_binding<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        current: &'a ResolvedChannel,
    ) -> BoxFuture<'a, Result<BindingPreparation, ExecutionError>> {
        Box::pin(async move {
            self.providers
                .inspect_binding_preparation(plan, target, credential(current, target)?)
                .await
                .map_err(super::builder::provider_error)
        })
    }
    fn reserve_binding<'a>(
        &'a self,
        target: &'a StoredTarget,
        current: &'a ResolvedChannel,
        intent: &'a PreparationIntent,
    ) -> BoxFuture<'a, Result<PreparationReceipt, ExecutionError>> {
        Box::pin(async move {
            self.providers
                .reserve_binding_preparation(
                    &target.launch.provider.slug,
                    &target.launch.provider.binding_id,
                    intent,
                    credential(current, target)?,
                )
                .await
                .map_err(super::builder::provider_error)
        })
    }
    fn prepare_at_generation<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        current: &'a ResolvedChannel,
        generation: Option<u64>,
    ) -> BoxFuture<'a, Result<FrozenTurn, ExecutionError>> {
        self.builder
            .prepare_at_generation(plan, target, current, generation)
    }
    fn start<'a>(
        &'a self,
        body: &'a FrozenTurn,
        binding: &'a ProviderBinding,
        credential: &'a Secret,
    ) -> BoxFuture<'a, StartResult> {
        self.start_observed(body, binding, credential, None)
    }
    fn start_observed<'a>(
        &'a self,
        body: &'a FrozenTurn,
        binding: &'a ProviderBinding,
        credential: &'a Secret,
        timing: Option<crate::telemetry::timing::IngressTiming>,
    ) -> BoxFuture<'a, StartResult> {
        Box::pin(async move {
            match self
                .providers
                .submit_frozen(
                    &binding.slug,
                    body.provider_body(),
                    &body.idempotency_key,
                    &body.metadata,
                    credential,
                    (self.providers.nyxid_trace_contract(binding), timing),
                )
                .await
            {
                FrozenSubmission::Accepted(response_id) => StartResult::Accepted { response_id },
                FrozenSubmission::NotAccepted => StartResult::NotAccepted,
                FrozenSubmission::NotAcceptedWithCode(code) => {
                    StartResult::NotAcceptedWithCode { code }
                }
                FrozenSubmission::Rejected(code) => StartResult::Rejected { code },
                FrozenSubmission::Unknown => StartResult::Unknown,
            }
        })
    }
}
