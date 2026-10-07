use super::{
    outcomes::{DeliveryCommand, TurnOutcome},
    projection::Projection,
};
use crate::{
    ingress::launch::{LaunchOutcome, LaunchRequest},
    providers::TurnStream,
};
use cmaeg_core::problem::{Problem, ProblemCode};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

pub(super) async fn consume(
    mut stream: TurnStream,
    turn: Arc<LaunchRequest>,
    delivery: &mpsc::Sender<DeliveryCommand>,
    admitted: &mut Option<oneshot::Sender<LaunchOutcome>>,
) -> Result<&'static str, &'static str> {
    let mut acceptance_pending = true;
    let mut projection = Projection::default();
    loop {
        tokio::select! {
            biased;
            accepted = &mut stream.accepted, if acceptance_pending => {
                acceptance_pending = false;
                let accepted = accepted.map_err(|_| "stream_lost")?;
                metrics::histogram!(crate::telemetry::names::PROVIDER_ACCEPT,
                    crate::telemetry::names::PROVIDER => turn.channel.definition.provider.slug.to_string())
                    .record(accepted.at.saturating_duration_since(stream.request_started).as_secs_f64());
                tracing::info!(metric="cmaeg_provider_accept_seconds",duration_seconds=accepted.at.saturating_duration_since(stream.request_started).as_secs_f64(),"provider accepted turn");
                if let Some(sender) = admitted.take() {let _ = sender.send(LaunchOutcome::Accepted {response_id:accepted.response_id.clone()});}
                send(delivery,&turn,TurnOutcome::Accepted {response_id:accepted.response_id,thread_reply:accepted.thread_reply}).await?;
            }
            event = stream.events.recv() => {
                let Some(event) = event else {return Err("stream_lost");};
                if let Some(outcome) = projection.event(event)? {
                    let terminal = outcome.terminal();
                    let kind = outcome.kind();
                    if terminal && acceptance_pending {
                        refuse(admitted,ProblemCode::ProviderUnavailable,"provider stream ended before acceptance");
                    }
                    send(delivery,&turn,outcome).await?;
                    if terminal {return Ok(kind);}
                }
            }
        }
    }
}
pub(super) async fn send(
    delivery: &mpsc::Sender<DeliveryCommand>,
    turn: &Arc<LaunchRequest>,
    outcome: TurnOutcome,
) -> Result<(), &'static str> {
    delivery
        .send(DeliveryCommand::StreamOutcome {
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
