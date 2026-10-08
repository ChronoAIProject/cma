use super::{
    outcomes::{DeliveryCommand, TurnOutcome},
    projection::Projection,
};
use crate::{
    delivery::router::DeliverySink,
    ingress::launch::{LaunchOutcome, LaunchRefusal, LaunchRequest},
    providers::TurnStream,
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
                    crate::telemetry::names::PROVIDER => provider.to_owned())
                    .record(accepted.at.saturating_duration_since(stream.request_started).as_secs_f64());
                tracing::info!(metric="cmaeg_provider_accept_seconds",duration_seconds=accepted.at.saturating_duration_since(stream.request_started).as_secs_f64(),"provider accepted turn");
                if let Some(sender) = admitted.take() {let _ = sender.send(LaunchOutcome::Accepted {response_id:accepted.response_id.clone()});}
                send(delivery,&turn,TurnOutcome::Accepted {response_id:accepted.response_id}).await?;
            }
            event = stream.events.recv() => {
                let Some(event) = event else {return Err("stream_lost");};
                if let Some(outcome) = projection.event(event)? {
                    let terminal = outcome.terminal();
                    let kind = outcome.kind();
                    if terminal && acceptance_pending {
                        refuse(admitted,LaunchRefusal::new(Problem::new(ProblemCode::ProviderUnavailable,503,"provider stream ended before acceptance"), "stream_lost"));
                    }
                    send(delivery,&turn,outcome).await?;
                    if terminal {return Ok(kind);}
                }
            }
        }
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
    refusal: LaunchRefusal,
) {
    if let Some(sender) = admitted.take() {
        let _ = sender.send(LaunchOutcome::Refused(refusal));
    }
}
