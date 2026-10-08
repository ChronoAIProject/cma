//! Routed custom transport refusals and primary-only stream admission.
use super::{invalid, EventAccepted};
use crate::{
    channels::resolver::ResolvedChannel,
    http::{consumer_admission::ConsumerAdmission, problems::problem},
    ingress::{
        custom::activity::BuildError,
        launch::{LaunchOutcome, LaunchRequest},
        IngressState,
    },
};
use axum::{
    http::{HeaderValue, StatusCode},
    response::Response,
};
use cmaeg_core::{activity::Activity, channel::Sink, problem::ProblemCode, routing::RouteError};
use tokio::sync::OwnedSemaphorePermit;

pub(super) fn build_error(error: BuildError) -> Response {
    let error = match error {
        BuildError::Invalid(reason) => return invalid(reason),
        BuildError::Routing(error) => error,
    };
    match error {
        RouteError::TargetUnknown(name) => invalid(&format!("target_unknown:{}", bounded(&name))),
        RouteError::TargetIgnoresActorKind(name) => {
            invalid(&format!("target_ignores_actor_kind:{}", bounded(&name)))
        }
        RouteError::TooManyTargets
        | RouteError::InvalidTargets
        | RouteError::TargetsNotPermitted
        | RouteError::TargetsUnsigned
        | RouteError::NoTargetAcceptsActorKind => invalid(&error.to_string()),
        RouteError::ConsumerUnavailable | RouteError::KillSwitchClosed => {
            let mut response = problem(
                StatusCode::SERVICE_UNAVAILABLE,
                ProblemCode::ProviderUnavailable,
                "consumer_unavailable",
            );
            if error == RouteError::KillSwitchClosed {
                response
                    .headers_mut()
                    .insert("retry-after", HeaderValue::from_static("300"));
            }
            response
        }
        RouteError::TargetNotMentionable(_) | RouteError::InteractionDenied => {
            tracing::warn!(code=%error,"unexpected custom routing refusal");
            problem(
                StatusCode::SERVICE_UNAVAILABLE,
                ProblemCode::ProviderUnavailable,
                "consumer_unavailable",
            )
        }
    }
}
fn bounded(value: &str) -> &str {
    let mut end = value.len().min(64);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

pub(super) fn stream_allowed(
    state: &IngressState,
    channel: &ResolvedChannel,
    activity: &Activity,
) -> bool {
    let Some(routing) = &activity.routing else {
        return true;
    };
    if routing.targets.len() <= 1 {
        return true;
    }
    // For a newly resolved routed output, default_ok's payload-independent
    // conditions are sufficient: the flag is on and this snapshot names a default.
    let default_ok = state.routing.multi_consumer_enabled
        && channel
            .definition
            .routing
            .as_ref()
            .is_some_and(|r| r.default_reply.is_some());
    default_ok
        || channel
            .definition
            .reply
            .sinks
            .iter()
            .any(|sink| matches!(sink, Sink::ResultWebhook { .. }))
}

pub(super) async fn launch(
    state: &IngressState,
    turn: LaunchRequest,
    hold: OwnedSemaphorePermit,
    primary_only: bool,
    accepted: &mut EventAccepted,
) -> Result<LaunchOutcome, Response> {
    if turn.activity.routing.is_none() {
        return Ok(state.launcher.launch_holding(turn, hold).await);
    }
    let outcome = state.launcher.launch_routed(turn, Some(hold)).await;
    let result = if primary_only {
        outcome
            .consumers
            .first()
            .map(|child| child.outcome.clone())
            .ok_or_else(invalid_admission)?
    } else {
        outcome.combined
    };
    accepted.conversation_id = outcome.primary_conversation;
    accepted.consumers = Some(
        outcome
            .consumers
            .into_iter()
            .map(ConsumerAdmission::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| invalid_admission())?,
    );
    Ok(result)
}
fn invalid_admission() -> Response {
    tracing::error!("launcher returned an invalid consumer admission");
    problem(
        StatusCode::INTERNAL_SERVER_ERROR,
        ProblemCode::Internal,
        "consumer_admission_invalid",
    )
}
