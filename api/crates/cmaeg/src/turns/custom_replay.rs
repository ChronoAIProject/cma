//! Recover legacy Custom retries from provider admission, never a process cache.
use crate::{
    ingress::launch::{LaunchOutcome, LaunchRequest},
    providers::{ProviderClient, ProviderError},
};
use cmaeg_core::{
    activity::{ActivityKind, SourceKind},
    problem::{Problem, ProblemCode},
};
use tokio::sync::oneshot;

pub(super) fn eligible(turn: &LaunchRequest) -> bool {
    turn.consumer.is_none()
        // Full-context GET is an optional provider extension. Only CMA's
        // deployed retained-admission reader has been verified; do not add
        // a new read dependency to legacy Nyxbot or other provider ingress.
        && turn.channel.consumer(None).is_some_and(|consumer| {
            consumer.provider.slug.as_str() == "cma_codex"
        })
        && turn.activity.routing.is_none()
        && turn.activity.source == SourceKind::Custom
        && matches!(turn.activity.kind, ActivityKind::Event { .. })
}

/// A fresh event returns None. Retained exact events acknowledge the original
/// admission without re-delivering its output or claiming a new sync waiter.
pub(super) async fn lookup(
    providers: &ProviderClient,
    turn: &LaunchRequest,
    retry_enabled: bool,
) -> Result<Option<LaunchOutcome>, Problem> {
    if !eligible(turn) {
        return Ok(None);
    }
    let consumer = turn.channel.consumer(None).ok_or_else(unavailable)?;
    let credential = consumer.credential.ok_or_else(unavailable)?;
    let found = if retry_enabled {
        providers
            .find_event_context_with_retry(
                &consumer.provider.slug,
                &consumer.provider.binding_id,
                &turn.conversation_id,
                &turn.activity.event_id,
                credential,
            )
            .await
            .map_err(|failure| {
                let retry_after = failure.retry_after;
                let mut problem = problem(failure.error);
                if let Some(after) = retry_after {
                    problem.retry_after_seconds = Some(
                        after
                            .as_millis()
                            .div_ceil(1000)
                            .max(1)
                            .min(u64::MAX as u128) as u64,
                    );
                }
                problem
            })?
    } else {
        providers
            .find_event_context(
                &consumer.provider.slug,
                &consumer.provider.binding_id,
                &turn.conversation_id,
                &turn.activity.event_id,
                credential,
            )
            .await
            .map_err(problem)?
    };
    let Some(retained) = found else {
        return Ok(None);
    };
    if retained.channel_id != turn.channel.id {
        return Err(unavailable());
    }
    let mut incoming = turn.activity.clone();
    // Only this server-generated timestamp is allowed to differ. The original
    // CloudEvent (including an explicit time), source, data and attachments
    // are still compared in full. Expiry and waiter are not caller content.
    if matches!(&incoming.kind, ActivityKind::Event { ce, .. } if ce.time.is_none()) {
        incoming
            .occurred_at
            .clone_from(&retained.activity.occurred_at);
    }
    if incoming != retained.activity {
        return Err(conflict());
    }
    Ok(Some(LaunchOutcome::Admitted {
        admission_id: turn.idempotency_key.as_str().to_owned(),
        conversation_id: turn.conversation_id.clone(),
        sync_waiter_attached: false,
    }))
}

pub(super) fn command_conflict(error: &ProviderError) -> bool {
    matches!(error, ProviderError::Rejected { status: 409, code }
        if code == "agent_command_key_conflict")
}

/// Refusals on a retry must not publish a failed callback for the admitted turn.
pub(super) fn answer(
    result: Result<Option<LaunchOutcome>, Problem>,
    sender: &mut Option<oneshot::Sender<LaunchOutcome>>,
) -> Option<&'static str> {
    let (outcome, label) = match result {
        Ok(None) => return None,
        Ok(Some(outcome)) => (outcome, "replayed"),
        Err(error) => (LaunchOutcome::Refused(error), "refused"),
    };
    if let Some(sender) = sender.take() {
        let _ = sender.send(outcome);
    }
    Some(label)
}

pub(super) fn conflict() -> Problem {
    Problem::new(ProblemCode::Conflict, 409, "event_id_content_conflict")
}

fn unavailable() -> Problem {
    Problem::new(
        ProblemCode::ProviderUnavailable,
        503,
        "provider_context_unavailable",
    )
}

fn problem(error: ProviderError) -> Problem {
    match error {
        ProviderError::Rejected { status: 401, .. } => Problem::new(
            ProblemCode::NotAuthenticated,
            401,
            "provider_context_not_authenticated",
        ),
        ProviderError::Rejected { status: 403, .. } => {
            Problem::new(ProblemCode::Forbidden, 403, "provider_context_forbidden")
        }
        _ => unavailable(),
    }
}
