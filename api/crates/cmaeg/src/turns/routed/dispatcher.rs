pub use super::dispatcher_authority::Authority;
pub(super) use super::dispatcher_authority::{current_target, validate_current};
use super::{
    journal::valid_response, model::*, BindingPreparation, Journal, PreparationIntent,
    PreparationReceipt,
};
use crate::{
    channels::resolver::ResolvedChannel,
    ingress::routing_store::{
        AdmissionError, AdmittedPlan, EventIdentity, PlanStore, StoredTarget,
    },
};
use cmaeg_core::channel::{ProviderBinding, Secret};
use futures_util::future::BoxFuture;
use std::sync::Arc;

/// A direct, awaited transport: it must not spawn a detached start or re-render
/// any frozen field. Exact-body idempotency is opt-in only after provider proof.
pub trait ExecutionPort: Send + Sync {
    fn support(&self, binding: &ProviderBinding) -> RecoverySupport;
    /// Preparation may read/render, but must never start a provider turn.
    fn prepare<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        current: &'a ResolvedChannel,
    ) -> BoxFuture<'a, Result<FrozenTurn, ExecutionError>>;
    fn inspect_binding<'a>(
        &'a self,
        _plan: &'a AdmittedPlan,
        _target: &'a StoredTarget,
        _current: &'a ResolvedChannel,
    ) -> BoxFuture<'a, Result<BindingPreparation, ExecutionError>> {
        Box::pin(async { Err(ExecutionError::Unsupported) })
    }
    fn reserve_binding<'a>(
        &'a self,
        _target: &'a StoredTarget,
        _current: &'a ResolvedChannel,
        _intent: &'a PreparationIntent,
    ) -> BoxFuture<'a, Result<PreparationReceipt, ExecutionError>> {
        Box::pin(async { Err(ExecutionError::Unsupported) })
    }
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
    fn start<'a>(
        &'a self,
        body: &'a FrozenTurn,
        binding: &'a ProviderBinding,
        credential: &'a Secret,
    ) -> BoxFuture<'a, StartResult>;
    /// Persist independent continuation scheduling after immutable body freeze.
    /// Failure must prevent the original provider POST; a wake-up never grants
    /// permission to replay that original request.
    fn prepare_continuations<'a>(
        &'a self,
        _plan: &'a AdmittedPlan,
        _target: &'a StoredTarget,
        _body: &'a FrozenTurn,
    ) -> BoxFuture<'a, Result<(), ExecutionError>> {
        Box::pin(async { Ok(()) })
    }
}

pub enum StartResult {
    Accepted {
        response_id: String,
    },
    /// Proved no work was accepted, so the exact request can be retried.
    NotAccepted,
    /// A terminal rejection proved by the provider, never a network/5xx guess.
    Rejected {
        code: String,
    },
    Unknown,
}
pub enum DispatchOutcome {
    /// Ownership and exact body needed by a separately durable delivery worker.
    /// Known accepted responses are never submitted again.
    Replay {
        lease: Box<Lease>,
        body: Box<FrozenTurn>,
        response_id: String,
        durable_cursor: Option<u64>,
    },
    Unknown,
    Retry,
    Terminal(Phase),
}

pub struct Dispatcher {
    plans: PlanStore,
    journal: Journal,
    authority: Arc<dyn Authority>,
    port: Arc<dyn ExecutionPort>,
}
impl Dispatcher {
    pub fn new(
        plans: PlanStore,
        journal: Journal,
        authority: Arc<dyn Authority>,
        port: Arc<dyn ExecutionPort>,
    ) -> Self {
        Self {
            plans,
            journal,
            authority,
            port,
        }
    }
    pub fn journal(&self) -> &Journal {
        &self.journal
    }

    /// Only an ID belonging to the committed original plan can be dispatched.
    /// This never reroutes a retry against today's routing policy.
    pub async fn dispatch(
        &self,
        identity: &EventIdentity,
        target_id: &str,
        worker: &str,
    ) -> Result<DispatchOutcome, ExecutionError> {
        let plan = self
            .plans
            .load(identity)
            .await
            .map_err(admission_error)?
            .ok_or(ExecutionError::Invalid)?;
        let target = plan
            .targets
            .iter()
            .find(|target| target.id == target_id)
            .ok_or(ExecutionError::Invalid)?;
        let mut lease = self.journal.claim(&plan, target, worker).await?;
        let result = match self.run(&plan, target, &mut lease).await {
            Err(ExecutionError::Expired) => match self.journal.expire_pending(&mut lease).await {
                Ok(Some(phase)) => Ok(RunResult::Done(DispatchOutcome::Terminal(phase))),
                Ok(None) => Err(ExecutionError::Expired),
                Err(error) => Err(error),
            },
            result => result,
        };
        match result {
            Ok(RunResult::Replay {
                body,
                response_id,
                durable_cursor,
            }) => Ok(DispatchOutcome::Replay {
                lease: Box::new(lease),
                body,
                response_id,
                durable_cursor,
            }),
            Ok(RunResult::Done(outcome)) => {
                if let DispatchOutcome::Terminal(Phase::Refused { code }) = &outcome {
                    self.journal
                        .observe(
                            &plan.identity,
                            crate::activity::Change::refused(target.launch.consumer.as_str(), code),
                        )
                        .await;
                }
                self.journal.release(&mut lease).await?;
                Ok(outcome)
            }
            Err(error) => {
                let _ = self.journal.release(&mut lease).await;
                Err(error)
            }
        }
    }
    async fn current(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<Arc<ResolvedChannel>, ExecutionError> {
        let current = current_target(self.authority.as_ref(), plan, target).await?;
        if !plan.current_preparation_source(&current.definition, &self.journal.keys) {
            return Err(ExecutionError::AuthorityChanged);
        }
        Ok(current)
    }
    /// A retained checkpoint is the only retry authority. Never re-inspect
    /// after marker acknowledgement, including an ambiguous reservation reply.
    async fn prepare_binding(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        lease: &mut Lease,
    ) -> Result<Option<u64>, ExecutionError> {
        const BUDGET: std::time::Duration = std::time::Duration::from_secs(20);
        if lease.state.binding_preparation.is_none() {
            let current = self.current(plan, target).await?;
            self.journal.renew(lease).await?;
            self.journal.guard(lease).await?;
            let observed = tokio::time::timeout(BUDGET, async {
                use super::generation_provenance::GenerationProvenance;
                match self
                    .journal
                    .inherited_binding_generation(&self.plans, plan, target)
                    .await?
                {
                    GenerationProvenance::Fresh => {
                        self.port.inspect_binding(plan, target, &current).await
                    }
                    GenerationProvenance::Inherited {
                        original_admission_id,
                        original_target_id,
                        generation,
                    } => Ok(BindingPreparation::Inherited {
                        version: 1,
                        binding_id: target.launch.provider.binding_id.to_string(),
                        original_admission_id,
                        original_target_id,
                        generation,
                    }),
                }
            })
            .await
            .map_err(|_| ExecutionError::Unavailable)??;
            self.journal
                .pin_binding_preparation(lease, plan, target, observed)
                .await?;
        }
        if let Some(BindingPreparation::Pending { intent }) =
            lease.state.binding_preparation.clone()
        {
            // Any revocation, routing edit or credential rotation prevents the
            // mutation even when the original plan/marker remains durable.
            let current = self.current(plan, target).await?;
            self.journal.renew(lease).await?;
            self.journal.guard(lease).await?;
            if lease.state.event_expires_at <= (self.journal.clock)() {
                return Err(ExecutionError::Expired);
            }
            let receipt =
                tokio::time::timeout(BUDGET, self.port.reserve_binding(target, &current, &intent))
                    .await
                    .map_err(|_| ExecutionError::Unavailable)??;
            self.journal
                .record_binding_preparation(lease, plan, target, receipt)
                .await?;
        }
        lease
            .state
            .binding_preparation
            .as_ref()
            .ok_or(ExecutionError::Corrupt)?
            .resolved_generation()
    }

    async fn run(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        lease: &mut Lease,
    ) -> Result<RunResult, ExecutionError> {
        if lease.state.phase.terminal() {
            return Ok(RunResult::Done(DispatchOutcome::Terminal(
                lease.state.phase.clone(),
            )));
        }
        let support = self.port.support(&target.launch.provider);
        if let Phase::Accepted {
            response_id,
            durable_cursor,
        } = &lease.state.phase
        {
            // Retained read/replay may outlive the original event, but platform
            // delivery must still enforce its unchanged reference expiry.
            self.current(plan, target).await?;
            if !support.response_replay {
                return Err(ExecutionError::Unsupported);
            }
            return Ok(RunResult::Replay {
                response_id: response_id.clone(),
                durable_cursor: *durable_cursor,
                body: Box::new(self.journal.body(lease).await?),
            });
        }
        if lease.state.event_expires_at <= (self.journal.clock)() {
            return Err(ExecutionError::Expired);
        }
        if !support.response_replay {
            return Err(ExecutionError::Unsupported);
        }
        let current = self.current(plan, target).await?;
        if matches!(lease.state.phase, Phase::Starting | Phase::Unknown)
            && !support.exact_body_idempotency
        {
            self.journal.transition(lease, Phase::Unknown).await?;
            return Ok(RunResult::Done(DispatchOutcome::Unknown));
        }
        if lease.state.phase == Phase::Pending {
            let generation = if plan.prepares_binding(target) {
                self.prepare_binding(plan, target, lease).await?
            } else {
                None
            };
            let current = if plan.prepares_binding(target) {
                let current = self.current(plan, target).await?;
                self.journal.renew(lease).await?;
                self.journal.guard(lease).await?;
                current
            } else {
                current
            };
            let body = self
                .port
                .prepare_at_generation(plan, target, &current, generation)
                .await?;
            validate_delivery(&body, &current, target)?;
            self.journal.freeze(lease, plan, target, &body).await?;
        }
        let body = self.journal.body(lease).await?;
        body.validate(plan, target, &self.journal.keys, (self.journal.clock)())?;
        // Media/conversation preparation has its own bound. Renew the still
        // owned lease so it cannot consume the subsequent acceptance budget.
        self.journal.renew(lease).await?;
        self.port.prepare_continuations(plan, target, &body).await?;
        self.journal.guard(lease).await?;
        // The durable start marker precedes any network effect. If either CAS
        // confirmation or the fresh authority check fails, no POST is made.
        self.journal.transition(lease, Phase::Starting).await?;
        let current = self.current(plan, target).await?;
        self.journal.guard(lease).await?;
        if lease.state.event_expires_at <= (self.journal.clock)() {
            return Err(ExecutionError::Expired);
        }
        let credential = current
            .definition
            .consumer(Some(&target.launch.consumer))
            .and_then(|consumer| consumer.credential)
            .ok_or(ExecutionError::AuthorityChanged)?;
        let mut change = crate::activity::Change::state(
            target.launch.consumer.as_str(),
            crate::activity::ConsumerState::Dispatching,
            None,
        );
        change.provider = Some(target.launch.provider.slug.to_string());
        change.conversation_id = Some(target.launch.event.conversation_id.to_string());
        change.display_name = current
            .definition
            .consumer(Some(&target.launch.consumer))
            .map(|c| c.label.to_owned());
        self.journal.observe(&plan.identity, change).await;
        self.journal.guard(lease).await?;
        match self
            .port
            .start(&body, &target.launch.provider, credential)
            .await
        {
            StartResult::Accepted { response_id } if valid_response(&response_id) => {
                self.journal
                    .transition(
                        lease,
                        Phase::Accepted {
                            response_id: response_id.clone(),
                            durable_cursor: None,
                        },
                    )
                    .await?;
                self.journal
                    .observe(
                        &plan.identity,
                        crate::activity::Change::state(
                            target.launch.consumer.as_str(),
                            crate::activity::ConsumerState::Accepted,
                            Some(response_id.clone()),
                        ),
                    )
                    .await;
                if support.response_replay {
                    Ok(RunResult::Replay {
                        body: Box::new(body),
                        response_id,
                        durable_cursor: None,
                    })
                } else {
                    Err(ExecutionError::Unsupported)
                }
            }
            StartResult::NotAccepted => {
                self.journal.transition(lease, Phase::Prepared).await?;
                Ok(RunResult::Done(DispatchOutcome::Retry))
            }
            StartResult::Rejected { code } if !code.is_empty() && code.len() <= 128 => {
                let phase = Phase::Refused { code };
                self.journal.transition(lease, phase.clone()).await?;
                self.journal
                    .observe(
                        &plan.identity,
                        crate::activity::Change::state(
                            target.launch.consumer.as_str(),
                            crate::activity::ConsumerState::Refused,
                            None,
                        ),
                    )
                    .await;
                Ok(RunResult::Done(DispatchOutcome::Terminal(phase)))
            }
            _ => {
                self.journal.transition(lease, Phase::Unknown).await?;
                self.journal
                    .observe(
                        &plan.identity,
                        crate::activity::Change::state(
                            target.launch.consumer.as_str(),
                            crate::activity::ConsumerState::Unknown,
                            None,
                        ),
                    )
                    .await;
                Ok(RunResult::Done(DispatchOutcome::Unknown))
            }
        }
    }
}
enum RunResult {
    Replay {
        body: Box<FrozenTurn>,
        response_id: String,
        durable_cursor: Option<u64>,
    },
    Done(DispatchOutcome),
}
fn validate_delivery(
    body: &FrozenTurn,
    current: &ResolvedChannel,
    target: &StoredTarget,
) -> Result<(), ExecutionError> {
    if matches!(
        target.launch.event.reply,
        cmaeg_core::seal::ReplyAddress::NativeLark(_)
    ) && crate::native_lark::credentials(&current.definition, &target.launch.event.reply)
        .is_none()
    {
        return Err(ExecutionError::AuthorityChanged);
    }
    let consumer = current
        .definition
        .consumer(Some(&target.launch.consumer))
        .ok_or(ExecutionError::AuthorityChanged)?;
    if body.delivery.consumer != *consumer.id
        || body.delivery.label != consumer.label
        || body.delivery.auto_final != consumer.reply.auto_final
        || body.delivery.destination != consumer.reply.destination
        || body.delivery.policy != current.definition.reply
    {
        return Err(ExecutionError::Invalid);
    }
    Ok(())
}
pub(super) fn admission_error(error: AdmissionError) -> ExecutionError {
    match error {
        AdmissionError::Expired => ExecutionError::Expired,
        AdmissionError::Unavailable => ExecutionError::Unavailable,
        AdmissionError::Uncertain => ExecutionError::Uncertain,
        _ => ExecutionError::Corrupt,
    }
}
