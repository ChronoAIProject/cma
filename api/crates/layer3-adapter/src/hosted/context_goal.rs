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
}

// Borrow the fields before taking shared state so unrelated updates never wait
// for a context-goal reader or writer, including with retention disabled.
pub(super) struct Observation<'a> {
    frame: &'a JsonFrame,
    session: &'a str,
    absent: bool,
}

impl<'a> Observation<'a> {
    pub(super) fn read(frame: &'a JsonFrame) -> Option<Self> {
        let params = frame.get("params")?;
        let session = params.get("sessionId")?.as_str()?;
        let update = params.get("update")?;
        if update.get("sessionUpdate").and_then(Value::as_str) != Some("session_info_update") {
            return None;
        }
        let goal = update.get("_meta")?.get("goal")?;
        Some(Self {
            frame,
            session,
            absent: goal.is_null(),
        })
    }

    pub(super) fn record(self, observed: &mut Option<ContextGoal>) {
        let Self {
            frame,
            session,
            absent,
        } = self;
        if observed
            .as_ref()
            .is_some_and(|goal| !goal.matches_session(session))
        {
            return;
        }
        // Native work vetoes maintenance until reconnection, even if a later
        // observation reports null while that work can still be running.
        let safe = absent && observed.as_ref().is_none_or(|goal| goal.safe);
        let session = match frame.shared() {
            Some(frame) => Session::Admitted(frame),
            None => Session::Legacy(session.to_owned()),
        };
        *observed = Some(ContextGoal { session, safe });
    }
}

#[cfg(test)]
#[path = "context_goal_tests.rs"]
mod tests;
