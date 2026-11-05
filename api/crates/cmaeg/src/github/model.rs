use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::activity::{ActivityConsumer, ConsumerState};

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BindRequest {
    pub app_ref: String,
    pub installation_id: u64,
    pub proof_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub binding_id: String,
    pub app_ref: String,
    pub app_id: u64,
    pub installation_id: u64,
    pub account_id: u64,
    pub status: String,
    pub authority_revision: u64,
    pub verified_until: i64,
    pub verified_by: String,
    pub verification_ref: String,
    pub repository_ids: Vec<u64>,
}

#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub receipt_id: String,
    pub app_ref: String,
    pub installation_id: Option<u64>,
    pub delivery_id: String,
    pub repository_id: Option<u64>,
    pub event: String,
    pub action: Option<String>,
    pub status: String,
    pub received_at: i64,
    pub raw_body_expires_at: i64,
    pub body_sha256: String,
    pub task_ids: Vec<String>,
    pub authority_revision: u64,
}

#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub task_id: String,
    pub status: String,
    pub response_id: Option<String>,
    pub channel_id: String,
    pub record_id: String,
    pub event_id: String,
    pub target_count: usize,
    pub accepted_at: i64,
    pub retained_until: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<TaskResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskResult {
    pub status: TaskResultStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskResultStatus {
    Succeeded,
    NoChange,
}

impl TaskResult {
    fn from_value(value: &Value) -> Option<Self> {
        let status = match value.get("status")?.as_str()? {
            "succeeded" => TaskResultStatus::Succeeded,
            "no_change" => TaskResultStatus::NoChange,
            _ => return None,
        };
        let branch = bounded_string(value.get("branch"), 128)?;
        let commit = bounded_string(value.get("commit"), 512)?;
        let pull_request = bounded_string(value.get("pull_request"), 2_048)?;
        match status {
            TaskResultStatus::Succeeded
                if branch.is_none() || commit.is_none() || pull_request.is_none() =>
            {
                return None;
            }
            TaskResultStatus::NoChange if commit.is_some() || pull_request.is_some() => {
                return None;
            }
            _ => {}
        }
        Some(Self {
            status,
            branch,
            commit,
            pull_request,
        })
    }
}

fn bounded_string(value: Option<&Value>, max: usize) -> Option<Option<String>> {
    let Some(value) = value else {
        return Some(None);
    };
    if value.is_null() {
        return Some(None);
    }
    let text = value.as_str()?;
    if text.is_empty() || text.len() > max || text.bytes().any(|byte| byte.is_ascii_control()) {
        return None;
    }
    Some(Some(text.to_owned()))
}

pub(crate) fn parse_task_result(response: &Value) -> Option<TaskResult> {
    if response.get("status").and_then(Value::as_str) != Some("completed") {
        return None;
    }
    let output = response.get("output")?.as_array()?;
    for item in output.iter().rev() {
        let Some(content) = item.get("content").and_then(Value::as_array) else {
            continue;
        };
        for part in content.iter().rev() {
            let Some(text) = part.get("text").and_then(Value::as_str) else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<Value>(text.trim()) else {
                continue;
            };
            if let Some(result) = TaskResult::from_value(&value) {
                return Some(result);
            }
        }
    }
    None
}

impl Task {
    pub(crate) fn status_for(consumers: &[ActivityConsumer]) -> &'static str {
        if consumers
            .iter()
            .any(|consumer| consumer.state == ConsumerState::Refused)
        {
            return "owner_action";
        }
        if consumers
            .iter()
            .any(|consumer| consumer.state == ConsumerState::Failed)
        {
            return "failed";
        }
        if consumers
            .iter()
            .any(|consumer| consumer.state == ConsumerState::Unknown)
        {
            return "recovering";
        }
        if !consumers.is_empty()
            && consumers
                .iter()
                .all(|consumer| consumer.state == ConsumerState::Completed)
        {
            return "succeeded";
        }
        if consumers.iter().any(|consumer| {
            matches!(
                consumer.state,
                ConsumerState::Dispatching | ConsumerState::Accepted | ConsumerState::Produced
            )
        }) {
            return "running";
        }
        "queued"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn consumer(state: ConsumerState) -> ActivityConsumer {
        ActivityConsumer {
            consumer_id: "goal-agent".into(),
            display_name: None,
            agent_id: None,
            provider: None,
            conversation_id: None,
            state,
            dispatched_at: None,
            accepted_at: None,
            produced_at: None,
            completed_at: None,
            response_id: None,
            deliveries: Vec::new(),
            media_responses: Vec::new(),
        }
    }

    #[test]
    fn parses_the_machine_readable_no_change_result_from_a_completed_response() {
        let response = json!({
            "status": "completed",
            "output": [{"content": [{
                "type": "output_text",
                "text": r#"{"status":"no_change","branch":"automation/chrono-goals-sync","commit":null,"pull_request":null}"#
            }]}]
        });

        assert_eq!(
            parse_task_result(&response),
            Some(TaskResult {
                status: TaskResultStatus::NoChange,
                branch: Some("automation/chrono-goals-sync".into()),
                commit: None,
                pull_request: None,
            })
        );
    }

    #[test]
    fn rejects_incomplete_or_contradictory_terminal_result_text() {
        for text in [
            r#"{"status":"succeeded","branch":"automation/chrono-goals-sync"}"#,
            r#"{"status":"no_change","branch":"automation/chrono-goals-sync","commit":"abc"}"#,
        ] {
            let response = json!({
                "status": "completed",
                "output": [{"content": [{"type": "output_text", "text": text}]}]
            });
            assert_eq!(parse_task_result(&response), None, "{text}");
        }
    }

    #[test]
    fn ignores_untrusted_or_incomplete_result_text() {
        let response = json!({
            "status": "in_progress",
            "output": [{"content": [{
                "type": "output_text",
                "text": r#"{"status":"succeeded","pull_request":"https://evil.example"}"#
            }]}]
        });

        assert_eq!(parse_task_result(&response), None);
    }

    #[test]
    fn task_status_projection_preserves_recovery_and_terminal_states() {
        assert_eq!(
            Task::status_for(&[consumer(ConsumerState::Queued)]),
            "queued"
        );
        assert_eq!(
            Task::status_for(&[consumer(ConsumerState::Accepted)]),
            "running"
        );
        assert_eq!(
            Task::status_for(&[consumer(ConsumerState::Unknown)]),
            "recovering"
        );
        assert_eq!(
            Task::status_for(&[consumer(ConsumerState::Completed)]),
            "succeeded"
        );
        assert_eq!(
            Task::status_for(&[consumer(ConsumerState::Refused)]),
            "owner_action"
        );
        assert_eq!(
            Task::status_for(&[
                consumer(ConsumerState::Completed),
                consumer(ConsumerState::Failed),
            ]),
            "failed"
        );
    }
}
