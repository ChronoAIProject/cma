use super::{agent_io::SOURCE_DEFERRED_FAILURE, AgentTranscript, AgentTurnOutcome, TypedFailure};
use crate::budget::BudgetError;

const CONTINUATION: &str = "The failed turn will not be replayed automatically. Earlier work may have completed; review the conversation before sending a new message to continue.";
pub const AGENT_TURN_NO_PROGRESS: &str = "agent_turn_no_progress";

/// Public Agent failures are a closed vocabulary, including for retained history.
/// Class and detail are both untrusted; neither supplies public diagnostic text.
pub fn public_agent_turn_failure(failure: &TypedFailure) -> TypedFailure {
    use super::turn_timeout::TurnTimeoutCause;
    let policy = match failure.class.as_str() {
        "agent_turn_timeout" => Some(TurnTimeoutCause::ServiceDeadline),
        "approval_timeout" => Some(TurnTimeoutCause::ApprovalBudget),
        "approval_denied" => Some(TurnTimeoutCause::ApprovalDenied),
        "approval_expired" => Some(TurnTimeoutCause::ApprovalExpired),
        "approval_wait_disconnected" => Some(TurnTimeoutCause::ApprovalDisconnected),
        _ => None,
    };
    if let Some(policy) = policy {
        return policy.failure();
    }
    let (class, detail, continuation) = match failure.class.as_str() {
        "upstream_timeout" => ("upstream_timeout", "The upstream model service timed out.", true),
        "upstream_unavailable" => ("upstream_unavailable", "The upstream model service is temporarily unavailable.", true),
        "upstream_rate_limited" => ("upstream_rate_limited", "The upstream model service is limiting requests. Wait before sending another message.", true),
        "insufficient_credits" => ("insufficient_credits", "The model request failed with HTTP 402 because its billing account has insufficient credits. Ask the Sandbox owner to top up the account used for model requests, then send a new message to continue. This failed turn will not be replayed automatically.", false),
        "owner_budget_exhausted" => ("owner_budget_exhausted", BudgetError::OwnerExhausted.guidance(), false),
        "turn_budget_exhausted" => ("turn_budget_exhausted", BudgetError::TurnExhausted.guidance(), false),
        "budget_unavailable" => ("budget_unavailable", BudgetError::Unavailable.guidance(), false),
        "platform_budget_exhausted" => ("platform_budget_exhausted", BudgetError::PlatformExhausted.guidance(), false),
        "agent_log_capacity_exceeded" => ("agent_log_capacity_exceeded", "This Agent cannot continue because its conversation history is full. If retained output is available, contact support to recover it.", false),
        "conversation_output_gap" => ("conversation_output_gap", "Some conversation output could not be retained. Earlier work may have completed; review the conversation before continuing.", false),
        AGENT_TURN_NO_PROGRESS => (AGENT_TURN_NO_PROGRESS, "The Agent turn ended after it stopped making progress.", true),
        "runtime_lost" => ("runtime_lost", "The Agent runtime was lost. Review the conversation and the Sandbox status before continuing.", false),
        SOURCE_DEFERRED_FAILURE => (SOURCE_DEFERRED_FAILURE, "The Agent input could not be authorized before it was sent.", false),
        _ => ("agent_turn_failed", "The Agent run failed.", true),
    };
    TypedFailure {
        class: class.into(),
        detail: if continuation {
            format!("{detail} {CONTINUATION}")
        } else {
            detail.into()
        },
    }
}

/// Apply only after folding original records and capturing retained-output facts.
pub fn sanitize_agent_transcript(transcript: &mut AgentTranscript) {
    for turn in &mut transcript.turns {
        if let AgentTurnOutcome::Failed { failure, .. } = &mut turn.outcome {
            *failure = public_agent_turn_failure(failure);
        }
    }
}

#[cfg(test)]
#[path = "agent_failure_projection_tests.rs"]
mod tests;
