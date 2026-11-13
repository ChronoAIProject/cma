use super::{
    outcomes::{DeliveryCommand, TurnOutcome},
    projection::Projection,
};
use crate::{
    delivery::router::DeliverySink,
    ingress::launch::{LaunchOutcome, LaunchRequest},
    providers::TurnStream,
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
    provider: &str,
    delivery: &dyn DeliverySink,
    activity: &dyn super::observer::ActivityObserver,
    admitted: &mut Option<oneshot::Sender<LaunchOutcome>>,
) -> Result<&'static str, &'static str> {
    let mut acceptance_pending = true;
    let mut projection = Projection::default();
    let mut trace = None;
    loop {
        tokio::select! {
            biased;
            accepted = &mut stream.accepted, if acceptance_pending => {
                acceptance_pending = false;
                let accepted = accepted.map_err(|_| "stream_lost")?;
                adopt_trace(&stream, &turn, &mut trace);
                metrics::histogram!(crate::telemetry::names::PROVIDER_ACCEPT,
                    crate::telemetry::names::PROVIDER => provider.to_owned())
                    .record(accepted.at.saturating_duration_since(stream.request_started).as_secs_f64());
                tracing::info!(metric="cmaeg_provider_accept_seconds",duration_seconds=accepted.at.saturating_duration_since(stream.request_started).as_secs_f64(),"provider accepted turn");
                if let Some(sender) = admitted.take() {let _ = sender.send(LaunchOutcome::Accepted {response_id:accepted.response_id.clone()});}
                send(delivery,&turn,TurnOutcome::Accepted {response_id:accepted.response_id,thread_reply:accepted.thread_reply}).await?;
            }
            event = stream.events.recv() => {
                let Some(event) = event else {return Err("stream_lost");};
                adopt_trace(&stream, &turn, &mut trace);
                if let crate::providers::ResponsesEvent::Created {
                    response_id, native_agent_id: Some(agent_id), ..
                } = &event {
                    let mut change = crate::activity::Change::state(
                        turn.consumer.as_ref().map_or("default", |id| id.as_str()),
                        crate::activity::ConsumerState::Accepted, Some(response_id.clone()),
                    );
                    change.agent_id = Some(agent_id.clone());
                    activity.checkpoint(&turn.channel.definition.record_id,
                        &turn.activity.event_id, change).await;
                }
                if let Some(outcome) = projection.event(event)? {
                    let terminal = outcome.terminal();
                    let kind = outcome.kind();
                    if terminal && acceptance_pending {
                        refuse(admitted,ProblemCode::ProviderUnavailable,"provider stream ended before acceptance");
                    }
                    send(delivery,&turn,outcome).await?;
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
pub(super) async fn send(
    delivery: &dyn DeliverySink,
    turn: &Arc<LaunchRequest>,
    outcome: TurnOutcome,
) -> Result<(), &'static str> {
    delivery
        .submit(DeliveryCommand::StreamOutcome {
            turn: turn.clone(),
            outcome,
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
