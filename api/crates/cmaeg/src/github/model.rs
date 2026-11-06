use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::activity::{ActivityConsumer, ConsumerState};

#[derive(Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BindRequest {
    pub app_ref: String,
    pub installation_id: u64,
    /// Retained for legacy requests; static proofs no longer grant authority.
    #[schema(value_type = String, required = false)]
    pub proof_ref: Option<String>,
    #[schema(value_type = u64, required = false)]
    pub account_id: Option<u64>,
    #[schema(value_type = Vec<u64>, required = false, max_items = 256)]
    pub repository_ids: Option<Vec<u64>>,
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
    #[schema(required = false)]
    pub version: u64,
    #[schema(required = false)]
    pub requested_repository_ids: Vec<u64>,
    #[schema(required = false)]
    pub approved_repository_ids: Vec<u64>,
    #[schema(required = false)]
    pub excluded_repository_ids: Vec<u64>,
    pub approval: Option<BindingApproval>,
}

impl Binding {
    /// Outside-scope repositories already have no authority. Retain exclusions
    /// only for the approved/requested scope, never an installation inventory.
    pub(super) fn exclude_repository(&mut self, id: u64) {
        if (self.approved_repository_ids.contains(&id)
            || self.requested_repository_ids.contains(&id))
            && !self.excluded_repository_ids.contains(&id)
        {
            self.excluded_repository_ids.push(id);
            self.excluded_repository_ids.sort_unstable();
        }
        self.repository_ids.retain(|candidate| *candidate != id);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BindingApproval {
    pub verified_by: String,
    pub evidence_ref: String,
    pub approved_at: i64,
    pub authority_revision: u64,
    pub repository_ids: Vec<u64>,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct VerifyBindingRequest {
    pub expected_version: u64,
    pub evidence_ref: String,
    #[schema(max_items = 256)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
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
    /// Retained v1 compatibility field; the gateway never interprets agent results.
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
