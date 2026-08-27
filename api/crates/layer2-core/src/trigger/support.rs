use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{
    ConfirmationAction, EffectState, FailureClass, Launch, LaunchBlockedReason, LaunchEffectKind,
    LaunchEventKind, LaunchId, LaunchState, LaunchStepKey, LaunchStepStatus, PromptDeliveryState,
    TriggerValidationError,
};
use crate::{CapabilityRevocationState, SubjectId};

pub const MAX_TRIGGER_SUPPORT_TIMELINE: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriggerSupportLookup {
    Launch(LaunchId),
    CorrelationId(String),
}

impl TriggerSupportLookup {
    pub fn correlation(value: impl Into<String>) -> Result<Self, TriggerValidationError> {
        let value = value.into();
        if value.trim().is_empty()
            || value.len() > 256
            || value.bytes().any(|byte| byte.is_ascii_control())
        {
            return Err(TriggerValidationError::field(
                "correlation_id",
                "is not a usable support correlation identifier",
            ));
        }
        Ok(Self::CorrelationId(value))
    }
}

/// Least-privilege operational projection. It intentionally has no field for
/// rendered prompts, task inputs, file paths/content, credentials, request
/// bodies, PR title/body, or provider response bodies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerSupportSnapshot {
    pub launch_id: LaunchId,
    pub owner: SubjectId,
    pub revision_digest: String,
    pub publisher: SubjectId,
    pub upstream_repository_id: u64,
    pub fork_repository_id: Option<u64>,
    pub github_account_id: Option<u64>,
    pub service_id: Option<String>,
    pub session_id: Option<String>,
    pub state: LaunchState,
    pub blocked_reason: Option<LaunchBlockedReason>,
    pub created_at: String,
    pub updated_at: String,
    pub started_at: Option<String>,
    pub terminal_at: Option<String>,
    pub lease: Option<TriggerSupportLease>,
    pub cleanup: Option<TriggerSupportCleanup>,
    pub effects: Vec<TriggerSupportEffect>,
    pub timeline: Vec<TriggerSupportEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerSupportLease {
    pub fence: u64,
    pub acquired_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerSupportCleanup {
    pub retention_policy_id: String,
    pub delete_after: String,
    pub target_count: usize,
    pub planned_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerSupportEffect {
    pub kind: LaunchEffectKind,
    pub state: EffectState,
    pub intent_key: String,
    pub attempt: u32,
    pub reconciliation_attempt: u32,
    pub fence: u64,
    pub provider_request_id: Option<String>,
    pub external_resource_ids: BTreeMap<String, String>,
    pub failure_class: Option<FailureClass>,
    pub failure_code: Option<String>,
    pub failure_correlation_id: Option<String>,
    pub next_attempt_at: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerSupportEvent {
    pub sequence: u64,
    pub occurred_at: String,
    pub correlation_id: String,
    pub kind: TriggerSupportEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TriggerSupportEventKind {
    Created,
    DraftUpdated,
    ConnectionSelected,
    Started,
    StateChanged {
        from: LaunchState,
        to: LaunchState,
    },
    StepChanged {
        key: LaunchStepKey,
        status: LaunchStepStatus,
        attempt: u32,
        worker_fencing_token: u64,
        external_resource_ids: BTreeMap<String, String>,
    },
    Blocked {
        reason: LaunchBlockedReason,
    },
    Resumed {
        reason: LaunchBlockedReason,
    },
    Failure {
        class: FailureClass,
        code: String,
    },
    EffectReconciled {
        effect: LaunchEffectKind,
    },
    RecoveryAuthorized {
        effect: LaunchEffectKind,
    },
    DeliveryChanged {
        state: PromptDeliveryState,
    },
    ReachChanged {
        state: CapabilityRevocationState,
        generation: u64,
        fence: u64,
    },
    ConfirmationRecorded {
        action: ConfirmationAction,
    },
    CleanupPlanned {
        target_count: usize,
    },
    CleanupCompleted {
        target_count: usize,
    },
    Cancelled,
}

impl TriggerSupportSnapshot {
    pub fn from_launch(launch: &Launch) -> Self {
        let effects = launch
            .effects
            .iter()
            .map(|effect| TriggerSupportEffect {
                kind: effect.kind,
                state: effect.state,
                intent_key: effect.intent_key.clone(),
                attempt: effect.attempt,
                reconciliation_attempt: effect.reconciliation_attempt,
                fence: effect.fence,
                provider_request_id: effect.provider_request_id.clone(),
                external_resource_ids: safe_external_resource_ids(&effect.external_resource_ids),
                failure_class: effect.last_failure.as_ref().map(|failure| failure.class),
                failure_code: effect
                    .last_failure
                    .as_ref()
                    .map(|failure| failure.code.clone()),
                failure_correlation_id: effect
                    .last_failure
                    .as_ref()
                    .map(|failure| failure.correlation_id.clone()),
                next_attempt_at: effect.next_attempt_at.clone(),
                updated_at: effect.updated_at.clone(),
            })
            .collect();
        let timeline = launch
            .events
            .iter()
            .rev()
            .take(MAX_TRIGGER_SUPPORT_TIMELINE)
            .rev()
            .map(|event| TriggerSupportEvent {
                sequence: event.sequence,
                occurred_at: event.occurred_at.clone(),
                correlation_id: event.correlation_id.clone(),
                kind: support_event(&event.kind),
            })
            .collect();
        Self {
            launch_id: launch.id.clone(),
            owner: launch.owner.clone(),
            revision_digest: launch.revision_digest.clone(),
            publisher: launch.revision.content.publisher_subject_id.clone(),
            upstream_repository_id: launch.revision.content.upstream.repository.repository_id,
            fork_repository_id: launch
                .fork
                .as_ref()
                .map(|fork| fork.repository.repository_id),
            github_account_id: launch
                .connection
                .as_ref()
                .map(|connection| connection.account.account_id),
            service_id: launch
                .connection
                .as_ref()
                .map(|connection| connection.service_id.clone()),
            session_id: launch
                .session
                .as_ref()
                .map(|session| session.session_id.0.clone()),
            state: launch.state,
            blocked_reason: launch.blocked_reason,
            created_at: launch.created_at.clone(),
            updated_at: launch.updated_at.clone(),
            started_at: launch.started_at.clone(),
            terminal_at: launch.terminal_at.clone(),
            lease: launch.lease.as_ref().map(|lease| TriggerSupportLease {
                fence: lease.fencing_token,
                acquired_at: lease.acquired_at.clone(),
                expires_at: lease.expires_at.clone(),
            }),
            cleanup: launch
                .cleanup
                .as_ref()
                .map(|cleanup| TriggerSupportCleanup {
                    retention_policy_id: cleanup.plan.retention_policy_id.clone(),
                    delete_after: cleanup.plan.delete_after.clone(),
                    target_count: cleanup.plan.targets.len(),
                    planned_at: cleanup.plan.planned_at.clone(),
                    completed_at: cleanup.completed_at.clone(),
                }),
            effects,
            timeline,
        }
    }
}

fn support_event(kind: &LaunchEventKind) -> TriggerSupportEventKind {
    match kind {
        LaunchEventKind::Created => TriggerSupportEventKind::Created,
        LaunchEventKind::DraftUpdated => TriggerSupportEventKind::DraftUpdated,
        LaunchEventKind::ConnectionSelected => TriggerSupportEventKind::ConnectionSelected,
        LaunchEventKind::Started => TriggerSupportEventKind::Started,
        LaunchEventKind::StateChanged { from, to } => TriggerSupportEventKind::StateChanged {
            from: *from,
            to: *to,
        },
        LaunchEventKind::StepChanged { step } => TriggerSupportEventKind::StepChanged {
            key: step.key,
            status: step.status,
            attempt: step.attempt,
            worker_fencing_token: step.worker_fencing_token,
            external_resource_ids: safe_external_resource_ids(&step.external_resource_ids),
        },
        LaunchEventKind::Blocked { reason } => TriggerSupportEventKind::Blocked { reason: *reason },
        LaunchEventKind::Resumed { reason } => TriggerSupportEventKind::Resumed { reason: *reason },
        LaunchEventKind::Failure { failure } => TriggerSupportEventKind::Failure {
            class: failure.class,
            code: failure.code.clone(),
        },
        LaunchEventKind::EffectReconciled { effect } => {
            TriggerSupportEventKind::EffectReconciled { effect: *effect }
        }
        LaunchEventKind::RecoveryAuthorized { effect } => {
            TriggerSupportEventKind::RecoveryAuthorized { effect: *effect }
        }
        LaunchEventKind::DeliveryChanged { state } => {
            TriggerSupportEventKind::DeliveryChanged { state: *state }
        }
        LaunchEventKind::ReachChanged { reach } => TriggerSupportEventKind::ReachChanged {
            state: reach.state,
            generation: reach.policy_generation,
            fence: reach.policy_fence,
        },
        LaunchEventKind::ConfirmationRecorded { action } => {
            TriggerSupportEventKind::ConfirmationRecorded { action: *action }
        }
        LaunchEventKind::CleanupPlanned { target_count } => {
            TriggerSupportEventKind::CleanupPlanned {
                target_count: *target_count,
            }
        }
        LaunchEventKind::CleanupCompleted { target_count } => {
            TriggerSupportEventKind::CleanupCompleted {
                target_count: *target_count,
            }
        }
        LaunchEventKind::Cancelled => TriggerSupportEventKind::Cancelled,
    }
}

fn safe_external_resource_ids(values: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    const SAFE_KEYS: &[&str] = &[
        "account_id",
        "base_tree_id",
        "branch_head",
        "branch_ref",
        "checkpoint_id",
        "checkpoint_version",
        "commit_id",
        "diff_artifact_digest",
        "diff_artifact_id",
        "fork_owner_account_id",
        "fork_repository_id",
        "policy_expires_at",
        "policy_fence",
        "policy_generation",
        "provider_request_id",
        "pull_request_number",
        "pull_request_provider_id",
        "pull_request_state",
        "resulting_head",
        "runtime_audience",
        "service_id",
        "session_id",
        "source_sha",
        "tree_id",
        "upstream_repository_id",
        "workspace_digest",
        "workspace_entry_count",
    ];
    values
        .iter()
        .filter(|(key, _)| SAFE_KEYS.binary_search(&key.as_str()).is_ok())
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trigger::tests::revision_fixture;

    #[test]
    fn support_projection_has_no_owner_content_or_object_references() {
        let mut launch = Launch::new(
            SubjectId("owner".into()),
            revision_fixture(SubjectId("publisher".into())),
            "2026-08-27T00:00:00Z",
            "support-correlation",
        )
        .unwrap();
        launch.effective.rendered_prompt = "PRIVATE PROMPT".into();
        launch.overrides.task_inputs.insert(
            "secret".into(),
            serde_json::Value::String("PRIVATE INPUT".into()),
        );
        launch.lease = Some(crate::trigger::WorkerLease {
            holder: "PRIVATE WORKER HOLDER".into(),
            fencing_token: 7,
            acquired_at: "2026-08-27T00:00:01Z".into(),
            expires_at: "2026-08-27T00:02:01Z".into(),
        });
        let support = TriggerSupportSnapshot::from_launch(&launch);
        let json = serde_json::to_string(&support).unwrap();
        assert!(!json.contains("PRIVATE PROMPT"));
        assert!(!json.contains("PRIVATE INPUT"));
        assert!(!json.contains("PRIVATE WORKER HOLDER"));
        assert!(!json.contains("task_inputs"));
        assert_eq!(support.lease.as_ref().map(|lease| lease.fence), Some(7));
    }

    #[test]
    fn support_external_ids_use_a_closed_allowlist() {
        let values = BTreeMap::from([
            ("provider_request_id".into(), "request-1".into()),
            (
                "workspace_object_reference".into(),
                "private/object/key".into(),
            ),
            ("unexpected".into(), "must-not-appear".into()),
        ]);
        assert_eq!(
            safe_external_resource_ids(&values),
            BTreeMap::from([("provider_request_id".into(), "request-1".into())])
        );
    }
}
