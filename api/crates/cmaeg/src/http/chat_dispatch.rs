//! Shared admitted chat dispatch for relay and native transports.
use super::{
    callbacks::{invalid, refused, CallbackResponse},
    consumer_admission::ConsumerAdmission,
    problems::problem,
};
use crate::ingress::{
    admission::Refusal,
    launch::{LaunchOutcome, LaunchRequest},
    relay::{facts::CallbackFacts, VerificationError},
    IngressState,
};
use crate::telemetry::timing::IngressTiming;
use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use cmaeg_core::problem::ProblemCode;
use tokio::sync::OwnedSemaphorePermit;
/// Everything an admitted chat turn needs once its request has been released.
///
/// Admission helpers return this instead of dispatching themselves so that the
/// raw body, its headers and every parsed copy are dropped before the
/// acceptance wait; a waiting handler then holds only this small context.
pub(crate) struct AdmittedChat {
    pub request: LaunchRequest,
    pub callback: Option<CallbackFacts>,
    pub route: String,
    pub now: i64,
}
/// Dispatch an admitted chat turn. `hold` is the handler's ingress permit:
/// every early return drops it, and the launcher decides how long a launched
/// turn keeps it.
pub(crate) async fn dispatch(
    state: IngressState,
    admitted: AdmittedChat,
    timing: IngressTiming,
    hold: OwnedSemaphorePermit,
) -> Response {
    let AdmittedChat {
        mut request,
        callback,
        route,
        now,
    } = admitted;
    if let Some(callback) = callback {
        let Some(store) = &state.interactions else {
            return refused(Refusal::InteractiveActionUnsupported);
        };
        // The store may retry a CAS. Capture its latest admitted decision, but
        // publish it only once the matching click has actually been consumed.
        let decision = std::sync::Arc::new(std::sync::Mutex::new(None));
        let captured = decision.clone();
        let channel = request.channel.clone();
        let activity = request.activity.clone();
        let routing = state.routing.clone();
        let admit = move |consumer: Option<&cmaeg_core::ids::ConsumerId>| {
            let input = cmaeg_core::routing::RouteInput {
                activity: &activity,
                chat_source: channel.definition.source_of(&activity.source),
                card_consumer: Some(consumer),
                published: None,
                non_human_providers: &routing.non_human_providers,
                attributes_signed: true,
            };
            let result = if consumer.is_some() && channel.definition.routing.is_none() {
                Err(cmaeg_core::routing::RouteError::InteractionDenied)
            } else {
                cmaeg_core::routing::resolve(
                    &channel.definition,
                    &input,
                    routing.multi_consumer_enabled,
                )
            };
            let admitted = result.is_ok();
            *captured.lock().unwrap() = Some((consumer.cloned(), result));
            admitted
        };
        let interaction = store
            .consume(
                &request.channel,
                &request.activity,
                &route,
                &callback.id,
                &callback.data,
                &callback.message_id,
                &admit,
            )
            .await;
        let decision = decision.lock().unwrap().take();
        if let Some((consumer, result)) = &decision {
            // A store failure after an admitted gate is not a routing success.
            // A rejecting gate returns Denied immediately, with no later CAS.
            if interaction.is_ok()
                || matches!(
                    (&interaction, result),
                    (Err(crate::interactions::Error::Denied), Err(_))
                )
            {
                crate::telemetry::routing::record(
                    &request.channel,
                    &cmaeg_core::routing::RouteInput {
                        activity: &request.activity,
                        chat_source: request
                            .channel
                            .definition
                            .source_of(&request.activity.source),
                        card_consumer: Some(consumer.as_ref()),
                        published: None,
                        non_human_providers: &state.routing.non_human_providers,
                        attributes_signed: true,
                    },
                    state.routing.multi_consumer_enabled,
                    result,
                );
            }
        }
        let interaction = match interaction {
            Ok(value) => value,
            Err(crate::interactions::Error::Denied) => {
                return refused(Refusal::InteractionExpiredOrInvalid)
            }
            Err(crate::interactions::Error::Unavailable) => {
                return problem(
                    StatusCode::SERVICE_UNAVAILABLE,
                    ProblemCode::ProviderUnavailable,
                    "interaction_store_unavailable",
                )
            }
        };
        request.activity.kind = cmaeg_core::activity::ActivityKind::Interaction(interaction.0);
        request.activity.occurred_at = interaction.1;
        let Some((_, Ok(decision))) = decision else {
            tracing::error!("consumed interaction has no admitted routing decision");
            return problem(
                StatusCode::INTERNAL_SERVER_ERROR,
                ProblemCode::Internal,
                "interaction_routing_invalid",
            );
        };
        if let cmaeg_core::routing::Resolution::Routed(decision) = decision {
            request.activity.routing = Some(decision.routing);
            state.observer.routed(
                &request.channel.pointer.record_id,
                &crate::turns::observer::ActivitySummary::of(&request.activity),
                &decision.trace,
            );
        }
        let Ok(mut opened) = request.event_ref.open(&state.master_keys, now) else {
            return invalid(timing.source(), VerificationError("event_ref_invalid"));
        };
        opened.payload.activity_sha256 = Some(cmaeg_core::seal::event_ref::activity_digest(
            &request.activity,
        ));
        let Ok(reference) = cmaeg_core::seal::EventRef::seal(&opened.payload, &state.master_keys)
        else {
            return invalid(timing.source(), VerificationError("event_ref_invalid"));
        };
        request.event_ref = reference;
    }
    if let Err(error) =
        crate::ingress::receipts::observe(&state, &request.channel, &request.activity).await
    {
        return problem(
            StatusCode::from_u16(error.status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
            error.code,
            error.detail.unwrap_or_default(),
        );
    }
    request.timing = Some(timing);
    let event_id = request.activity.event_id.to_string();
    let conversation_id = request.conversation_id.to_string();
    let (outcome, conversation_id, consumers) = if request.activity.routing.is_some() {
        let launched = state.launcher.launch_routed(request, Some(hold)).await;
        let consumers = match launched
            .consumers
            .into_iter()
            .map(ConsumerAdmission::try_from)
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(consumers) => consumers,
            Err(_) => {
                tracing::error!("launcher returned an invalid consumer admission name");
                return problem(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    ProblemCode::Internal,
                    "consumer_admission_invalid",
                );
            }
        };
        (
            launched.combined,
            launched.primary_conversation.to_string(),
            Some(consumers),
        )
    } else {
        (
            state.launcher.launch_holding(request, hold).await,
            conversation_id,
            None,
        )
    };
    match outcome {
        LaunchOutcome::Accepted { .. } | LaunchOutcome::Deferred => (
            StatusCode::ACCEPTED,
            Json(CallbackResponse::Accepted {
                event_id,
                conversation_id,
                consumers,
            }),
        )
            .into_response(),
        LaunchOutcome::Refused(error) => {
            let error = error.problem;
            let retry = error.code == ProblemCode::ProviderBusy;
            let mut response = problem(
                StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                error.code,
                error.detail.unwrap_or_default(),
            );
            if retry {
                response
                    .headers_mut()
                    .insert(header::RETRY_AFTER, HeaderValue::from_static("2"));
            }
            response
        }
    }
}
