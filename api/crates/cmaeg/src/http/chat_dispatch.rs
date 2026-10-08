//! Shared admitted chat dispatch for relay and native transports.
use super::{
    callbacks::{invalid, refused, CallbackResponse},
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
        let interaction = match store
            .consume(
                &request.channel,
                &request.activity,
                &route,
                &callback.id,
                &callback.data,
                &callback.message_id,
                // Only default cards can exist until ingress routes clicks to
                // the consumer that owns the card.
                &|consumer| consumer.is_none(),
            )
            .await
        {
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
    match state.launcher.launch_holding(request, hold).await {
        LaunchOutcome::Accepted { .. } | LaunchOutcome::Deferred => (
            StatusCode::ACCEPTED,
            Json(CallbackResponse::Accepted {
                event_id,
                conversation_id,
            }),
        )
            .into_response(),
        LaunchOutcome::Refused(error) => {
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
