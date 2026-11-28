//! Shared admitted chat dispatch for relay and native transports.
use super::{
    callbacks::{authenticated_refusal, invalid, CallbackResponse},
    problems::{problem, problem_document},
};
use crate::ingress::{
    admission::Refusal,
    launch::{LaunchOutcome, LaunchRequest},
    relay::{facts::CallbackFacts, VerificationError},
    routed::TARGET_UNKNOWN,
    IngressState,
};
use crate::telemetry::timing::IngressTiming;
use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use cmaeg_core::{activity::SourceKind, channel::Platform, problem::ProblemCode, seal::EventRef};
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
    let telegram_refusal = matches!(
        request.activity.source,
        SourceKind::NyxidRelay {
            platform: Platform::Telegram,
            ..
        }
    )
    .then(|| {
        let definition = &request.channel.definition;
        let receipt = super::callbacks::media_refusals::receipt_for_event(
            &request.channel,
            &request.activity.event_id,
            request.activity.source.clone(),
            request.activity.conversation.kind,
            request.activity.occurred_at.clone(),
        );
        let notice = definition
            .routing
            .as_ref()
            .filter(|policy| policy.addressing.command && policy.addressing.unknown_target_notice)
            .map(|_| request.event_ref.clone());
        (
            request.channel.clone(),
            request.activity.event_id.clone(),
            request.activity.occurred_at.clone(),
            receipt,
            notice,
        )
    });
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
            if error.code == ProblemCode::AdmissionRefused
                && error.detail.as_deref() == Some(TARGET_UNKNOWN)
            {
                if let Some((channel, event, occurred_at, receipt, notice)) = telegram_refusal {
                    let record = &channel.definition.record_id;
                    state
                        .activity
                        .refused(record, "nyxid_relay", Some(&event), TARGET_UNKNOWN);
                    let confirmed = match receipt {
                        Some(receipt) => receipt
                            .persist_label(&state, record, TARGET_UNKNOWN)
                            .await
                            .is_ok(),
                        None => false,
                    };
                    if confirmed {
                        if let Some(reference) = notice {
                            explain_unknown_target(
                                &state,
                                &channel,
                                &event,
                                &occurred_at,
                                &reference,
                            )
                            .await;
                        }
                    } else {
                        tracing::warn!("Telegram unknown-target refusal persistence unavailable");
                    }
                }
            }
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

async fn explain_unknown_target(
    state: &IngressState,
    channel: &crate::channels::resolver::ResolvedChannel,
    event: &cmaeg_core::ids::EventId,
    occurred_at: &str,
    reference: &EventRef,
) {
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let Ok(opened) = reference.open(&state.master_keys, now) else {
        return;
    };
    super::telegram_refusal_notice::send(
        state,
        channel,
        event,
        occurred_at,
        &opened.payload.reply,
        "Unknown target. Check the Consumer name and try again.",
    )
    .await;
}
