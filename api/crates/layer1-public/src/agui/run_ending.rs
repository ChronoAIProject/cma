//! Classifying the durable edges that finish an AG-UI run.

use layer2_core::{SessionStatus, StopReason};

/// Whether a run ends here, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Ending {
    /// The turn finished. The run ends with it.
    Finished,
    /// The runtime is blocked on a decision. The run ends to ask.
    Interrupted,
    /// The session failed. The run ends saying so.
    Failed,
}

/// Whether a status change is the end of the run.
///
/// Going idle is not: a turn ending already says so, and an approval parks a
/// session idle while its run is being asked about. What ends a run is the
/// session ceasing to be able to answer at all.
pub(super) fn ends_the_run(status: SessionStatus, stop_reason: Option<StopReason>) -> bool {
    match status {
        SessionStatus::Terminated => true,
        // A handover, either onto a newer runtime or onto the other plane. The
        // session is coming back, but this run is not: its runtime is being
        // torn down, and a client left holding a stream that simply stops
        // cannot tell that from an agent taking its time. The detail carried
        // with the status says which handover it is and that the session
        // returns, so ending the run here is a statement rather than a
        // failure dressed up as one.
        SessionStatus::Rotating => true,
        SessionStatus::Idle => matches!(
            stop_reason,
            Some(StopReason::Error) | Some(StopReason::Revoked) | Some(StopReason::BudgetReached)
        ),
        _ => false,
    }
}

/// A stop reason as the wire spells it.
pub(super) fn wire_stop_reason(reason: StopReason) -> String {
    serde_json::to_value(reason)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}
