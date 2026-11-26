//! Count attributable durable progress while omitting prompt and output text.

use std::collections::BTreeSet;

use layer2_core::{ids::AgentCommandId, seconds_from_rfc3339};
use mongodb::bson::{doc, Document};

use super::{positive, ScanError};

pub(super) const MAX_ROWS: u64 = 128;

pub(in crate::store::stale_turns) fn projection() -> Document {
    doc! {
        "_id": 0,
        "proof_bytes": {"$bsonSize": "$$ROOT"},
        "seq": 1,
        "at": bounded_string("$record.appended_at", 64),
        "kind": bounded_string("$record.kind.type", 64),
        "turn": "$record.kind.turn_sequence",
        "origin": bounded_string("$record.kind.origin_key", 256),
        "call": bounded_string("$record.kind.call_id", 256),
        "status": bounded_string("$record.kind.status", 64),
        "approval": bounded_string("$record.kind.approval_id", 256),
    }
}

fn bounded_string(path: &str, max_bytes: i32) -> Document {
    doc! {"$cond": [
        {"$eq": [{"$type": path}, "string"]},
        {"$cond": [{"$lte": [{"$strLenBytes": path}, max_bytes]}, path, null]},
        null,
    ]}
}

pub(super) struct Progress {
    turn: u64,
    next: u64,
    origin: String,
    latest_at: u64,
    tools: BTreeSet<String>,
    hold: Option<&'static str>,
}

impl Progress {
    pub(super) fn new(turn: u64, command: &AgentCommandId) -> Self {
        Self {
            turn,
            next: turn,
            origin: format!("command:{}", command.as_str()),
            latest_at: 0,
            tools: BTreeSet::new(),
            hold: None,
        }
    }

    pub(super) fn observe(&mut self, row: &Document) -> Result<(), ScanError> {
        if positive(row, "seq") != Some(self.next) {
            return Err(ScanError::Invalid);
        }
        let kind = row.get_str("kind").map_err(|_| ScanError::Invalid)?;
        let at = row
            .get_str("at")
            .ok()
            .and_then(seconds_from_rfc3339)
            .ok_or(ScanError::Invalid)?;
        if self.next == self.turn {
            if kind != "turn_started"
                || positive(row, "turn") != Some(self.turn)
                || row.get_str("origin").ok() != Some(self.origin.as_str())
            {
                self.hold = Some("identity_changed");
            }
            self.latest_at = at;
        } else {
            match kind {
                "turn_started" => self.hold = Some("identity_changed"),
                "turn_completed" | "turn_failed" | "turn_interrupted"
                    if positive(row, "turn") == Some(self.turn) =>
                {
                    self.hold = Some("canonical_result");
                }
                "conversation_gap" if self.hold.is_none() => self.hold = Some("output_gap"),
                "agent_message" | "tool_output_reported" => self.record(at),
                "tool_call_started" | "tool_call_updated" => {
                    let call = row.get_str("call").map_err(|_| ScanError::Invalid)?;
                    let status = row.get_str("status").map_err(|_| ScanError::Invalid)?;
                    if matches!(status, "completed" | "failed" | "cancelled") {
                        self.tools.remove(call);
                    } else {
                        self.tools.insert(call.to_owned());
                    }
                    self.record(at);
                }
                _ => {}
            }
        }
        self.next += 1;
        Ok(())
    }

    fn record(&mut self, at: u64) {
        self.latest_at = self.latest_at.max(at);
    }

    pub(super) fn finish(self, head: u64, now: u64) -> Result<u64, &'static str> {
        if self.turn == self.next || head.checked_add(1) != Some(self.next) {
            return Err("history_unavailable");
        }
        if let Some(reason) = self.hold {
            return Err(reason);
        }
        if !self.tools.is_empty() {
            return Err("tool_active");
        }
        now.checked_sub(self.latest_at).ok_or("clock_mismatch")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: &str = "2026-01-01T00:00:00Z";
    const LATER: &str = "2026-01-01T00:14:00Z";
    const NOW: &str = "2026-01-01T00:16:00Z";

    fn progress() -> Progress {
        let mut progress = Progress::new(2, &AgentCommandId("cmd_example".into()));
        progress
            .observe(&doc! {
                "seq": 2, "at": START, "kind": "turn_started",
                "turn": 2, "origin": "command:cmd_example",
            })
            .unwrap();
        progress
    }

    #[test]
    fn stale_turn_progress_counts_output_but_ignores_unrelated_usage() {
        let now = seconds_from_rfc3339(NOW).unwrap();
        let mut progress = progress();
        progress
            .observe(&doc! {"seq": 3, "at": LATER, "kind": "usage_updated"})
            .unwrap();
        assert_eq!(progress.finish(3, now), Ok(960));
        let mut progress = self::progress();
        progress
            .observe(&doc! {"seq": 3, "at": LATER, "kind": "agent_message"})
            .unwrap();
        assert_eq!(progress.finish(3, now), Ok(120));
    }

    #[test]
    fn stale_turn_progress_holds_tools_and_preserves_a_canonical_result() {
        let now = seconds_from_rfc3339(NOW).unwrap();
        let started = doc! {
            "seq": 3, "at": START, "kind": "tool_call_started",
            "call": "example", "status": "in_progress",
        };
        let mut waiting = progress();
        waiting.observe(&started).unwrap();
        assert_eq!(waiting.finish(3, now), Err("tool_active"));
        let mut completed = progress();
        completed.observe(&started).unwrap();
        completed
            .observe(&doc! {
                "seq": 4, "at": LATER, "kind": "turn_failed", "turn": 2,
            })
            .unwrap();
        assert_eq!(completed.finish(4, now), Err("canonical_result"));
    }

    #[test]
    fn stale_turn_progress_holds_incomplete_history() {
        let now = seconds_from_rfc3339(NOW).unwrap();
        assert_eq!(progress().finish(3, now), Err("history_unavailable"));
    }
}
