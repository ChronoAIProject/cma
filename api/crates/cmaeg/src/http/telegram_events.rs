//! New Telegram notifications require the retained durable routing path.
use crate::{
    http::{
        callbacks::{authenticated_invalid, authenticated_refusal, CallbackResponse},
        problems::problem,
    },
    ingress::{
        admission::Refusal, launch::LaunchOutcome, message_event::VerifiedMessageEvent,
        relay::VerificationError, telegram::events::Error, IngressState,
    },
    telemetry::timing::IngressTiming,
};
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use cmaeg_core::{ids::RecordId, problem::ProblemCode};

pub(super) fn failure(state: &IngressState, record: &RecordId, error: Error) -> Response {
    match error {
        Error::Context => authenticated_refusal(
            state,
            record,
            "telegram",
            Refusal::MessageContextUnavailable,
        ),
        Error::Refused(reason) => authenticated_refusal(state, record, "telegram", reason),
        Error::Invalid => authenticated_invalid(
            state,
            record,
            "telegram",
            VerificationError("telegram_message_event_invalid"),
        ),
        Error::Unavailable => problem(
            StatusCode::SERVICE_UNAVAILABLE,
            ProblemCode::ProviderUnavailable,
            "telegram_message_context_unavailable",
        ),
    }
}
pub(super) async fn dispatch(
    state: IngressState,
    mut verified: VerifiedMessageEvent,
    timing: IngressTiming,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> Response {
    verified.request.timing = Some(timing);
    let event_id = verified.request.activity.event_id.to_string();
    match state.launch_message_event_verified(verified, permit).await {
        LaunchOutcome::Admitted {
            conversation_id, ..
        } => (
            StatusCode::ACCEPTED,
            Json(CallbackResponse::Accepted {
                event_id,
                conversation_id: conversation_id.to_string(),
            }),
        )
            .into_response(),
        LaunchOutcome::Refused(error) => problem(
            StatusCode::from_u16(error.status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
            error.code,
            error.detail.unwrap_or_default(),
        ),
        _ => problem(
            StatusCode::SERVICE_UNAVAILABLE,
            ProblemCode::ProviderUnavailable,
            "telegram_message_event_durable_admission_required",
        ),
    }
}
