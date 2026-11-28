use super::{
    now_rfc3339, AdmittedAgentCommand, AgentActor, AgentCommand, AgentCommandEnvelope,
    AgentSourceKind, Duration, UNCLAIMED_RECHECK_DELAY,
};

pub(super) fn is_channel_message(command: &AdmittedAgentCommand) -> bool {
    matches!(command.receipt.envelope.actor, AgentActor::Channel { .. })
        && command.receipt.envelope.source.kind == AgentSourceKind::Channel
        && matches!(
            command.receipt.envelope.command,
            AgentCommand::Message { .. }
        )
}

pub(super) fn unclaimed_recheck_delay(command: &AdmittedAgentCommand) -> Duration {
    if !is_channel_message(command) {
        return UNCLAIMED_RECHECK_DELAY;
    }
    // A transient observation must not add fifteen seconds to a fresh chat
    // message. Age the retry pace from its immutable admission receipt so a
    // sustained outage returns to normal polling, including after restarts.
    let age = crate::seconds_from_rfc3339(&now_rfc3339())
        .zip(crate::seconds_from_rfc3339(&command.receipt.admitted_at))
        .and_then(|(now, admitted)| now.checked_sub(admitted));
    match age {
        Some(0..30) => Duration::from_secs(1),
        Some(30..120) => Duration::from_secs(5),
        _ => UNCLAIMED_RECHECK_DELAY,
    }
}

/// An already authorized API message may be waiting only for startup or the
/// first prompt. Recheck promptly for a bounded age without changing its lane,
/// receipt, queue deadline, or authorization requirements on the next attempt.
pub(super) fn api_message_recheck_delay(
    envelope: &AgentCommandEnvelope,
    admitted_at: &str,
    deadline: &str,
    now: &str,
) -> Duration {
    if envelope.source.kind != AgentSourceKind::Api
        || !matches!(
            envelope.actor,
            AgentActor::Human { .. } | AgentActor::Api { .. }
        )
        || !matches!(envelope.command, AgentCommand::Message { .. })
    {
        return UNCLAIMED_RECHECK_DELAY;
    }
    let observed = crate::seconds_from_rfc3339(now);
    let age = observed
        .zip(crate::seconds_from_rfc3339(deadline))
        .filter(|(now, deadline)| now < deadline)
        .zip(crate::seconds_from_rfc3339(admitted_at))
        .and_then(|((now, _), admitted)| now.checked_sub(admitted));
    match age {
        Some(0..30) => Duration::from_secs(1),
        Some(30..120) => Duration::from_secs(5),
        _ => UNCLAIMED_RECHECK_DELAY,
    }
}

/// Preserve the durable deadline while allowing a bounded scheduler to keep
/// scanning past blocked messages. A restart may shorten this process-local
/// cooldown; every attempt still rereads and reauthorizes the command.
pub(super) fn unclaimed_cooldown(command: &AdmittedAgentCommand, delay: Duration) -> Duration {
    let remaining = crate::seconds_from_rfc3339(&command.queue_deadline)
        .zip(crate::seconds_from_rfc3339(&now_rfc3339()))
        .map(|(deadline, now)| Duration::from_secs(deadline.saturating_sub(now)))
        .unwrap_or_default();
    delay.min(remaining)
}
