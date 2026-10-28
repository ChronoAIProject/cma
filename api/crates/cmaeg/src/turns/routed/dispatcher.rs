use super::{journal::valid_response, model::*, Journal};
use crate::{
    channels::resolver::{ChannelResolver, ResolveError, ResolvedChannel},
    ingress::routing_store::{
        AdmissionError, AdmittedPlan, EventIdentity, PlanStore, StoredTarget,
    },
};
use cmaeg_core::channel::{ProviderBinding, Secret};
use futures_util::future::BoxFuture;
use std::sync::Arc;

pub trait Authority: Send + Sync {
    /// Must bypass process caches and authenticate the current sealed definition.
    fn current<'a>(
        &'a self,
        identity: &'a EventIdentity,
    ) -> BoxFuture<'a, Result<Arc<ResolvedChannel>, ExecutionError>>;
    fn native_control<'a>(
        &'a self,
        _current: &'a ResolvedChannel,
        plan: &'a AdmittedPlan,
        _target: &'a StoredTarget,
    ) -> BoxFuture<'a, Result<(), ExecutionError>> {
        Box::pin(async move {
            if native_lark_control(plan)
                || crate::whatsapp::message_context::shape(&plan.activity)
                || crate::interactions::whatsapp_flow::shape(&plan.activity)
            {
                Err(ExecutionError::AuthorityChanged)
            } else {
                Ok(())
            }
        })
    }
}
impl Authority for ChannelResolver {
    fn native_control<'a>(
        &'a self,
        current: &'a ResolvedChannel,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
    ) -> BoxFuture<'a, Result<(), ExecutionError>> {
        Box::pin(async move {
            if native_lark_control(plan) {
                self.native_lark_control_current(current, &plan.activity, &target.launch.event)
                    .await
                    .map_err(|e| match e {
                        crate::interactions::Error::Denied => ExecutionError::AuthorityChanged,
                        crate::interactions::Error::Unavailable => ExecutionError::Unavailable,
                    })?;
            }
            if crate::interactions::whatsapp_flow::shape(&plan.activity) {
                self.whatsapp_flow_current(current, &plan.activity, &target.launch.event)
                    .await
                    .map_err(|e| match e {
                        crate::interactions::Error::Denied => ExecutionError::AuthorityChanged,
                        crate::interactions::Error::Unavailable => ExecutionError::Unavailable,
                    })?;
            }
            if crate::whatsapp::message_context::shape(&plan.activity) {
                self.whatsapp_message_change_current(current, plan)
                    .await
                    .map_err(|error| match error {
                        crate::whatsapp::message_context::Error::Unavailable => {
                            ExecutionError::Unavailable
                        }
                        _ => ExecutionError::AuthorityChanged,
                    })?;
            }
            Ok(())
        })
    }
    fn current<'a>(
        &'a self,
        identity: &'a EventIdentity,
    ) -> BoxFuture<'a, Result<Arc<ResolvedChannel>, ExecutionError>> {
        Box::pin(async move {
            self.resolve_current(identity.channel_id.as_str())
                .await
                .map_err(|error| match error {
                    ResolveError::Unavailable | ResolveError::RateLimited => {
                        ExecutionError::Unavailable
                    }
                    _ => ExecutionError::AuthorityChanged,
                })
        })
    }
}

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
    fn start<'a>(
        &'a self,
        body: &'a FrozenTurn,
        binding: &'a ProviderBinding,
        credential: &'a Secret,
    ) -> BoxFuture<'a, StartResult>;
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
        let result = self.run(&plan, target, &mut lease).await;
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
        current_target(self.authority.as_ref(), plan, target).await
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
            // Reader-first release: never fall through to legacy conversation
            // creation for an admission requiring preparation we cannot perform.
            if plan.prepares_binding(target) {
                return Err(ExecutionError::Unsupported);
            }
            let body = self.port.prepare(plan, target, &current).await?;
            validate_delivery(&body, &current, target)?;
            self.journal.freeze(lease, plan, target, &body).await?;
        }
        let body = self.journal.body(lease).await?;
        body.validate(plan, target, &self.journal.keys, (self.journal.clock)())?;
        // Media/conversation preparation has its own bound. Renew the still
        // owned lease so it cannot consume the subsequent acceptance budget.
        self.journal.renew(lease).await?;
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

pub(super) async fn current_target(
    authority: &dyn Authority,
    plan: &AdmittedPlan,
    target: &StoredTarget,
) -> Result<Arc<ResolvedChannel>, ExecutionError> {
    let current = authority.current(&plan.identity).await?;
    validate_current(&current, plan, target)?;
    authority.native_control(&current, plan, target).await?;
    Ok(current)
}

pub(super) fn validate_current(
    current: &ResolvedChannel,
    plan: &AdmittedPlan,
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
    if matches!(
        target.launch.event.reply,
        cmaeg_core::seal::ReplyAddress::WhatsappGroup(_)
    ) && crate::whatsapp::credentials(&current.definition, &target.launch.event.reply).is_none()
    {
        return Err(ExecutionError::AuthorityChanged);
    }
    let consumer = current
        .definition
        .consumer(Some(&target.launch.consumer))
        .ok_or(ExecutionError::AuthorityChanged)?;
    if current.id != plan.identity.channel_id
        || current.pointer.record_id != plan.identity.record_id
        || current.definition.record_id != plan.identity.record_id
        || current.definition.owner.subject != plan.identity.owner
        || current.definition.routing.is_none()
        || !current
            .definition
            .source_of(&plan.activity.source)
            .is_some_and(|source| {
                if matches!(source, cmaeg_core::channel::Source::Whatsapp { .. })
                    && !crate::interactions::whatsapp_flow::shape(&plan.activity)
                    && !crate::whatsapp::message_context::shape(&plan.activity)
                {
                    crate::whatsapp::legacy_source(source)
                        == crate::whatsapp::legacy_source(&plan.source)
                } else if matches!(source, cmaeg_core::channel::Source::Telegram { .. }) {
                    crate::telegram::messaging_source(source)
                        == crate::telegram::messaging_source(&plan.source)
                } else {
                    crate::native_lark::same_messaging_source(source, &plan.source)
                }
            })
        || current
            .definition
            .creation
            .as_ref()
            .is_some_and(|creation| !creation.ready)
        || consumer.pending
        || consumer.credential.is_none()
        || consumer.provider != &target.launch.provider
    {
        return Err(ExecutionError::AuthorityChanged);
    }
    Ok(())
}

fn native_lark_control(plan: &AdmittedPlan) -> bool {
    matches!(
        plan.activity.source,
        cmaeg_core::activity::SourceKind::NativeLark(_)
    ) && matches!(
        plan.activity.kind,
        cmaeg_core::activity::ActivityKind::Interaction(_)
    )
}
