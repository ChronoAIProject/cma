//! Shared admitted chat dispatch for relay and native transports.
use super::{
    callbacks::{authenticated_refusal, invalid, CallbackResponse},
    problems::{problem, problem_document},
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
/// headers and parsed copies are dropped before the acceptance wait. Selected
/// durable ingress retains at most 256 KiB of authenticated wire bytes until its
/// admission commits; legacy ingress retains no raw body.
pub(crate) struct AdmittedChat {
    pub request: LaunchRequest,
    pub routed_source: Option<crate::ingress::routed::VerifiedIngress>,
    pub callback: Option<Box<CallbackFacts>>,
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
        routed_source,
        callback,
        route,
        now,
    } = admitted;
    if let Some(callback) = callback {
        let record = &request.channel.definition.record_id;
        let source = timing.source();
        let Some(store) = &state.interactions else {
            return authenticated_refusal(
                &state,
                record,
                source,
                Refusal::InteractiveActionUnsupported,
            );
        };
        let interaction = match store
            .consume_for_dispatch(
                &request.channel,
                &request.activity,
                &route,
                &callback.id,
                &callback.data,
                &callback.message_id,
                callback.form.as_ref(),
            )
            .await
        {
            Ok(value) => value,
            Err(crate::interactions::Error::Denied) => {
                return authenticated_refusal(
                    &state,
                    record,
                    source,
                    Refusal::InteractionExpiredOrInvalid,
                )
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
        if let Some(owner) = interaction.2 {
            request.consumer = owner.consumer.clone();
            request.conversation_id = owner.conversation_id.clone();
            request.idempotency_key = match &owner.consumer {
                Some(consumer) => cmaeg_core::ids::IdempotencyKey::for_consumer(
                    &request.activity.event_id,
                    consumer,
                ),
                None => cmaeg_core::ids::IdempotencyKey::from_event(&request.activity.event_id),
            };
            request.activity.routing = owner.routing;
            opened.payload.consumer = owner.consumer;
            opened.payload.conversation_id = owner.conversation_id;
            opened.payload.partition_key = owner.partition_key;
        }
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
    match state.launch_verified(request, routed_source, hold).await {
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
            let mut response = problem_document(error);
            if retry {
                response
                    .headers_mut()
                    .insert(header::RETRY_AFTER, HeaderValue::from_static("2"));
            }
            response
        }
    }
}
