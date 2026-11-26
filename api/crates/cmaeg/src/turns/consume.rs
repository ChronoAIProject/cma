use super::{
    execution_receipt,
    outcomes::{DeliveryCommand, TurnOutcome},
    projection::Projection,
};
use crate::{
    delivery::router::DeliverySink,
    ingress::launch::{LaunchOutcome, LaunchRequest},
    providers::{ProviderClient, TurnStream},
    telemetry::turn_trace::{scope, BoundOperation, Outcome, Stage},
};
use cmaeg_core::problem::{Problem, ProblemCode};
use std::sync::Arc;
use tokio::sync::oneshot;

/// Project `stream` into delivery commands. `provider` labels the acceptance
/// metric with the serving consumer's provider.
pub(super) async fn consume(
    mut stream: TurnStream,
    turn: Arc<LaunchRequest>,
    delivery: &dyn DeliverySink,
    activity: &dyn super::observer::ActivityObserver,
    providers: &ProviderClient,
    recovery_authority: Option<&Arc<super::RecoveryAuthority>>,
    admitted: &mut Option<oneshot::Sender<LaunchOutcome>>,
) -> Result<&'static str, &'static str> {
    // No provider acceptance observed yet.
    let mut acceptance_pending = true;
    // The acceptance signal can still arrive. A stream that ends without one
    // closes it, while any terminal answer it sent remains queued as an event.
    let mut acceptance_open = true;
    let mut accepted_response = None;
    let mut native_agent_id = None;
    let mut projection = Projection::default();
    let mut trace = None;
    let provider = &turn
        .channel
        .definition
        .consumer(turn.consumer.as_ref())
        .ok_or("provider_unavailable")?
        .provider
        .slug;
    let execution_verifier = providers.execution_verifiers.verifier(provider.as_str());
    loop {
        tokio::select! {
            biased;
            accepted = &mut stream.accepted, if acceptance_open => {
                acceptance_open = false;
                // Drain the events: an ended stream is lost only if it never
                // delivered a terminal answer.
                let Ok(accepted) = accepted else { continue };
                acceptance_pending = false;
                adopt_trace(&stream, &turn, &mut trace);
                metrics::histogram!(crate::telemetry::names::PROVIDER_ACCEPT,
                    crate::telemetry::names::PROVIDER => provider.as_str().to_owned())
                    .record(accepted.at.saturating_duration_since(stream.request_started).as_secs_f64());
                tracing::info!(metric="cmaeg_provider_accept_seconds",duration_seconds=accepted.at.saturating_duration_since(stream.request_started).as_secs_f64(),"provider accepted turn");
                if let Some(sender) = admitted.take() {let _ = sender.send(LaunchOutcome::Accepted {response_id:accepted.response_id.clone()});}
                accepted_response = Some(accepted.response_id.clone());
                delivery.submit(DeliveryCommand::StreamOutcome {
                    turn: turn.clone(),
                    outcome: TurnOutcome::Accepted {
                        response_id: accepted.response_id,
                        thread_reply: accepted.thread_reply,
                    },
                    accepted_deadline: recovery_authority.map(|authority| authority.deadline),
                    answer_response_id: None,
                    recovery: None,
                }).await.map_err(|_| "delivery_unavailable")?;
            }
            observed = stream.events.recv_observed() => {
                let (event, bound_to_accepted, recovery_gate) = match observed {
                    Some(observed) => (observed.event, observed.bound_to_accepted, None),
                    None => {
                        let response_id = accepted_response.as_deref().ok_or("stream_lost")?;
                        let recovery = stream.recovery.take().ok_or("stream_lost")?;
                        let authority = recovery_authority.ok_or("stream_lost")?;
                        let event = recovery
                            .terminal(providers, provider, response_id, native_agent_id.take(), authority)
                            .await?
                            .ok_or("stream_lost")?;
                        // The verified snapshot supplies the whole terminal payload.
                        projection = Projection::default();
                        (event, true, Some(authority.clone()))
                    }
                };
                adopt_trace(&stream, &turn, &mut trace);
                if let crate::providers::ResponsesEvent::InProgress {
                    native_agent_id: Some(agent_id),
                } = &event {
                    if bound_to_accepted {
                        native_agent_id.get_or_insert_with(|| agent_id.clone());
                    }
                }
                if let crate::providers::ResponsesEvent::Created {
                    response_id, native_agent_id: Some(agent_id), ..
                } = &event {
                    if bound_to_accepted {
                        native_agent_id.get_or_insert_with(|| agent_id.clone());
                    }
                    let mut change = crate::activity::Change::state(
                        turn.consumer.as_ref().map_or("default", |id| id.as_str()),
                        crate::activity::ConsumerState::Accepted, Some(response_id.clone()),
                    );
                    change.agent_id = Some(agent_id.clone());
                    activity.checkpoint(&turn.channel.definition.record_id,
                        &turn.activity.event_id, change).await;
                }
                let execution = execution_receipt::observe(
                    execution_verifier, &turn, provider.as_str(), accepted_response.as_deref(), &event,
                );
                if let Some(outcome) = projection.event(event)? {
                    let terminal = outcome.terminal();
                    let kind = outcome.kind();
                    let answer_response_id = if bound_to_accepted
                        && matches!(outcome, TurnOutcome::MessageCommitted { .. } | TurnOutcome::Completed { .. })
                    {
                        accepted_response.clone()
                    } else {
                        None
                    };
                    let delivery = send(delivery,&turn,outcome, answer_response_id, recovery_gate).await;
                    if terminal && acceptance_pending {
                        // The provider answered without a response identity.
                        // Once delivery holds that answer the event is handled:
                        // refusing it would invite the source to redeliver an
                        // event whose answer the user already has.
                        if delivery.is_ok() {
                            if let Some(sender) = admitted.take() {let _ = sender.send(LaunchOutcome::Deferred);}
                        } else {
                            refuse(admitted,ProblemCode::ProviderUnavailable,"provider stream ended before acceptance");
                        }
                    }
                    if let Some(change) = execution {
                        activity.checkpoint(&turn.channel.definition.record_id,
                            &turn.activity.event_id, change).await;
                    }
                    delivery?;
                    if terminal {
                        if let Some(trace) = trace {trace.finish(Outcome::Streamed);}
                        return Ok(kind);
                    }
                }
            }
        }
    }
}

fn adopt_trace(stream: &TurnStream, turn: &LaunchRequest, trace: &mut Option<BoundOperation>) {
    if trace.is_some() {
        return;
    }
    let Some(association) = stream.trace.as_ref().and_then(|slot| slot.get()).copied() else {
        return;
    };
    if let Some(operation) = BoundOperation::start(Some(association), Stage::Channel) {
        scope::attach(&turn.trace, operation.context());
        *trace = Some(operation);
    }
}
async fn send(
    delivery: &dyn DeliverySink,
    turn: &Arc<LaunchRequest>,
    outcome: TurnOutcome,
    answer_response_id: Option<String>,
    recovery: Option<Arc<super::RecoveryAuthority>>,
) -> Result<(), &'static str> {
    delivery
        .submit(DeliveryCommand::StreamOutcome {
            turn: turn.clone(),
            outcome,
            answer_response_id,
            accepted_deadline: None,
            recovery,
        })
        .await
        .map_err(|_| "delivery_unavailable")
}
pub(super) fn refuse(
    admitted: &mut Option<oneshot::Sender<LaunchOutcome>>,
    code: ProblemCode,
    detail: &str,
) {
    if let Some(sender) = admitted.take() {
        let _ = sender.send(LaunchOutcome::Refused(Problem::new(code, 503, detail)));
    }
}
