//! Parsing of the native Codex JSONL history export.

use layer2_core::seconds_from_rfc3339;
use serde_json::Value;

pub(super) const MAX_HISTORY_BYTES: usize = 64 * 1024 * 1024;
const MAX_HISTORY_LINES: usize = 131_072;

#[derive(Debug)]
pub(super) struct NativeTerminal {
    pub(super) conversation_id: String,
    pub(super) turn_id: String,
    pub(super) _started_at: String,
    pub(super) completed_at: String,
    pub(super) final_answer: String,
}

pub(super) fn parse_history(
    bytes: &[u8],
    expected_conversation: &str,
    expected: &str,
) -> Option<NativeTerminal> {
    if bytes.is_empty() || bytes.len() > MAX_HISTORY_BYTES {
        return None;
    }
    let mut conversation_id = None;
    let mut open = std::collections::BTreeMap::<String, (String, i128)>::new();
    let mut completed = None;
    let mut final_answer = String::new();
    let mut lines = 0;
    for line in bytes.split(|byte| *byte == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        lines += 1;
        if lines > MAX_HISTORY_LINES {
            return None;
        }
        let value: Value = serde_json::from_slice(line).ok()?;
        let object = value.as_object()?;
        match object.get("type").and_then(Value::as_str) {
            Some("session_meta") => {
                let id = value.pointer("/payload/id").and_then(Value::as_str)?;
                if uuid::Uuid::parse_str(id).is_err()
                    || conversation_id
                        .as_deref()
                        .is_some_and(|current| current != id)
                {
                    return None;
                }
                conversation_id.get_or_insert_with(|| id.to_owned());
            }
            Some("response_item") => {
                let payload = value.get("payload")?;
                let phase = payload.get("phase").and_then(Value::as_str);
                if payload.get("role").and_then(Value::as_str) == Some("assistant")
                    && phase.is_none_or(|phase| phase == "final_answer")
                {
                    if let Some(text) = native_message_text(payload) {
                        if open.contains_key(expected) {
                            final_answer.push_str(&text);
                        }
                    }
                }
            }
            Some("event_msg") => {
                let payload = value.get("payload")?.as_object()?;
                let kind = payload
                    .get("type")
                    .or_else(|| payload.get("kind"))
                    .and_then(Value::as_str);
                let Some(kind) = kind else {
                    continue;
                };
                if !matches!(
                    kind,
                    "task_started"
                        | "turn_started"
                        | "task_complete"
                        | "turn_complete"
                        | "turn_aborted"
                ) {
                    continue;
                }
                let turn = payload.get("turn_id").and_then(Value::as_str)?;
                if uuid::Uuid::parse_str(turn).is_err() {
                    return None;
                }
                let (at, millis) = native_timestamp(value.get("timestamp")?.as_str()?)?;
                match kind {
                    "task_started" | "turn_started" => {
                        if open.len() >= 128 {
                            return None;
                        }
                        open.insert(turn.to_owned(), (at, millis));
                        if turn == expected {
                            final_answer.clear();
                        }
                    }
                    "task_complete" | "turn_complete" => {
                        let (started_at, started_millis) = open.remove(turn)?;
                        if turn == expected {
                            if millis < started_millis {
                                return None;
                            }
                            completed = Some((turn.to_owned(), started_at, at));
                        }
                    }
                    "turn_aborted" => {
                        open.remove(turn);
                        if turn == expected {
                            return None;
                        }
                    }
                    _ => unreachable!(),
                }
            }
            _ => {}
        }
    }
    let conversation_id = conversation_id?;
    let (turn_id, started_at, completed_at) = completed?;
    (conversation_id == expected_conversation && open.is_empty() && !final_answer.is_empty())
        .then_some(NativeTerminal {
            conversation_id,
            turn_id,
            _started_at: started_at,
            completed_at,
            final_answer,
        })
}

fn native_message_text(payload: &Value) -> Option<String> {
    let content = payload.get("content")?;
    if let Some(text) = content.as_str() {
        return Some(text.to_owned());
    }
    let parts = content.as_array()?;
    let mut text = String::new();
    for part in parts {
        if let Some(value) = part.get("text").and_then(Value::as_str) {
            text.push_str(value);
        }
    }
    (!text.is_empty()).then_some(text)
}

fn native_timestamp(value: &str) -> Option<(String, i128)> {
    let value = value.strip_suffix('Z')?;
    let (whole, fraction) = value.split_once('.').map_or((value, ""), |parts| parts);
    if whole.len() != 19 || !fraction.is_empty() && fraction.len() > 9 {
        return None;
    }
    let normalized = format!("{whole}Z");
    let seconds = seconds_from_rfc3339(&normalized)? as i128;
    if !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let nanos = if fraction.is_empty() {
        0
    } else {
        fraction
            .parse::<u32>()
            .ok()?
            .checked_mul(10_u32.pow(9_u32.saturating_sub(fraction.len() as u32)))?
    };
    Some((normalized, seconds * 1_000 + i128::from(nanos / 1_000_000)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_requires_matching_complete_and_no_open_turn() {
        let conversation = "11111111-1111-7111-8111-111111111111";
        let turn = "22222222-2222-7222-8222-222222222222";
        let history = include_bytes!("fixtures/native_history_export.jsonl");
        let native = parse_history(history, conversation, turn).expect("native export parses");
        assert_eq!(native.final_answer, "The completed answer.\n");
    }
}
