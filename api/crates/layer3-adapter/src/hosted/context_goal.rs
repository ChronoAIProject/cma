//! Keep context identity tied to its admitted frame instead of copying its string.

use super::retained_json::JsonFrame;
use serde_json::Value;

enum Session {
    Legacy(String),
    // The entire original tree stays charged, including unused siblings.
    Admitted(JsonFrame),
}

pub(super) struct ContextGoal {
    session: Session,
    safe: bool,
}

impl ContextGoal {
    fn matches_session(&self, session: &str) -> bool {
        match &self.session {
            Session::Legacy(id) => id == session,
            Session::Admitted(frame) => {
                frame
                    .get("params")
                    .and_then(|params| params.get("sessionId"))
                    .and_then(Value::as_str)
                    == Some(session)
            }
        }
    }

    pub(super) fn is_safe_for(&self, session: &str) -> bool {
        self.safe && self.matches_session(session)
    }

    pub(super) fn observe(observed: &mut Option<Self>, frame: &JsonFrame) {
        let Some(params) = frame.get("params") else {
            return;
        };
        let Some(session) = params.get("sessionId").and_then(Value::as_str) else {
            return;
        };
        let Some(update) = params.get("update") else {
            return;
        };
        if update.get("sessionUpdate").and_then(Value::as_str) != Some("session_info_update") {
            return;
        }
        let Some(goal) = update.get("_meta").and_then(|meta| meta.get("goal")) else {
            return;
        };
        if observed
            .as_ref()
            .is_some_and(|goal| !goal.matches_session(session))
        {
            return;
        }
        // Native work vetoes maintenance until reconnection, even if a later
        // observation reports null while that work can still be running.
        let safe = goal.is_null() && observed.as_ref().is_none_or(|goal| goal.safe);
        let session = match frame.shared() {
            Some(frame) => Session::Admitted(frame),
            None => Session::Legacy(session.to_owned()),
        };
        *observed = Some(Self { session, safe });
    }
}

#[cfg(test)]
#[path = "context_goal_tests.rs"]
mod tests;
